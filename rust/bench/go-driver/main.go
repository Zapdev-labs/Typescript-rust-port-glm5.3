// Command go-bench is the Go-side benchmark driver for the performance
// program (rust/SPEC.md §15, Phase A). It races the Rust port's `tsc-bench`
// (rust/crates/bench-harness) over the same corpus with identical work:
//
//	go-bench parse --corpus <dir> [--threads N] [--iterations K] [--json]
//
// Contract (both drivers MUST match; see rust/SPEC.md §15 "The race is honest"):
//
//   - Corpus walk: deterministic. Files with extensions .ts, .tsx, .mts,
//     .cts (".d.ts" ends in ".ts" and is therefore covered), sorted by path.
//     Directories named node_modules, .git, dist, build, out, .yarn are
//     skipped entirely.
//   - File IO happens BEFORE timing starts; each file is read fully into
//     memory once and the same string is reused across all iterations.
//   - Timing covers ONLY parse + AST walk, all K passes, whole file list.
//   - Output is a single JSON object whose counted fields (files, bytes,
//     nodes, errors, shape_hash) are identical at ANY thread count.
//
// The Rust driver must reproduce the scriptKind mapping (see scriptKind
// comment under indexCorpus) and the shape_hash recipe (see shapeHash).
package main

import (
	"bytes"
	stdjson "encoding/json"
	"flag"
	"fmt"
	"io/fs"
	"math"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	tscjson "github.com/microsoft/TypeScript/tsc/internal/json"
	"github.com/microsoft/TypeScript/tsc/internal/module"
	"github.com/microsoft/TypeScript/tsc/internal/parser"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
	"github.com/microsoft/TypeScript/tsc/internal/vfs"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/osvfs"
)

func main() {
	if len(os.Args) < 2 {
		usage()
	}
	switch os.Args[1] {
	case "parse":
		runParse(os.Args[2:])
	case "dump":
		runDump(os.Args[2:])
	case "showconfig":
		runShowConfig(os.Args[2:])
	case "resolvereact":
		runResolveReact(os.Args[2:])
	case "help", "-h", "--help":
		usage()
	default:
		fmt.Fprintf(os.Stderr, "go-bench: unknown command %q\n", os.Args[1])
		usage()
	}
}

func usage() {
	fmt.Fprintln(os.Stderr, "usage: go-bench parse --corpus <dir> [--threads N] [--iterations K] [--json]")
	fmt.Fprintln(os.Stderr, "       go-bench dump --corpus <dir>   (per-file NDJSON: nodes, diagnostics, histogram)")
	fmt.Fprintln(os.Stderr, "       go-bench showconfig --corpus <dir> | --tsconfig <path>   (per-config NDJSON: marshaled TSConfig)")
	fmt.Fprintln(os.Stderr, "       go-bench resolvereact --corpus <dir>   (per-specifier NDJSON + summary)")
	os.Exit(2)
}

// runDump is the parity-debugging twin of the Rust driver's `tsc-bench dump`:
// parses every file (threads=1, walk order) and prints one NDJSON line per
// file with nodes, diagnostics (key/code/pos/end/args) and the kind
// histogram. Both drivers emit byte-identical lines for identical parses —
// diff the outputs to localize divergence.
func runDump(args []string) {
	fs := flag.NewFlagSet("dump", flag.ExitOnError)
	corpus := fs.String("corpus", "", "corpus directory")
	_ = fs.Parse(args)
	if *corpus == "" {
		fmt.Fprintln(os.Stderr, "go-bench dump: missing required --corpus <dir>")
		os.Exit(2)
	}
	files, _, err := indexCorpus(*corpus)
	if err != nil {
		fmt.Fprintf(os.Stderr, "go-bench: %v\n", err)
		os.Exit(1)
	}
	var sb strings.Builder
	for i := range files {
		f := &files[i]
		sf := parser.ParseSourceFile(ast.SourceFileParseOptions{
			FileName: f.fileName,
			PathKey:  f.pathKey,
		}, f.text, f.scriptKind)
		s := walkForDump(sf.AsNode())
		sb.Reset()
		sb.WriteString(`{"file":"`)
		jsonEscape(&sb, f.fileName.AsString())
		sb.WriteString(`","nodes":`)
		sb.WriteString(strconv.FormatInt(s.nodes, 10))
		sb.WriteString(`,"errors":`)
		sb.WriteString(strconv.Itoa(len(sf.Diagnostics())))
		sb.WriteString(`,"diag":[`)
		for di, d := range sf.Diagnostics() {
			if di > 0 {
				sb.WriteByte(',')
			}
			sb.WriteString(`{"key":"`)
			jsonEscape(&sb, string(d.MessageKey()))
			sb.WriteString(`","code":`)
			sb.WriteString(strconv.FormatInt(int64(d.Code()), 10))
			sb.WriteString(`,"pos":`)
			sb.WriteString(strconv.Itoa(d.Pos()))
			sb.WriteString(`,"end":`)
			sb.WriteString(strconv.Itoa(d.End()))
			sb.WriteString(`,"args":[`)
			for ai, a := range d.MessageArgs() {
				if ai > 0 {
					sb.WriteByte(',')
				}
				sb.WriteByte('"')
				jsonEscape(&sb, a)
				sb.WriteByte('"')
			}
			sb.WriteString(`]}`)
		}
		sb.WriteString(`],"hist":"`)
		first := true
		for o := 0; o < int(ast.KindCount); o++ {
			if c := s.hist[ast.Kind(o)]; c > 0 {
				if !first {
					sb.WriteByte(' ')
				}
				first = false
				sb.WriteString(strconv.Itoa(o))
				sb.WriteByte(':')
				sb.WriteString(strconv.FormatInt(c, 10))
			}
		}
		sb.WriteString(`"}`)
		fmt.Println(sb.String())
	}
}

// walkForDump counts nodes and histogram entries exactly like parseAndWalk
// (contract rule 2), minus the diagnostics count (runDump adds it).
func walkForDump(root *ast.Node) fileStats {
	var s fileStats
	s.hist = make(map[ast.Kind]int64)
	var visit ast.Visitor
	visit = func(n *ast.Node) bool {
		s.nodes++
		s.hist[n.Kind]++
		n.ForEachChild(visit)
		return false
	}
	visit(root)
	return s
}

// jsonEscape implements the minimal, deliberately-shared escaping rules the
// Rust driver mirrors byte-for-byte (`json_escape` in parse.rs): `"` and `\`
// and control characters only. Not encoding/json's rules (no HTML escaping,
// no \n/\t shorthand) — identical bytes on both sides is the whole point.
func jsonEscape(sb *strings.Builder, s string) {
	for i := 0; i < len(s); i++ {
		c := s[i]
		switch {
		case c == '"':
			sb.WriteString(`\"`)
		case c == '\\':
			sb.WriteString(`\\`)
		case c < 0x20:
			fmt.Fprintf(sb, `\u%04x`, c)
		default:
			sb.WriteByte(c)
		}
	}
}

// ─────────────────────────────────────────────────────────────────────
// showconfig: the tsconfig→TSConfig parity twin (Harness 1).
//
//	go-bench showconfig --corpus <dir>
//	go-bench showconfig --tsconfig <path>
//
// For every tsconfig.json/jsconfig.json under the corpus (same walk/skip
// rules as dump), runs the full showConfig pipeline exactly the way the
// tsoptions tests drive it:
//
//	text → ParseConfigFileTextToJson (order-preserving)
//	     → ParseJsonConfigFileContent (extends resolution, osvfs host)
//	     → ConvertToTSConfig
//	     → json.MarshalIndentWrite(., "", "    ")   (tsc.go's showConfig)
//
// and prints one NDJSON line per config; the Rust driver's `tsc-bench
// showconfig` emits byte-identical lines:
//
//	{"file":"...","ok":true,"out":"<marshaled TSConfig JSON>","errors":N}
//	{"file":"...","ok":false,"error":"cannot read file"}
// ─────────────────────────────────────────────────────────────────────

func runShowConfig(args []string) {
	cli := flag.NewFlagSet("showconfig", flag.ExitOnError)
	corpus := cli.String("corpus", "", "corpus directory")
	tsconfig := cli.String("tsconfig", "", "single tsconfig/jsconfig path")
	_ = cli.Parse(args)
	if *corpus == "" && *tsconfig == "" {
		fmt.Fprintln(os.Stderr, "go-bench showconfig: missing required --corpus <dir> or --tsconfig <path>")
		os.Exit(2)
	}
	var configPaths []string
	if *tsconfig != "" {
		abs, err := filepath.Abs(*tsconfig)
		if err != nil {
			fmt.Fprintf(os.Stderr, "go-bench: resolving tsconfig path: %v\n", err)
			os.Exit(1)
		}
		configPaths = []string{abs}
	} else {
		var err error
		configPaths, err = indexConfigFiles(*corpus)
		if err != nil {
			fmt.Fprintf(os.Stderr, "go-bench: %v\n", err)
			os.Exit(1)
		}
	}
	host := osvfs.FS()
	var sb strings.Builder
	for _, p := range configPaths {
		sb.Reset()
		emitShowConfigLine(&sb, p, host)
		fmt.Println(sb.String())
	}
}

// configBaseNames are the config file names the showconfig corpus walk
// collects (exact-name, case-sensitive — both drivers).
var configBaseNames = map[string]bool{
	"tsconfig.json": true,
	"jsconfig.json": true,
}

// indexConfigFiles walks the corpus with dump's walk/skip rules and returns
// every tsconfig.json/jsconfig.json path, sorted by path (raw string sort,
// exactly like indexCorpus).
func indexConfigFiles(dir string) ([]string, error) {
	absCorpus, err := filepath.Abs(dir)
	if err != nil {
		return nil, fmt.Errorf("resolving corpus dir: %w", err)
	}
	var paths []string
	err = filepath.WalkDir(absCorpus, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			if path == absCorpus {
				return err
			}
			return nil
		}
		if d.IsDir() {
			if path != absCorpus && skipDirs[d.Name()] {
				return fs.SkipDir
			}
			return nil
		}
		if configBaseNames[d.Name()] {
			paths = append(paths, path)
		}
		return nil
	})
	if err != nil {
		return nil, fmt.Errorf("walking corpus: %w", err)
	}
	sort.Strings(paths)
	return paths, nil
}

// emitShowConfigLine renders the NDJSON parity line for one config file.
func emitShowConfigLine(sb *strings.Builder, path string, host vfs.FS) {
	fileName := tspath.ToRootedFilePath(filepath.ToSlash(path), "/")
	sb.WriteString(`{"file":"`)
	jsonEscape(sb, fileName.AsString())
	sb.WriteString(`",`)
	text, err := os.ReadFile(path)
	if err != nil {
		// Both drivers emit the same fixed marker for unreadable files
		// (os vs std::io error strings differ; fixed text keeps the diff
		// meaningful — PORT-noted in the Rust twin).
		sb.WriteString(`"ok":false,"error":"cannot read file"}`)
		return
	}
	basePath := fileName.Directory()
	pathKey := host.CaseSensitivity().PathKey(fileName.AsPath())
	jsonValue, textDiags := tsoptions.ParseConfigFileTextToJson(fileName, pathKey, string(text))
	parsed := tsoptions.ParseJsonConfigFileContent(jsonValue, host, basePath, nil /*existingOptions*/, fileName, nil /*resolutionStack*/, nil /*extendedConfigCache*/)
	errorCount := len(textDiags) + len(parsed.Errors)
	tsConfig := tsoptions.ConvertToTSConfig(parsed, fileName)
	var buf bytes.Buffer
	if err := tscjson.MarshalIndentWrite(&buf, tsConfig, "", "    "); err != nil {
		sb.WriteString(`"ok":false,"error":"marshal error"}`)
		return
	}
	sb.WriteString(`"ok":true,"out":"`)
	jsonEscape(sb, buf.String())
	sb.WriteString(`","errors":`)
	sb.WriteString(strconv.Itoa(errorCount))
	sb.WriteString(`}`)
}

// ─────────────────────────────────────────────────────────────────────
// resolvereact: the module-resolution parity twin (Harness 2).
//
//	go-bench resolvereact --corpus <dir>
//
// For every tsconfig.json/jsconfig.json/package.json-bearing directory in
// the corpus (dump's walk/skip rules, so node_modules trees are visible to
// the resolver FS but never project dirs), resolves a fixed canonical
// specifier list from the directory (PORT: a synthetic containing file
// <dir>/__resolvereact__.ts — only its directory participates in
// resolution), with moduleResolution taken from the project's tsconfig
// (full tsoptions pipeline) or the module tests' default literal
// {Bundler, ESNext, ESNext}. One NDJSON line per specifier:
//
//	{"file":"...","specifier":"...","kind":<ModuleResolutionKind int>,
//	 "resolved":"<path>"|null,"failed":<bool>,"external":<bool>,
//	 "pkgId":"<name@version>"|null}
//
// plus a final summary line whose shape_hash is the FNV-1a 64 fold of every
// record's fields (same recipe as parse's shapeHash):
//
//	{"summary":true,"dirs":N,"specs":M,"resolved":R,"failed":F,
//	 "shape_hash":"<16 lowercase hex>"}
// ─────────────────────────────────────────────────────────────────────

// resolvereactSpecifiers is the fixed canonical specifier list, resolved
// identically by both drivers from every project directory (PORT: fixed
// list over AST-extracted specifiers — byte-stable by construction and
// driveable through the same DefaultResolver call shape on both sides).
var resolvereactSpecifiers = []string{
	"fs", "node:fs", "os",
	"./x", "./x.ts", "./index", "./a/b.js", "./missing", "../y",
	"pkg", "pkg/", "pkg/sub", "pkg/package.json", "pkg/main.js",
	"@scope/pkg", "@scope/pkg/sub", "@scope/missing",
	"idx", "cjspkg",
	"selfpkg", "#dep",
}

// resolveReactHost is the ResolutionHost the module tests construct
// (resolutionTestInputs{fs, cwd}); cwd is pinned to the absolute corpus
// root so no process state leaks into the output.
type resolveReactHost struct {
	fs  vfs.FS
	cwd tspath.RootedDirectoryPath
}

func (h *resolveReactHost) FS() vfs.FS                                      { return h.fs }
func (h *resolveReactHost) GetCurrentDirectory() tspath.RootedDirectoryPath { return h.cwd }

func runResolveReact(args []string) {
	cli := flag.NewFlagSet("resolvereact", flag.ExitOnError)
	corpus := cli.String("corpus", "", "corpus directory")
	_ = cli.Parse(args)
	if *corpus == "" {
		fmt.Fprintln(os.Stderr, "go-bench resolvereact: missing required --corpus <dir>")
		os.Exit(2)
	}
	absCorpus, err := filepath.Abs(*corpus)
	if err != nil {
		fmt.Fprintf(os.Stderr, "go-bench: resolving corpus dir: %v\n", err)
		os.Exit(1)
	}
	host := osvfs.FS()
	reactHost := &resolveReactHost{fs: host, cwd: tspath.RootedDirectoryPath(filepath.ToSlash(absCorpus))}
	projectDirs, err := indexProjectDirs(absCorpus, host)
	if err != nil {
		fmt.Fprintf(os.Stderr, "go-bench: %v\n", err)
		os.Exit(1)
	}

	// shape_hash accumulator (FNV-1a 64, parse's shapeHash recipe).
	h := uint64(0xcbf29ce484222325)
	fold := func(s string) {
		for i := 0; i < len(s); i++ {
			h ^= uint64(s[i])
			h *= 1099511628211
		}
	}
	var sb strings.Builder
	var specs, resolvedCount, failedCount int
	for _, dir := range projectDirs {
		opts := projectCompilerOptions(dir, host)
		resolver := module.NewResolver(module.ResolverOptions{Host: reactHost, CompilerOptions: opts})
		containingFile := tspath.ToRootedFilePath(filepath.ToSlash(filepath.Join(dir, "__resolvereact__.ts")), "/")
		for _, spec := range resolvereactSpecifiers {
			r, _, _ := resolver.ResolveModuleName(spec, containingFile, core.ModuleKindESNext /*resolutionMode: ESM*/, nil /*redirectedReference*/)
			specs++
			isResolved := r.IsResolved()
			var external bool
			var pkgID string
			if isResolved {
				resolvedCount++
				external = r.IsExternalLibraryImport
				// pkgId is emitted only for a non-zero PackageId (the zero
				// struct's String() would render a meaningless "@"); both
				// drivers use the same zero-value test.
				if r.PackageId != (module.PackageId{}) {
					pkgID = r.PackageId.String()
				}
			} else {
				failedCount++
			}
			sb.Reset()
			sb.WriteString(`{"file":"`)
			jsonEscape(&sb, containingFile.AsString())
			sb.WriteString(`","specifier":"`)
			jsonEscape(&sb, spec)
			sb.WriteString(`","kind":`)
			sb.WriteString(strconv.FormatInt(int64(opts.ModuleResolution), 10))
			if isResolved {
				sb.WriteString(`,"resolved":"`)
				jsonEscape(&sb, r.ResolvedFileName.AsString())
				sb.WriteString(`","failed":false,"external":`)
				sb.WriteString(strconv.FormatBool(external))
			} else {
				sb.WriteString(`,"resolved":null,"failed":true,"external":false`)
			}
			if pkgID != "" {
				sb.WriteString(`,"pkgId":"`)
				jsonEscape(&sb, pkgID)
				sb.WriteString(`"`)
			} else {
				sb.WriteString(`,"pkgId":null`)
			}
			sb.WriteString(`}`)
			fmt.Println(sb.String())
			// Fold the record's fields into the shape hash.
			fold("file=" + containingFile.AsString() + "\n")
			fold("specifier=" + spec + "\n")
			fold("kind=" + strconv.FormatInt(int64(opts.ModuleResolution), 10) + "\n")
			if isResolved {
				fold("resolved=" + r.ResolvedFileName.AsString() + "\n")
				fold("failed=0\n")
				fold("external=" + map[bool]string{true: "1", false: "0"}[external] + "\n")
			} else {
				fold("resolved=\n")
				fold("failed=1\n")
				fold("external=0\n")
			}
			fold("pkgId=" + pkgID + "\n")
		}
	}
	fmt.Printf(`{"summary":true,"dirs":%d,"specs":%d,"resolved":%d,"failed":%d,"shape_hash":"%016x"}`+"\n",
		len(projectDirs), specs, resolvedCount, failedCount, h)
}

// projectBaseNames are the files that make a directory a project dir for
// resolvereact (tsconfig or package.json bearing).
var projectBaseNames = map[string]bool{
	"tsconfig.json": true,
	"jsconfig.json": true,
	"package.json":  true,
}

// indexProjectDirs walks the corpus (dump's skip rules) and returns the
// sorted list of directories that directly contain a tsconfig.json,
// jsconfig.json or package.json.
func indexProjectDirs(absCorpus string, host vfs.FS) ([]string, error) {
	dirSet := map[string]bool{}
	err := filepath.WalkDir(absCorpus, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			if path == absCorpus {
				return err
			}
			return nil
		}
		if d.IsDir() {
			if path != absCorpus && skipDirs[d.Name()] {
				return fs.SkipDir
			}
			return nil
		}
		if projectBaseNames[d.Name()] {
			dirSet[filepath.Dir(path)] = true
		}
		return nil
	})
	if err != nil {
		return nil, fmt.Errorf("walking corpus: %w", err)
	}
	dirs := make([]string, 0, len(dirSet))
	for d := range dirSet {
		dirs = append(dirs, d)
	}
	sort.Strings(dirs)
	return dirs, nil
}

// projectCompilerOptions returns the project's CompilerOptions: parsed from
// its tsconfig.json (or jsconfig.json) through the same pipeline the
// showconfig twin drives, or the module tests' default literal when the
// directory has no config (PORT-noted default).
func projectCompilerOptions(dir string, host vfs.FS) *core.CompilerOptions {
	for _, name := range []string{"tsconfig.json", "jsconfig.json"} {
		fileName := tspath.ToRootedFilePath(filepath.ToSlash(filepath.Join(dir, name)), "/")
		if !host.FileExists(fileName) {
			continue
		}
		text, ok := host.ReadFile(fileName)
		if !ok {
			break
		}
		pathKey := host.CaseSensitivity().PathKey(fileName.AsPath())
		jsonValue, _ := tsoptions.ParseConfigFileTextToJson(fileName, pathKey, text)
		parsed := tsoptions.ParseJsonConfigFileContent(jsonValue, host, fileName.Directory(), nil /*existingOptions*/, fileName, nil /*resolutionStack*/, nil /*extendedConfigCache*/)
		return parsed.CompilerOptions()
	}
	// The default literal the module tests use (resolver_test.go's opts).
	return &core.CompilerOptions{
		ModuleResolution: core.ModuleResolutionKindBundler,
		Module:           core.ModuleKindESNext,
		Target:           core.ScriptTargetESNext,
	}
}

// benchFile is one indexed corpus file: text, precomputed parse inputs, and
// its byte size. Built before the timed section; reused across iterations.
type benchFile struct {
	text       string
	bytes      int64
	fileName   tspath.RootedFilePath
	pathKey    tspath.PathKey
	scriptKind core.ScriptKind
}

// fileStats is the per-file result of one parse+walk pass.
type fileStats struct {
	nodes  int64
	errors int64
	hist   map[ast.Kind]int64
}

// skipDirs are directory names skipped during the corpus walk.
var skipDirs = map[string]bool{
	"node_modules": true,
	".git":         true,
	"dist":         true,
	"build":        true,
	"out":          true,
	".yarn":        true,
}

// benchExts are the file extensions indexed from the corpus. ".d.ts" ends
// with ".ts" (filepath.Ext returns ".ts") and is therefore covered.
var benchExts = map[string]bool{
	".ts":  true,
	".tsx": true,
	".mts": true,
	".cts": true,
}

func runParse(args []string) {
	cli := flag.NewFlagSet("parse", flag.ExitOnError)
	corpus := cli.String("corpus", "", "corpus directory")
	threads := cli.Int("threads", 1, "worker goroutines (default 1)")
	iterations := cli.Int("iterations", 1, "passes over the corpus (default 1)")
	itersAlias := cli.Int("iters", 0, "alias for --iterations (tsc-bench spelling)")
	jsonOut := cli.Bool("json", false, "emit a single JSON object on stdout")
	cli.Parse(args)

	if *corpus == "" {
		fmt.Fprintln(os.Stderr, "go-bench: missing required --corpus <dir>")
		os.Exit(2)
	}
	if *itersAlias > 0 {
		*iterations = *itersAlias
	}
	if *threads < 1 {
		*threads = 1
	}
	if *iterations < 1 {
		*iterations = 1
	}

	files, readErrs, err := indexCorpus(*corpus)
	if err != nil {
		fmt.Fprintf(os.Stderr, "go-bench: %v\n", err)
		os.Exit(1)
	}

	var totalBytes int64
	for i := range files {
		totalBytes += files[i].bytes
	}
	// Unreadable files count as one error each (see indexCorpus) and are
	// excluded from files/bytes. They are included in the errors total and
	// therefore folded into shape_hash. Both drivers must mirror this.
	totalErrors := int64(readErrs)

	// ── Timed section: parse + walk only, across all K passes. ──
	start := time.Now()
	totalNodes := int64(0)
	hist := make(map[ast.Kind]int64)
	for iter := 0; iter < *iterations; iter++ {
		stats := runPass(files, *threads)
		// Merge in file-index order so aggregation is deterministic
		// regardless of worker scheduling.
		for i := range stats {
			totalNodes += stats[i].nodes
			totalErrors += stats[i].errors
			for k, c := range stats[i].hist {
				hist[k] += c
			}
		}
	}
	wall := time.Since(start)

	wallMs := round3(float64(wall) / float64(time.Millisecond))
	var mbPerS float64
	if secs := wall.Seconds(); secs > 0 {
		mbPerS = round3((float64(totalBytes) * float64(*iterations) / (1024 * 1024)) / secs)
	}

	res := result{
		Driver:     "go-bench",
		Op:         "parse",
		Files:      len(files),
		Bytes:      totalBytes,
		Iterations: *iterations,
		Threads:    *threads,
		WallMs:     wallMs,
		MbPerS:     mbPerS,
		Nodes:      totalNodes,
		Errors:     totalErrors,
		ShapeHash:  shapeHash(totalNodes, totalErrors, hist),
	}

	if *jsonOut {
		out, err := stdjson.Marshal(res)
		if err != nil {
			fmt.Fprintf(os.Stderr, "go-bench: marshal: %v\n", err)
			os.Exit(1)
		}
		fmt.Println(string(out))
		return
	}
	fmt.Printf("driver:      %s\n", res.Driver)
	fmt.Printf("op:          %s\n", res.Op)
	fmt.Printf("files:       %d\n", res.Files)
	fmt.Printf("bytes:       %d\n", res.Bytes)
	fmt.Printf("iterations:  %d\n", res.Iterations)
	fmt.Printf("threads:     %d\n", res.Threads)
	fmt.Printf("wall_ms:     %.3f\n", res.WallMs)
	fmt.Printf("mb_per_s:    %.3f\n", res.MbPerS)
	fmt.Printf("nodes:       %d\n", res.Nodes)
	fmt.Printf("errors:      %d\n", res.Errors)
	fmt.Printf("shape_hash:  %s\n", res.ShapeHash)
}

// result is the JSON contract. Field order IS the output key order:
//
//	{"driver":"go-bench","op":"parse","files":N,"bytes":B,"iterations":K,
//	 "threads":T,"wall_ms":X,"mb_per_s":Y,"nodes":Z,"errors":E,
//	 "shape_hash":"<hex>"}
type result struct {
	Driver     string  `json:"driver"`
	Op         string  `json:"op"`
	Files      int     `json:"files"`
	Bytes      int64   `json:"bytes"`
	Iterations int     `json:"iterations"`
	Threads    int     `json:"threads"`
	WallMs     float64 `json:"wall_ms"`
	MbPerS     float64 `json:"mb_per_s"`
	Nodes      int64   `json:"nodes"`
	Errors     int64   `json:"errors"`
	ShapeHash  string  `json:"shape_hash"`
}

// indexCorpus walks the corpus deterministically and reads every indexed
// file fully into memory before any timing starts.
//
// scriptKind mapping: this driver uses the reference helper
// core.GetScriptKindFromFileName (tsc/internal/core/core.go:525), which maps
// by final extension:
//
//	.js/.cjs/.mjs -> ScriptKindJS(1)      .jsx -> ScriptKindJSX(2)
//	.ts/.d.ts/.cts/.mts -> ScriptKindTS(3)   .tsx -> ScriptKindTSX(4)
//	.json -> ScriptKindJSON(6)             (else -> ScriptKindUnknown(0))
//
// Since the driver only indexes .ts/.tsx/.mts/.cts, the effective mapping is:
// .ts, .d.ts, .mts, .cts -> ScriptKindTS(3); .tsx -> ScriptKindTSX(4).
// The Rust driver must mirror exactly this: TSX for ".tsx", TS for the rest.
func indexCorpus(dir string) ([]benchFile, int64, error) {
	absCorpus, err := filepath.Abs(dir)
	if err != nil {
		return nil, 0, fmt.Errorf("resolving corpus dir: %w", err)
	}

	var paths []string
	var readErrs int64
	err = filepath.WalkDir(absCorpus, func(path string, d fs.DirEntry, err error) error {
		if err != nil {
			// Unreadable root is fatal; unreadable entries count as
			// errors and the walk continues.
			if path == absCorpus {
				return err
			}
			readErrs++
			return nil
		}
		if d.IsDir() {
			if path != absCorpus && skipDirs[d.Name()] {
				return fs.SkipDir
			}
			return nil
		}
		if benchExts[strings.ToLower(filepath.Ext(path))] {
			paths = append(paths, path)
		}
		return nil
	})
	if err != nil {
		return nil, 0, fmt.Errorf("walking corpus: %w", err)
	}

	// WalkDir is depth-first lexical per directory; sort globally by path
	// so both drivers walk the identical order.
	sort.Strings(paths)

	files := make([]benchFile, 0, len(paths))
	for _, p := range paths {
		data, err := os.ReadFile(p)
		if err != nil {
			// Unreadable file: +1 error, continue.
			readErrs++
			continue
		}
		fileName := tspath.ToRootedFilePath(filepath.ToSlash(p), "/")
		files = append(files, benchFile{
			text:       string(data),
			bytes:      int64(len(data)),
			fileName:   fileName,
			pathKey:    tspath.CaseSensitive.PathKey(fileName.AsPath()),
			scriptKind: core.GetScriptKindFromFileName(fileName),
		})
	}
	return files, readErrs, nil
}

// runPass parses+walks every file once, using `threads` worker goroutines
// over the indexed file list. Each pass fully joins before returning, so a
// pass's per-file stats slots (stats[i]) are each written by exactly one
// worker and merged by the caller in file-index order — output is identical
// at any thread count.
func runPass(files []benchFile, threads int) []fileStats {
	stats := make([]fileStats, len(files))
	if len(files) == 0 {
		return stats
	}
	if threads == 1 {
		for i := range files {
			stats[i] = parseAndWalk(&files[i])
		}
		return stats
	}
	var next atomic.Int64
	var wg sync.WaitGroup
	for w := 0; w < threads; w++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for {
				i := int(next.Add(1)) - 1
				if i >= len(files) {
					return
				}
				stats[i] = parseAndWalk(&files[i])
			}
		}()
	}
	wg.Wait()
	return stats
}

// parseAndWalk parses one file with the reference parser
// (parser.ParseSourceFile, tsc/internal/parser/parser.go:134) and counts:
//
//   - nodes: every node reached from the SourceFile root through the
//     non-JSDoc child walker Node.ForEachChild (tsc/internal/ast/
//     ast_generated.go:8693), INCLUDING the SourceFile root itself and the
//     EndOfFileToken (both yielded by SourceFile.ForEachChild, ast.go:2782).
//     NodeList wrappers are NOT counted (visitNodeList calls the visitor on
//     the children only). The Rust driver must count exactly this set.
//   - errors: len(sf.Diagnostics()) (tsc/internal/ast/ast.go:2715), i.e.
//     the parse diagnostics only (not js/jsdoc/bind diagnostics).
func parseAndWalk(f *benchFile) fileStats {
	var s fileStats
	s.hist = make(map[ast.Kind]int64)
	sf := parser.ParseSourceFile(ast.SourceFileParseOptions{
		FileName: f.fileName,
		PathKey:  f.pathKey,
	}, f.text, f.scriptKind)
	if sf == nil {
		s.errors = 1
		return s
	}
	var visit ast.Visitor
	visit = func(n *ast.Node) bool {
		s.nodes++
		s.hist[n.Kind]++
		n.ForEachChild(visit) // recurse; return false = "keep going"
		return false
	}
	visit(sf.AsNode())
	s.errors = int64(len(sf.Diagnostics()))
	return s
}

// shapeHash computes the stable digest that the Rust driver
// (`tsc-bench parse`) MUST reproduce byte-for-byte.
//
// Recipe (FNV-1a 64; all integers are plain decimal ASCII, no padding; every
// line ends with a single '\n'; bytes folded in the order written):
//
//	h = 0xcbf29ce484222325                    (FNV-1a 64 offset basis)
//	fold(s): for each ASCII byte b of s in order:
//	    h = h XOR b
//	    h = h * 0x100000001b3  (mod 2^64)     (FNV-1a 64 prime)
//
//	fold("nodes="   + decimal(totalNodes)  + "\n")
//	fold("errors="  + decimal(totalErrors) + "\n")
//	for each kind ordinal o in ascending numeric order where hist[o] > 0:
//	    fold(decimal(o) + ":" + decimal(hist[o]) + "\n")
//
//	shape_hash = lowercase hex(h), zero-padded to 16 digits.
//
// totalNodes/totalErrors are the run totals: summed over every file and
// every one of the K iterations, plus one error per unreadable file. The
// kind ordinal is the integer value of ast.Kind (tsc/internal/ast/
// kind_generated.go: `type Kind int16`; e.g. KindSourceFile's ordinal is
// whatever the generated enum assigns) — the Rust driver must use the
// ported enum's integer value from its own generated kind table.
func shapeHash(totalNodes, totalErrors int64, hist map[ast.Kind]int64) string {
	h := uint64(0xcbf29ce484222325)
	fold := func(s string) {
		for i := 0; i < len(s); i++ {
			h ^= uint64(s[i])
			h *= 1099511628211
		}
	}
	fold("nodes=" + strconv.FormatInt(totalNodes, 10) + "\n")
	fold("errors=" + strconv.FormatInt(totalErrors, 10) + "\n")
	ordinals := make([]int, 0, len(hist))
	for k, c := range hist {
		if c > 0 {
			ordinals = append(ordinals, int(k))
		}
	}
	sort.Ints(ordinals)
	for _, o := range ordinals {
		fold(strconv.Itoa(o) + ":" + strconv.FormatInt(hist[ast.Kind(o)], 10) + "\n")
	}
	return fmt.Sprintf("%016x", h)
}

func round3(x float64) float64 {
	return math.Round(x*1000) / 1000
}

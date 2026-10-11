// ──────────────────────────────────────────────────────────────────────
// go-bench conformance — the M6 conformance twin (SPEC §8 M6: the staged
// baseline climb tracked in rust/CONFORMANCE.md).
//
// This file mirrors rust/crates/tsc-testrunner (compiler_runner.rs +
// test_case_parser.rs + compiler_test_filter.rs + options_generated.rs) and
// rust/crates/tsc-testutil's tsbaseline.rs renderer — NOT the original
// tsc/internal/testrunner harness (the twin discipline: both drivers
// implement the same ported pipeline at the same wired depth, so
// byte-identical output is by construction; the original harness's
// checker-scope arms are deliberately absent here too).
//
// Depth control: --stage parse|bind|check (default bind). The check stage
// mirrors the Rust runner's per-unit semantic seam (the single-file checker
// over each unit's own bytes — see confSemanticDiagnosticsOfUnit); the
// parse/bind stages are as before.
//
// Record contract (per (case, configuration) line, field order fixed):
//
//	{"case":...,"baseline":...,"verdict":"match|mismatch|missing-baseline",
//	 "errors_count":N,"shape_hash":"<16 hex>"}
//
//	- case: the case path relative to --corpus (posix).
//	- baseline: <suite>/<configuredName>.errors.txt.
//	- verdict: match (content equals the reference, or both sides expect
//	  none) | mismatch (reference exists and differs; also the
//	  errors_count:-1 panic sentinel) | missing-baseline (content but no
//	  reference).
//	- errors_count: tsconfig-parse + per-file parse + program diagnostics
//	  at the wired depth; -1 = panic sentinel.
//	- shape_hash: FNV-1a 64 over case/baseline/errors + one fold line per
//	  diagnostic in the render sort order (the recipe in
//	  tsc-testrunner's compiler_runner.rs).
// ──────────────────────────────────────────────────────────────────────

package main

import (
	"context"
	"flag"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"unicode"
	"unicode/utf8"

	"runtime/debug"

	"github.com/microsoft/TypeScript/tsc/internal/ast"
	"github.com/microsoft/TypeScript/tsc/internal/bundled"
	"github.com/microsoft/TypeScript/tsc/internal/checker"
	"github.com/microsoft/TypeScript/tsc/internal/compiler"
	"github.com/microsoft/TypeScript/tsc/internal/core"
	"github.com/microsoft/TypeScript/tsc/internal/locale"
	"github.com/microsoft/TypeScript/tsc/internal/scanner"
	"github.com/microsoft/TypeScript/tsc/internal/tsoptions"
	"github.com/microsoft/TypeScript/tsc/internal/tspath"
	"github.com/microsoft/TypeScript/tsc/internal/vfs/vfstest"
)

const (
	confNoContent  = "<no content>"
	confNewline    = "\r\n"
	confSrcFolder  = "/.src"
	confConfigUnit = "__conformance__.tsconfig.json"
)

// confHarnessOptions — the harness's own directive names (never marshaled
// into a tsconfig).
var confHarnessOptions = map[string]bool{
	"usecasesensitivefilenames": true,
	"baselinefile":              true,
	"includebuiltfile":          true,
	"filename":                  true,
	"libfiles":                  true,
	"noimplicitreferences":      true,
	"currentdirectory":          true,
	"symlink":                   true,
	"link":                      true,
	"notypesandsymbols":         true,
	"fullemitpaths":             true,
	"reportdiagnostics":         true,
	"capturesuggestions":        true,
	"typescriptversion":         true,
	"runexternalcode":           true,
}

// confSkippedTests — Go testrunner's skippedTests, verbatim.
var confSkippedTests = []string{
	// Tests that depended on typescript.d.ts in built.
	"APILibCheck.ts",
	"APISample_Watch.ts",
	"APISample_WatchWithDefaults.ts",
	"APISample_WatchWithOwnWatchHost.ts",
	"APISample_compile.ts",
	"APISample_jsdoc.ts",
	"APISample_linter.ts",
	"APISample_parseConfig.ts",
	"APISample_transform.ts",
	"APISample_watcher.ts",

	// These tests contain options that have been completely removed, so fail to parse.
	"preserveUnusedImports.ts",
	"noCrashWithVerbatimModuleSyntaxAndImportsNotUsedAsValues.ts",
	"verbatimModuleSyntaxCompat.ts",
	"verbatimModuleSyntaxCompat2.ts",
	"verbatimModuleSyntaxCompat3.ts",
	"verbatimModuleSyntaxCompat4.ts",
	"preserveValueImports.ts",
	"preserveValueImports_importsNotUsedAsValues.ts",
	"preserveValueImports_errors.ts",
	"preserveValueImports_mixedImports.ts",
	"preserveValueImports_module.ts",
	"importsNotUsedAsValues_error.ts",
	"alwaysStrictNoImplicitUseStrict.ts",
	"nonPrimitiveIndexingWithForInSupressError.ts",
	"parameterInitializerBeforeDestructuringEmit.ts",
	"mappedTypeUnionConstraintInferences.ts",
	"lateBoundConstraintTypeChecksCorrectly.ts",
	"keyofDoesntContainSymbols.ts",
	"noStrictGenericChecks.ts",
	"noImplicitUseStrict_umd.ts",
	"noImplicitUseStrict_system.ts",
	"noImplicitUseStrict_es6.ts",
	"noImplicitUseStrict_commonjs.ts",
	"noImplicitAnyIndexingSuppressed.ts",
	"excessPropertyErrorsSuppressed.ts",
	"moduleNoneDynamicImport.ts",
	"moduleNoneErrors.ts",
	"noErrorUsingImportExportModuleAugmentationInDeclarationFile1.ts",
	"noErrorUsingImportExportModuleAugmentationInDeclarationFile2.ts",
	"noErrorUsingImportExportModuleAugmentationInDeclarationFile3.ts",
	"requireOfJsonFileWithModuleEmitNone.ts",
	"requireOfJsonFileWithModuleNodeResolutionEmitNone.ts",
}

// confUnit — the Rust test_case_parser's TestUnit.
type confUnit struct {
	content string
	name    string
}

// confInputFile — the Rust tsbaseline's input-file pairing (unit name +
// content).
type confInputFile struct {
	unitName string
	content  string
}

// confView — the Rust tsbaseline's ErrorBaselineDiagnostic: a diagnostic
// paired with its file name and text ("" = global diagnostic).
type confView struct {
	fileName string
	fileText string
	d        *ast.Diagnostic
}

// confRecord — the twin's per-case NDJSON record.
type confRecord struct {
	caseName    string
	baseline    string
	verdict     string
	errorsCount int64
	shapeHash   string
	// dumpLines — the shape_hash fold lines (the --dump-filter bisect
	// read; NOT part of the NDJSON contract — mirrors CaseRecord::dump_lines).
	dumpLines []string
}

func (r confRecord) ndjson(sb *strings.Builder) {
	sb.Reset()
	sb.WriteString(`{"case":"`)
	jsonEscape(sb, r.caseName)
	sb.WriteString(`","baseline":"`)
	jsonEscape(sb, r.baseline)
	sb.WriteString(`","verdict":"`)
	sb.WriteString(r.verdict)
	sb.WriteString(`","errors_count":`)
	sb.WriteString(strconv.FormatInt(r.errorsCount, 10))
	sb.WriteString(`,"shape_hash":"`)
	sb.WriteString(r.shapeHash)
	sb.WriteString(`"}`)
}

// runConformance — the twin's entry (mirrors the Rust driver's
// conformance_seam.rs + tsc-testrunner's run_conformance).
func runConformance(args []string) {
	cli := flag.NewFlagSet("conformance", flag.ExitOnError)
	corpus := cli.String("corpus", "", "cases directory")
	baselines := cli.String("baselines", "", "baselines/reference directory")
	suite := cli.String("suite", "conformance", "suite subfolder")
	limit := cli.Int("limit", 0, "take the first N cases (0 = all)")
	filter := cli.String("filter", "", "-test.run style pattern")
	stage := cli.String("stage", "bind", "wired depth: parse|bind|check")
	localRoot := cli.String("local-root", "", "write local baselines here")
	dumpFilter := cli.String("dump-filter", "", "print the shape_hash fold lines of matching cases to stderr (the divergence bisect)")
	_ = cli.Parse(args)
	if *corpus == "" || *baselines == "" {
		fmt.Fprintln(os.Stderr, "go-bench conformance: missing required --corpus <dir> and --baselines <dir>")
		os.Exit(2)
	}
	stageName := ""
	switch *stage {
	case "parse":
		stageName = "parse"
	case "bind":
		stageName = "bind"
	case "check":
		stageName = "check"
	default:
		fmt.Fprintf(os.Stderr, "go-bench conformance: unknown stage %q (parse|bind|check)\n", *stage)
		os.Exit(2)
	}
	absCorpus, err := filepath.Abs(*corpus)
	if err != nil {
		fmt.Fprintf(os.Stderr, "go-bench: resolving corpus dir: %v\n", err)
		os.Exit(1)
	}
	absBaselines, err := filepath.Abs(*baselines)
	if err != nil {
		fmt.Fprintf(os.Stderr, "go-bench: resolving baselines dir: %v\n", err)
		os.Exit(1)
	}
	corpusDir := filepath.ToSlash(absCorpus)
	baselinesDir := filepath.ToSlash(absBaselines)
	var localDir string
	if *localRoot != "" {
		absLocal, err := filepath.Abs(*localRoot)
		if err != nil {
			fmt.Fprintf(os.Stderr, "go-bench: resolving local dir: %v\n", err)
			os.Exit(1)
		}
		localDir = filepath.ToSlash(absLocal)
	}

	files, err := confEnumerateCases(corpusDir, *filter, *limit)
	if err != nil {
		fmt.Fprintf(os.Stderr, "go-bench: %v\n", err)
		os.Exit(1)
	}

	var sb strings.Builder
	matched, mismatched, missing, panics, records := 0, 0, 0, 0, 0
	for _, file := range files {
		for _, record := range confRunCaseFile(corpusDir, baselinesDir, *suite, stageName, localDir, file) {
			record.ndjson(&sb)
			fmt.Println(sb.String())
			if *dumpFilter != "" && strings.Contains(record.caseName, *dumpFilter) {
				for _, line := range record.dumpLines {
					fmt.Fprintf(os.Stderr, "DUMP %s", line)
				}
			}
			records++
			switch record.verdict {
			case "match":
				matched++
			case "missing-baseline":
				missing++
			default:
				if record.errorsCount < 0 {
					panics++
				}
				mismatched++
			}
		}
	}
	pct := 0.0
	if records > 0 {
		pct = float64(matched) * 100.0 / float64(records)
	}
	fmt.Fprintf(os.Stderr, "conformance: %d cases, %d match (%.1f%%), %d mismatch, %d missing-baseline, %d panic\n",
		records, matched, pct, mismatched, missing, panics)
}

// ──────────────────────────────────────────────────────────────────────
// Case enumeration (mirrors enumerate_case_files + tsc-testutil's
// enumerate_files: recursive walk, \.tsx?$ on the full path, final
// full-path sort, the skip list, the filter, the limit)
// ──────────────────────────────────────────────────────────────────────

func confEnumerateCases(corpusDir string, filterPattern string, limit int) ([]string, error) {
	var files []string
	err := filepath.WalkDir(corpusDir, func(path string, entry fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if entry.IsDir() {
			return nil
		}
		slash := filepath.ToSlash(path)
		if strings.HasSuffix(slash, ".ts") || strings.HasSuffix(slash, ".tsx") {
			files = append(files, slash)
		}
		return nil
	})
	if err != nil {
		return nil, fmt.Errorf("walking cases: %w", err)
	}
	sort.Strings(files)
	filter := confCompilerTestFileFilter(filterPattern)
	var selected []string
	for _, file := range files {
		basename := tspath.GetBaseFileName(file)
		if confSliceContains(confSkippedTests, basename) {
			continue
		}
		if !filter(basename) {
			continue
		}
		selected = append(selected, file)
		if limit > 0 && len(selected) >= limit {
			break
		}
	}
	return selected, nil
}

func confSliceContains(list []string, name string) bool {
	for _, item := range list {
		if item == name {
			return true
		}
	}
	return false
}

// ──────────────────────────────────────────────────────────────────────
// The -test.run filter (mirrors tsc-testrunner's compiler_test_filter.rs —
// the structural RE2-subset port, NOT the original regexp consumer)
// ──────────────────────────────────────────────────────────────────────

func confCompilerTestFileFilter(pattern string) func(string) bool {
	acceptAll := func(string) bool { return true }
	if confNameNeedsRewrite(pattern) {
		return acceptAll
	}
	parts, ok := confSplitCompilerTestFilter(pattern)
	if !ok || len(parts) < 2 {
		return acceptAll
	}
	component := parts[1]
	anchored := strings.HasPrefix(component, "^")
	expression := strings.TrimPrefix(component, "^")
	prefix, valid := confLiteralPrefix(expression)
	if !valid {
		return acceptAll
	}
	if prefix == "" || confNameNeedsRewrite(prefix) || (!anchored && !strings.ContainsFunc(prefix, unicode.IsUpper)) {
		return acceptAll
	}
	return func(filename string) bool {
		if confNameNeedsRewrite(filename) {
			return true
		}
		if anchored {
			return strings.HasPrefix(filename, prefix) || strings.HasPrefix(prefix, filename+"_")
		}
		if strings.Contains(filename, prefix) {
			return true
		}
		for index := range filename {
			if strings.HasPrefix(prefix, filename[index:]+"_") {
				return true
			}
		}
		return false
	}
}

// confNameNeedsRewrite — whitespace or non-printable characters.
func confNameNeedsRewrite(name string) bool {
	return strings.ContainsFunc(name, func(r rune) bool {
		return unicode.IsSpace(r) || !confIsPrint(r)
	})
}

// confIsPrint — strconv.IsPrint equivalent for the filter's discrimination
// set (Unicode graphic + ASCII space).
func confIsPrint(r rune) bool {
	if r == ' ' {
		return true
	}
	return !unicode.IsSpace(r) && !unicode.IsControl(r)
}

// confSplitCompilerTestFilter — split on '/' at depth 0 outside classes; a
// top-level '|' makes the whole split nil.
func confSplitCompilerTestFilter(pattern string) ([]string, bool) {
	var parts []string
	start, depth := 0, 0
	inClass, escaped := false, false
	for index, character := range pattern {
		if escaped {
			escaped = false
			continue
		}
		if character == '\\' {
			escaped = true
			continue
		}
		if inClass {
			if character == ']' {
				inClass = false
			}
			continue
		}
		switch character {
		case '[':
			inClass = true
		case '(':
			depth++
		case ')':
			depth--
		case '|':
			if depth == 0 {
				return nil, false
			}
		case '/':
			if depth == 0 {
				parts = append(parts, pattern[start:index])
				start = index + 1
			}
		}
	}
	return append(parts, pattern[start:]), true
}

// confLiteralPrefix — the structural literal prefix (mirrors the Rust
// port's literal_prefix, including Go regexp's whole-group transparency).
func confLiteralPrefix(expression string) (string, bool) {
	if strings.HasPrefix(expression, "(") && !strings.HasPrefix(expression[1:], "?") {
		close := confMatchingGroupClose(expression)
		if close < 0 {
			return "", false
		}
		if close == len(expression)-1 {
			return confLiteralPrefix(expression[1:close])
		}
		// the group is part of a larger expression — the main scan below
		// breaks on the '(' immediately (see the Rust port's note).
	}
	var prefix strings.Builder
	runes := []rune(expression)
	i := 0
	for i < len(runes) {
		c := runes[i]
		switch {
		case c == '\\':
			if i+1 >= len(runes) {
				return "", false // trailing backslash — invalid
			}
			next := runes[i+1]
			switch next {
			case 'x':
				decoded, width, ok := confDecodeHexEscape(string(runes[i+2:]))
				if !ok {
					return "", false
				}
				prefix.WriteRune(decoded)
				i += 2 + width
			case 't':
				prefix.WriteByte('\t')
				i += 2
			case 'n':
				prefix.WriteByte('\n')
				i += 2
			case 'r':
				prefix.WriteByte('\r')
				i += 2
			case 'f':
				prefix.WriteByte('\f')
				i += 2
			case 'v':
				prefix.WriteByte('\v')
				i += 2
			case 'a':
				prefix.WriteByte('\a')
				i += 2
			default:
				if !unicode.IsLetter(next) && !unicode.IsDigit(next) {
					prefix.WriteRune(next)
					i += 2
				} else {
					// character classes / anchors / backrefs — prefix ends
					return "", false
				}
			}
		case c == '|':
			// top-level alternation: no common literal prefix
			return "", true
		case c == '(' || c == ')' || c == '[' || c == ']' || c == '{' || c == '}' ||
			c == '?' || c == '*' || c == '+' || c == '^' || c == '$' || c == '.':
			// metacharacter — the prefix ends before it
			i = len(runes)
		default:
			prefix.WriteRune(c)
			i++
		}
	}
	if !confStructurallyValid(expression) {
		return "", false
	}
	return prefix.String(), true
}

func confMatchingGroupClose(expression string) int {
	depth, inClass, escaped := 0, false, false
	for index, c := range expression {
		if escaped {
			escaped = false
			continue
		}
		if c == '\\' {
			escaped = true
			continue
		}
		if inClass {
			if c == ']' {
				inClass = false
			}
			continue
		}
		switch c {
		case '[':
			inClass = true
		case '(':
			depth++
		case ')':
			depth--
			if depth == 0 {
				return index
			}
		}
	}
	return -1
}

func confDecodeHexEscape(after string) (rune, int, bool) {
	if strings.HasPrefix(after, "{") {
		end := strings.Index(after, "}")
		if end < 0 {
			return 0, 0, false
		}
		value, err := strconv.ParseUint(after[1:end], 16, 32)
		if err != nil {
			return 0, 0, false
		}
		return rune(value), end + 2, true
	}
	if len(after) < 2 {
		return 0, 0, false
	}
	value, err := strconv.ParseUint(after[:2], 16, 32)
	if err != nil {
		return 0, 0, false
	}
	return rune(value), 2, true
}

func confStructurallyValid(expression string) bool {
	depth, inClass, escaped := 0, false, false
	for _, c := range expression {
		if escaped {
			escaped = false
			continue
		}
		if c == '\\' {
			escaped = true
			continue
		}
		if inClass {
			if c == ']' {
				inClass = false
			}
			continue
		}
		switch c {
		case '[':
			inClass = true
		case '(':
			depth++
		case ')':
			depth--
			if depth < 0 {
				return false
			}
		}
	}
	return depth == 0 && !inClass && !escaped
}

// ──────────────────────────────────────────────────────────────────────
// The @filename / @option directive machinery (mirrors
// tsc-testrunner's test_case_parser.rs — the line-based port)
// ──────────────────────────────────────────────────────────────────────

var confFourslashDirectives = []string{"emitthisfile", "noopen"}

func confSplitLineDelimited(text string) []string {
	lines := strings.Split(text, "\n")
	for i, line := range lines {
		lines[i] = strings.TrimSuffix(line, "\r")
	}
	return lines
}

// confFindOptionDirective — `^\/{2}\s*@(\w+)\s*:\s*([^\r\n]*)` per line.
func confFindOptionDirective(line string) (name, value string, ok bool) {
	afterSlashes, ok := strings.CutPrefix(line, "//")
	if !ok {
		return "", "", false
	}
	afterWs := strings.TrimLeft(afterSlashes, "\t\v\f\r ")
	afterAt, ok := strings.CutPrefix(afterWs, "@")
	if !ok {
		return "", "", false
	}
	nameLen := 0
	for nameLen < len(afterAt) {
		c := afterAt[nameLen]
		if c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9' || c == '_' {
			nameLen++
		} else {
			break
		}
	}
	if nameLen == 0 {
		return "", "", false
	}
	name, rest := afterAt[:nameLen], afterAt[nameLen:]
	rest = strings.TrimLeft(rest, "\t\v\f\r ")
	rest, ok = strings.CutPrefix(rest, ":")
	if !ok {
		return "", "", false
	}
	rest = strings.TrimLeft(rest, "\t\v\f\r ")
	return name, rest, true
}

// confFindLinkDirective — `^\/{2}\s*@link\s*:\s*([^\r\n]*)\s*->\s*([^\r\n]*)`
// → (original, symlink).
func confFindLinkDirective(line string) (original, symlink string, ok bool) {
	afterSlashes, ok := strings.CutPrefix(line, "//")
	if !ok {
		return "", "", false
	}
	afterWs := strings.TrimLeft(afterSlashes, "\t\v\f\r ")
	afterAt, ok := strings.CutPrefix(afterWs, "@")
	if !ok {
		return "", "", false
	}
	rest, ok := strings.CutPrefix(afterAt, "link")
	if !ok {
		return "", "", false
	}
	rest = strings.TrimLeft(rest, "\t\v\f\r ")
	rest, ok = strings.CutPrefix(rest, ":")
	if !ok {
		return "", "", false
	}
	rest = strings.TrimLeft(rest, "\t\v\f\r ")
	sep := strings.LastIndex(rest, "->")
	if sep < 0 {
		return "", "", false
	}
	return strings.TrimRight(rest[:sep], "\t\v\f\r "), strings.TrimLeft(rest[sep+2:], "\t\v\f\r "), true
}

// confAllowImplicitFirstFile — the compiler runner's parse options: the
// implicit-first-file gate is OFF (the fourslash harness's form; kept as a
// mirror of the Rust port's ParseTestFilesOptions default).
const confAllowImplicitFirstFile = false

// confParseTestFilesAndSymlinks — mirrors the Rust port's concrete form.
func confParseTestFilesAndSymlinks(code string, fileName string) (units []confUnit, symlinks map[string]string, currentDirectory string, globalOptions map[string]string) {
	units = nil
	symlinks = map[string]string{}
	globalOptions = map[string]string{}
	var currentFileContent strings.Builder
	var currentFileName string
	seenContentLine, hasSeenFile := false, false
	currentFileOptions := map[string]string{}

	for _, line := range confSplitLineDelimited(code) {
		if confParseSymlinkFromTest(line, symlinks) {
			continue
		}
		metaDataName, metaDataRaw, isOption := confFindOptionDirective(line)
		if isOption {
			metaDataName = strings.ToLower(metaDataName)
			metaDataValue := strings.TrimSpace(metaDataRaw)
			if metaDataName == "currentdirectory" {
				currentDirectory = metaDataValue
			}
			if metaDataName != "filename" {
				if metaDataName == "symlink" && currentFileName != "" {
					for _, link := range strings.Split(metaDataValue, ",") {
						link = strings.TrimSpace(link)
						if link != "" {
							symlinks[link] = currentFileName
						}
					}
				} else if confSliceContains(confFourslashDirectives, metaDataName) {
					currentFileOptions[metaDataName] = metaDataValue
				} else {
					globalOptions[metaDataName] = metaDataValue
				}
				continue
			}
			if currentFileName != "" {
				shouldSaveFile := !confAllowImplicitFirstFile || currentFileContent.Len() != 0 || hasSeenFile
				if shouldSaveFile {
					hasSeenFile = true
					units = append(units, confUnit{content: currentFileContent.String(), name: currentFileName})
				}
				currentFileContent.Reset()
				seenContentLine = false
				currentFileName = metaDataValue
				currentFileOptions = map[string]string{}
			} else {
				hasContentBeforeFirstFilename := currentFileContent.Len() != 0 &&
					scanner.SkipTrivia(currentFileContent.String(), 0) != currentFileContent.Len()
				if hasContentBeforeFirstFilename && !confAllowImplicitFirstFile {
					panic("Non-comment test content appears before the first '// @Filename' directive")
				}
				currentFileContent.Reset()
				seenContentLine = false
				currentFileName = strings.TrimSpace(metaDataRaw)
				currentFileOptions = map[string]string{}
			}
		} else {
			// fourslash tests preserve leading blank lines via
			// seenContentLine; compiler tests drop them (the Len() != 0
			// form) — the Rust port's exact branches.
			if confAllowImplicitFirstFile {
				if seenContentLine {
					currentFileContent.WriteByte('\n')
				}
				seenContentLine = true
			} else if currentFileContent.Len() != 0 {
				currentFileContent.WriteByte('\n')
			}
			currentFileContent.WriteString(line)
		}
	}

	if len(units) == 0 && currentFileName == "" {
		currentFileName = tspath.GetBaseFileName(fileName)
	}
	units = append(units, confUnit{content: currentFileContent.String(), name: currentFileName})
	return units, symlinks, currentDirectory, globalOptions
}

func confParseSymlinkFromTest(line string, symlinks map[string]string) bool {
	original, symlink, ok := confFindLinkDirective(line)
	if !ok {
		return false
	}
	symlinks[strings.TrimSpace(symlink)] = strings.TrimSpace(original)
	return true
}

// confExtractCompilerSettings — every option directive, value trimmed and
// ';' stripped.
func confExtractCompilerSettings(content string) map[string]string {
	opts := map[string]string{}
	for _, line := range confSplitLineDelimited(content) {
		name, raw, ok := confFindOptionDirective(line)
		if !ok {
			continue
		}
		value := strings.TrimSpace(raw)
		opts[strings.ToLower(name)] = strings.TrimSuffix(value, ";")
	}
	return opts
}

// ──────────────────────────────────────────────────────────────────────
// The configuration expansion (mirrors tsc-testutil's harness.rs)
// ──────────────────────────────────────────────────────────────────────

type confNamedConfiguration struct {
	name   string
	config map[string]string
}

// confGetFileBasedTestConfigurations — sorted-key iteration everywhere
// (Go's map order is random; the twin sorts — the port's rule).
func confGetFileBasedTestConfigurations(settings map[string]string) []confNamedConfiguration {
	keys := make([]string, 0, len(settings))
	for key := range settings {
		keys = append(keys, key)
	}
	sort.Strings(keys)

	var optionEntries [][2]interface{} // [option name, []values]
	optionEntries = nil
	nonVaryingOptions := map[string]string{}
	for _, option := range keys {
		value := settings[option]
		if confVaryBy(option) {
			entries := confSplitOptionValues(value, option)
			if len(entries) > 1 {
				optionEntries = append(optionEntries, [2]interface{}{option, entries})
			} else if len(entries) == 1 {
				nonVaryingOptions[option] = entries[0]
			}
		} else {
			nonVaryingOptions[option] = value
		}
	}

	var configurations []confNamedConfiguration
	if len(optionEntries) > 0 {
		variations := []map[string]string{}
		confComputeVariations(optionEntries, 0, map[string]string{}, &variations)
		for _, varyingConfig := range variations {
			description := confConfigurationDescription(varyingConfig)
			for key, value := range nonVaryingOptions {
				varyingConfig[key] = value
			}
			configurations = append(configurations, confNamedConfiguration{description, varyingConfig})
		}
	} else if len(nonVaryingOptions) > 0 {
		configurations = append(configurations, confNamedConfiguration{"", nonVaryingOptions})
	}
	return configurations
}

func confComputeVariations(optionEntries [][2]interface{}, index int, state map[string]string, out *[]map[string]string) {
	if index >= len(optionEntries) {
		clone := make(map[string]string, len(state))
		for key, value := range state {
			clone[key] = value
		}
		*out = append(*out, clone)
		return
	}
	optionKey := optionEntries[index][0].(string)
	entries := optionEntries[index][1].([]string)
	for _, entry := range entries {
		state[optionKey] = entry
		confComputeVariations(optionEntries, index+1, state, out)
	}
}

func confConfigurationDescription(config map[string]string) string {
	keys := make([]string, 0, len(config))
	for key := range config {
		keys = append(keys, key)
	}
	sort.Strings(keys)
	parts := make([]string, len(keys))
	for i, key := range keys {
		parts[i] = key + "=" + strings.ToLower(config[key])
	}
	return strings.Join(parts, ",")
}

func confVaryBy(name string) bool {
	for _, entry := range optionTable {
		if entry.directive == name {
			return true
		}
	}
	return false
}

// confSplitOptionValues — the simplified literal form (see the harness.rs
// PORT note: `*` stays a literal, `-`/`!` are literal exclusions).
func confSplitOptionValues(value string, option string) []string {
	if len(value) == 0 {
		return nil
	}
	star := false
	var includes, excludes []string
	for _, s := range strings.Split(value, ",") {
		s = strings.TrimSpace(s)
		if len(s) == 0 {
			continue
		}
		if s == "*" {
			star = true
		} else if strings.HasPrefix(s, "-") {
			excludes = append(excludes, s[1:])
		} else if strings.HasPrefix(s, "!") {
			excludes = append(excludes, s[1:])
		} else {
			includes = append(includes, s)
		}
	}
	if len(includes) == 0 && !star && len(excludes) == 0 {
		return nil
	}
	if star {
		includes = append([]string{"*"}, includes...)
	}
	var seen []string
	for _, include := range includes {
		if !confSliceContains(excludes, include) && !confSliceContains(seen, include) {
			seen = append(seen, include)
		}
	}
	if len(seen) == 0 {
		panic(fmt.Sprintf("Variations in test option '@%s' resulted in an empty set.", option))
	}
	return seen
}

// ──────────────────────────────────────────────────────────────────────
// The per-case pipeline (mirrors tsc-testrunner's compiler_runner.rs)
// ──────────────────────────────────────────────────────────────────────

func confRunCaseFile(corpusDir, baselinesDir, suite, stage, localDir, file string) []confRecord {
	data, err := os.ReadFile(file)
	if err != nil {
		panic("Could not read test file: " + file + ": " + err.Error())
	}
	content := string(data)
	basename := tspath.GetBaseFileName(file)
	settings := confExtractCompilerSettings(content)

	configurations := confGetFileBasedTestConfigurations(settings)
	sort.Slice(configurations, func(i, j int) bool { return configurations[i].name < configurations[j].name })
	if len(configurations) == 0 {
		configurations = []confNamedConfiguration{{"", map[string]string{}}}
	}

	units, symlinks, _, _ := confParseTestFilesAndSymlinks(content, file)
	caseName := strings.ReplaceAll(strings.TrimPrefix(filepath.ToSlash(file), corpusDir+"/"), "\\", "/")

	var records []confRecord
	for _, named := range configurations {
		baselineName := confBaselineFileName(basename, named.name)
		baseline := suite + "/" + baselineName
		record, panicked := confRunConfiguration(corpusDir, baselinesDir, suite, stage, localDir, caseName, baselineName, basename, baseline, units, symlinks, named.config)
		if panicked {
			record = confRecord{caseName: caseName, baseline: baseline, verdict: "mismatch", errorsCount: -1}
		}
		records = append(records, record)
	}
	return records
}

// confRunConfiguration — the depth-controlled pipeline with the panic
// containment (the -1 sentinel record).
func confRunConfiguration(corpusDir, baselinesDir, suite, stage, localDir, caseName, baselineName, basename, baseline string, units []confUnit, symlinks map[string]string, harnessConfig map[string]string) (record confRecord, panicked bool) {
	defer func() {
		if r := recover(); r != nil {
			// The panic message goes to STDERR only (stdout stays the pure
			// NDJSON contract) — the Rust twin prints the message too.
			fmt.Fprintf(os.Stderr, "go-bench conformance: panic on case %s (%s): %v\n%s\n",
				caseName, baselineName, r, debug.Stack())
			panicked = true
		}
	}()

	srcFolder := tspath.RootedDirectoryPathFromNormalized(confSrcFolder)
	currentDirectory := srcFolder
	if raw, ok := harnessConfig["currentdirectory"]; ok && raw != "" {
		currentDirectory = tspath.ToRootedDirectoryPath(raw, srcFolder)
	}

	rootedNames := make([]string, len(units))
	for i, unit := range units {
		rootedNames[i] = tspath.ToRootedFilePath(unit.name, currentDirectory).AsString()
	}

	tsConfigUnit := -1
	for i, unit := range units {
		if confGetConfigNameFromFileName(unit.name) != "" {
			tsConfigUnit = i
			break
		}
	}

	noImplicitReferences := false
	if value, ok := harnessConfig["noimplicitreferences"]; ok && value != "" {
		noImplicitReferences = true
	}
	last := len(units) - 1
	referencesLast := noImplicitReferences ||
		strings.Contains(units[last].content, "require(") ||
		confContainsReferencePath(units[last].content)
	var rootIndexes, otherIndexes []int
	if referencesLast {
		rootIndexes = []int{last}
		for i := 0; i < last; i++ {
			otherIndexes = append(otherIndexes, i)
		}
	} else {
		for i := range units {
			rootIndexes = append(rootIndexes, i)
		}
	}

	var configPath tspath.RootedFilePath
	var configText strings.Builder
	if tsConfigUnit >= 0 {
		configPath = tspath.RootedFilePathFromNormalized(rootedNames[tsConfigUnit])
		configText.WriteString(units[tsConfigUnit].content)
	} else {
		configPath = tspath.ToRootedFilePath(confConfigUnit, currentDirectory)
		configText.WriteString(`{"compilerOptions":{`)
		confMarshalCompilerOptions(harnessConfig, &configText)
		configText.WriteString(`},"files":[`)
		first := true
		for _, index := range rootIndexes {
			ext := confAnyExtension(units[index].name)
			if ext == ".json" || ext == ".tsbuildinfo" {
				continue
			}
			if !first {
				configText.WriteByte(',')
			}
			first = false
			configText.WriteByte('"')
			jsonEscape(&configText, rootedNames[index])
			configText.WriteByte('"')
		}
		configText.WriteString(`]}`)
	}

	entries := make(map[string]string, len(units)+1+len(symlinks))
	for i, unit := range units {
		entries[rootedNames[i]] = unit.content
	}
	configTextStr := configText.String()
	entries[configPath.AsString()] = configTextStr
	// symlinks seeded as file copies of their target unit (the shared
	// depth-controlled rule — see the Rust port's comment)
	for link, target := range symlinks {
		linkPath := tspath.ToRootedFilePath(link, currentDirectory).AsString()
		targetPath := tspath.ToRootedFilePath(target, currentDirectory).AsString()
		if content, ok := entries[targetPath]; ok {
			entries[linkPath] = content
		}
	}
	fsys := bundled.WrapFS(vfstest.FromMap(entries, tspath.CaseSensitive))

	config, _ := tsoptions.GetParsedCommandLineOfConfigFile(configPath, nil, nil, fsys, nil)
	host := compiler.NewCompilerHost(fsys, bundled.LibPath(), nil, nil, nil)
	program := compiler.NewProgram(compiler.ProgramOptions{
		ProgramConfig: compiler.ProgramConfig{Config: config},
		ProgramHosts:  compiler.ProgramHosts{Host: host},
	})
	if stage == "bind" || stage == "check" {
		// The Rust twin binds for every depth past parse
		// (compiler_runner.rs: config.stage != Stage::Parse).
		program.BindSourceFiles()
	}

	// roots vs others for a tsconfig unit: the program's own file list
	if tsConfigUnit >= 0 {
		fileNames := make(map[string]bool)
		for _, f := range program.GetSourceFiles() {
			fileNames[f.FileName().AsString()] = true
		}
		rootIndexes, otherIndexes = nil, nil
		for i := range units {
			if i == tsConfigUnit {
				continue
			}
			if fileNames[rootedNames[i]] {
				rootIndexes = append(rootIndexes, i)
			} else {
				otherIndexes = append(otherIndexes, i)
			}
		}
	}

	// diagnostics at the wired depth: tsconfig-parse + per-file parse +
	// program diagnostics (Go resolves the file through the pointer graph)
	var views []confView
	for _, d := range program.GetConfigFileParsingDiagnostics() {
		views = append(views, confViewForDiagnostic(d, configPath, configTextStr))
	}
	for _, f := range program.GetSourceFiles() {
		for _, d := range f.Diagnostics() {
			views = append(views, confView{fileName: f.FileName().AsString(), fileText: f.Text(), d: d})
		}
		for _, d := range f.JSDiagnostics() {
			views = append(views, confView{fileName: f.FileName().AsString(), fileText: f.Text(), d: d})
		}
	}
	for _, d := range program.GetProgramDiagnostics() {
		views = append(views, confViewForDiagnostic(d, configPath, configTextStr))
	}
	// ── The W3 check stage: the semantic layer ──
	// The Rust runner's per-unit seam (compiler_runner.rs
	// `semantic_diagnostics_of_unit`): every unit that is a TS source (the
	// in-test tsconfig unit is the config, not a source; .json/.tsbuildinfo
	// units are not TS) is checked through a single-file checker over its
	// own bytes with the case's parsed options. The Go twin mirrors that
	// construction: a per-unit single-file Program (the map FS holds only
	// the unit — same as the Rust checker, which parses no other unit and
	// no lib), built from the SAME parsed command line with the file list
	// swapped (`WithFileNames`), then the REAL Go checker's read
	// (checker.GetDiagnostics + GetGlobalDiagnostics — the Rust
	// `semantic_diagnostics()` returns the whole diagnostics collection,
	// file-paired and global entries).
	//
	// Known divergences vs the Rust seam (CONFORMANCE.md, check-stage
	// bisect list): Go's exported GetDiagnostics computes `checkUnused`
	// from NoUnusedLocals/NoUnusedParameters where the Rust seam passes
	// false — noUnused* configurations bisect separately.
	if stage == "check" {
		for i, unit := range units {
			if tsConfigUnit >= 0 && i == tsConfigUnit {
				continue
			}
			ext := confAnyExtension(unit.name)
			if ext == ".json" || ext == ".tsbuildinfo" {
				continue
			}
			for _, d := range confSemanticDiagnosticsOfUnit(rootedNames[i], unit.content, config) {
				views = append(views, confView{fileName: rootedNames[i], fileText: unit.content, d: d})
			}
		}
	}
	errorsCount := int64(len(views))

	// input files in Go's order: tsconfig, to-be-compiled, others
	var inputFiles []confInputFile
	if tsConfigUnit >= 0 {
		inputFiles = append(inputFiles, confInputFile{rootedNames[tsConfigUnit], units[tsConfigUnit].content})
	}
	for _, index := range rootIndexes {
		inputFiles = append(inputFiles, confInputFile{rootedNames[index], units[index].content})
	}
	for _, index := range otherIndexes {
		inputFiles = append(inputFiles, confInputFile{rootedNames[index], units[index].content})
	}

	// the .errors.txt content (the ported emitter below)
	var baselineContent string
	if len(views) == 0 {
		baselineContent = confNoContent
	} else {
		baselineContent = confGetErrorBaseline(inputFiles, views)
	}

	// the verdict (Go writeComparison's semantics, mapped onto the twin)
	referencePath := filepath.Join(baselinesDir, suite, baselineName)
	verdict := "mismatch"
	if baselineContent == confNoContent {
		if _, err := os.Stat(referencePath); os.IsNotExist(err) {
			verdict = "match"
		}
	} else if _, err := os.Stat(referencePath); os.IsNotExist(err) {
		verdict = "missing-baseline"
	} else if expected, err := os.ReadFile(referencePath); err == nil && string(expected) == baselineContent {
		verdict = "match"
	}

	// the local-baseline write (on demand — the ported golden-file
	// machinery; the Go twin mirrors write_comparison's writes)
	if localDir != "" {
		confWriteLocalBaseline(localDir, baselinesDir, suite, baselineName, baselineContent)
	}

	// the shape_hash fold lines (the twin's recipe)
	sorted := make([]confView, len(views))
	copy(sorted, views)
	sort.SliceStable(sorted, func(i, j int) bool { return confCompareViews(&sorted[i], &sorted[j]) < 0 })
	h := uint64(0xcbf29ce484222325)
	fold := func(s string) {
		for i := 0; i < len(s); i++ {
			h ^= uint64(s[i])
			h *= 1099511628211
		}
	}
	dumpLines := make([]string, 0, len(sorted)+3)
	line := func(s string) {
		fold(s)
		dumpLines = append(dumpLines, s)
	}
	line("case=" + caseName + "\n")
	line("baseline=" + suite + "/" + baselineName + "\n")
	line("errors=" + strconv.FormatInt(errorsCount, 10) + "\n")
	for _, view := range sorted {
		line(fmt.Sprintf("diag=%s|%s|%d|%d|%d|%s\n",
			view.fileName, string(view.d.MessageKey()), view.d.Code(), view.d.Pos(), view.d.End(),
			strings.Join(view.d.MessageArgs(), "\x01")))
	}

	return confRecord{
		caseName:    caseName,
		baseline:    baseline,
		verdict:     verdict,
		errorsCount: errorsCount,
		shapeHash:   fmt.Sprintf("%016x", h),
		dumpLines:   dumpLines,
	}, false
}

// confSemanticDiagnosticsOfUnit — the Rust runner's
// `semantic_diagnostics_of_unit` twin: a per-unit single-file program (the
// map FS holds only the unit; no default lib — the Rust unit checker parses
// neither other units nor lib files) over the case's parsed command line
// with the file list swapped, then the real Go checker over it. The read
// mirrors the Rust seam's `semantic_diagnostics()` (the whole collection:
// per-file GetDiagnostics + the global entries).
//
// W3 tranche 3: the special-global-types error arm
// (getGlobalType/getGlobalValue with reportErrors=true — TS2316/TS2317/
// TS2318/TS2468) is NO LONGER filtered. The Rust port's getGlobalSymbol is
// real (resolveName over the global scope), so both drivers emit the arm.
func confSemanticDiagnosticsOfUnit(unitName, content string, config *tsoptions.ParsedCommandLine) []*ast.Diagnostic {
	// The Rust seam bails on JSON sources (ensure_script_kind's JSON arm —
	// the caller's extension filter skips .json/.tsbuildinfo already; the
	// guard here is the belt to the same braces).
	switch core.GetScriptKindFromFileName(tspath.RootedFilePathFromNormalized(unitName)) {
	case core.ScriptKindJSON, core.ScriptKindUnknown:
		return nil
	}
	entries := map[string]string{unitName: content}
	fsys := bundled.WrapFS(vfstest.FromMap(entries, tspath.CaseSensitive))
	// The default-library path points at the map FS's root, which holds no
	// lib files: the Rust unit program carries no lib files (the checker's
	// globals come from the checker crate alone). An empty library path
	// panics Go's path resolver, so "/" is the no-libs sentinel.
	host := compiler.NewCompilerHost(fsys, tspath.RootedDirectoryPathFromNormalized("/"), nil, nil, nil)
	unitConfig := config.WithFileNames([]tspath.RootedFilePath{tspath.RootedFilePathFromNormalized(unitName)})
	program := compiler.NewProgram(compiler.ProgramOptions{
		ProgramConfig: compiler.ProgramConfig{Config: unitConfig},
		ProgramHosts:  compiler.ProgramHosts{Host: host},
	})
	files := program.GetSourceFiles()
	if len(files) == 0 {
		return nil
	}
	fileChecker, _ := checker.NewChecker(program, nil)
	diags := fileChecker.GetDiagnostics(context.Background(), files[0])
	diags = append(diags, fileChecker.GetGlobalDiagnostics()...)
	// W3T7 PROBE (temporary): print each diagnostic's pos/end + the text
	// slice, gated on W3T7PROBE (2703 span divergence bisect).
	if os.Getenv("W3T7PROBE") != "" && strings.Contains(unitName, "await_unary") {
		for _, d := range diags {
			pos, end := int(d.Pos()), int(d.End())
			lo, hi := pos, end
			if lo > len(content) {
				lo = len(content)
			}
			if hi > len(content) {
				hi = len(content)
			}
			fmt.Fprintf(os.Stderr, "w3t7probe-go file=%s code=%d pos=%d end=%d text=%q\n",
				unitName, d.Code(), pos, end, content[lo:hi])
		}
	}
	return diags
}

// confViewForDiagnostic — resolve the (file name, file text) a diagnostic
// renders against (the program's copy, the config text, or "" global).
func confViewForDiagnostic(d *ast.Diagnostic, configPath tspath.RootedFilePath, configText string) confView {
	if d.File() == nil {
		return confView{fileName: "", fileText: "", d: d}
	}
	fileName := d.File().FileName().AsString()
	if d.File().Text() != "" {
		return confView{fileName: fileName, fileText: d.File().Text(), d: d}
	}
	if fileName == configPath.AsString() {
		return confView{fileName: fileName, fileText: configText, d: d}
	}
	return confView{fileName: fileName, fileText: "", d: d}
}

// confWriteLocalBaseline — write_comparison's writes (new/changed/delete).
func confWriteLocalBaseline(localDir, baselinesDir, suite, baselineName, actual string) {
	localPath := filepath.Join(localDir, suite, baselineName)
	referencePath := filepath.Join(baselinesDir, suite, baselineName)
	if actual == confNoContent {
		if _, err := os.Stat(referencePath); err == nil {
			_ = os.MkdirAll(filepath.Dir(localPath), 0o755)
			_ = os.WriteFile(localPath+".delete", []byte{}, 0o644)
		}
		return
	}
	_ = os.MkdirAll(filepath.Dir(localPath), 0o755)
	if expected, err := os.ReadFile(referencePath); err != nil || string(expected) != actual {
		_ = os.WriteFile(localPath, []byte(actual), 0o644)
	}
}

// confGetConfigNameFromFileName — the tsconfig/jsconfig unit detector.
func confGetConfigNameFromFileName(fileName string) string {
	basenameLower := strings.ToLower(tspath.GetBaseFileName(fileName))
	if basenameLower == "tsconfig.json" || basenameLower == "jsconfig.json" {
		return basenameLower
	}
	return ""
}

// confContainsReferencePath — Go's `reference\spath` regex.
func confContainsReferencePath(content string) bool {
	index := 0
	for {
		found := strings.Index(content[index:], "reference")
		if found < 0 {
			return false
		}
		at := index + found + len("reference")
		if at < len(content) && unicode.IsSpace(rune(content[at])) && strings.HasPrefix(content[at+1:], "path") {
			return true
		}
		index = at
	}
}

// confAnyExtension — the last ".xxx" suffix ("" when none).
func confAnyExtension(name string) string {
	index := strings.LastIndex(name, ".")
	if index < 0 {
		return ""
	}
	return name[index:]
}

// confConfiguredName — `base(ext)(desc)ext`.
func confConfiguredName(basename, configurationName string) string {
	if configurationName == "" {
		return basename
	}
	ext := confAnyExtension(basename)
	extensionless := basename[:len(basename)-len(ext)]
	return fmt.Sprintf("%s(%s)%s", extensionless, configurationName, ext)
}

// confBaselineFileName — `\.tsx?$` → ".errors.txt".
func confBaselineFileName(basename, configurationName string) string {
	configured := confConfiguredName(basename, configurationName)
	trimmed := strings.TrimSuffix(configured, ".tsx")
	if trimmed == configured {
		trimmed = strings.TrimSuffix(configured, ".ts")
	}
	return trimmed + ".errors.txt"
}

// confMarshalCompilerOptions — the table + rules (sorted directive keys).
func confMarshalCompilerOptions(config map[string]string, out *strings.Builder) {
	keys := make([]string, 0, len(config))
	for key := range config {
		keys = append(keys, key)
	}
	sort.Strings(keys)
	first := true
	for _, key := range keys {
		if confHarnessOptions[key] {
			continue
		}
		var entry *optEntry
		for i := range optionTable {
			if optionTable[i].directive == key {
				entry = &optionTable[i]
				break
			}
		}
		if entry == nil {
			continue
		}
		if !first {
			out.WriteByte(',')
		}
		first = false
		out.WriteByte('"')
		out.WriteString(entry.tsconfigName)
		out.WriteString(`":`)
		value := strings.TrimSpace(config[key])
		switch entry.kind {
		case kindBoolean:
			switch strings.ToLower(value) {
			case "true":
				out.WriteString("true")
			case "false":
				out.WriteString("false")
			default:
				out.WriteByte('"')
				jsonEscape(out, value)
				out.WriteByte('"')
			}
		case kindEnum:
			out.WriteByte('"')
			jsonEscape(out, value)
			out.WriteByte('"')
		}
	}
}

// ──────────────────────────────────────────────────────────────────────
// The .errors.txt emitter (mirrors tsc-testutil's tsbaseline.rs — the
// ported emitter with the twin's PORT deviations)
// ──────────────────────────────────────────────────────────────────────

var confTestPathPrefixReplacer = strings.NewReplacer(
	"/.ts/", "",
	"/.lib/", "",
	"/.src/", "",
	"bundled:///libs/", "",
	"file:///./ts/", "file:///",
	"file:///./lib/", "file:///",
	"file:///./src/", "file:///",
)

func confRemoveTestPathPrefixes(text string) string {
	return confTestPathPrefixReplacer.Replace(text)
}

func confIsDefaultLibraryFile(filePath string) bool {
	fileName := tspath.GetBaseFileName(filePath)
	return strings.HasPrefix(fileName, "lib.") && strings.HasSuffix(fileName, ".d.ts")
}

// confLocalizeDiagnostic — Go's Diagnostic.Localize(locale.Default).
func confLocalizeDiagnostic(d *ast.Diagnostic) string {
	return d.Localize(locale.Default)
}

// confFlattenDiagnosticMessage — the chain walk (Go's
// WriteFlattenedDiagnosticMessage).
func confFlattenDiagnosticMessage(d *ast.Diagnostic) string {
	var out strings.Builder
	out.WriteString(confLocalizeDiagnostic(d))
	for _, chain := range d.MessageChain() {
		confFlattenChain(&out, chain, 1)
	}
	return out.String()
}

func confFlattenChain(out *strings.Builder, chain *ast.Diagnostic, level int) {
	out.WriteString(confNewline)
	for i := 0; i < level; i++ {
		out.WriteString("  ")
	}
	out.WriteString(confLocalizeDiagnostic(chain))
	for _, child := range chain.MessageChain() {
		confFlattenChain(out, child, level+1)
	}
}

// confDiagnosticPrefix — the diagnostic's source or "TS".
func confDiagnosticPrefix(d *ast.Diagnostic) string {
	if source := d.Source(); source != "" {
		return source
	}
	return "TS"
}

// confMessageIdentity — Go's getDiagnosticMessageIdentity (the
// message!=nil && code==-1 arm has no Go accessor here; parse-level
// diagnostics never carry a real message at code -1, so the arm is dead —
// PORT-noted in CONFORMANCE.md).
func confMessageIdentity(d *ast.Diagnostic) string {
	if d.MessageText() != "" {
		return d.MessageText()
	}
	return string(d.MessageKey())
}

// confCompareViews — Go's CompareDiagnostics over the view pairing (path,
// pos, end, code, category, source, identity, args).
func confCompareViews(a, b *confView) int {
	if c := strings.Compare(a.fileName, b.fileName); c != 0 {
		return c
	}
	if c := a.d.Pos() - b.d.Pos(); c != 0 {
		return c
	}
	if c := a.d.End() - b.d.End(); c != 0 {
		return c
	}
	if c := int(a.d.Code()) - int(b.d.Code()); c != 0 {
		return c
	}
	if c := strings.Compare(a.d.Category().Name(), b.d.Category().Name()); c != 0 {
		return c
	}
	if c := strings.Compare(a.d.Source(), b.d.Source()); c != 0 {
		return c
	}
	if c := strings.Compare(confMessageIdentity(a.d), confMessageIdentity(b.d)); c != 0 {
		return c
	}
	return strings.Compare(strings.Join(a.d.MessageArgs(), "\x01"), strings.Join(b.d.MessageArgs(), "\x01"))
}

// confLineAndUTF16Character — scanner.GetECMALineAndUTF16CharacterOfPosition
// over precomputed line starts (the character is a core.UTF16Offset).
func confLineAndUTF16Character(lineStarts []core.TextPos, text string, pos int) (int, core.UTF16Offset) {
	line := scanner.ComputeLineOfPosition(lineStarts, pos)
	character := core.UTF16Len(text[int(lineStarts[line]):pos])
	return line, character
}

// confMinimalDiagnosticsToString — the header block
// (`file(line+1,char+1): category PREFIXcode: message\r\n`).
func confMinimalDiagnosticsToString(views []confView) string {
	var out strings.Builder
	for _, view := range views {
		d := view.d
		if view.fileName != "" {
			lineStarts := core.ComputeECMALineStarts(view.fileText)
			line, character := confLineAndUTF16Character(lineStarts, view.fileText, d.Pos())
			out.WriteString(confRemoveTestPathPrefixes(view.fileName))
			fmt.Fprintf(&out, "(%d,%d): ", line+1, character+1)
		}
		fmt.Fprintf(&out, "%s %s%d: ", d.Category().Name(), confDiagnosticPrefix(d), d.Code())
		out.WriteString(confFlattenDiagnosticMessage(d))
		out.WriteString(confNewline)
	}
	return out.String()
}

// confReplaceDiagnosticsLocationPrefix — Go's
// `(?im)^(lib.*\.d\.ts)\(\d+,\d+\)` → `$1(--,--)`.
func confReplaceDiagnosticsLocationPrefix(line string) string {
	lower := strings.ToLower(line)
	if !strings.HasPrefix(lower, "lib") {
		return line
	}
	open := strings.Index(lower, ".d.ts(")
	if open < 0 {
		return line
	}
	rest := line[open+len(".d.ts("):]
	i := 0
	for i < len(rest) && rest[i] >= '0' && rest[i] <= '9' {
		i++
	}
	if i == 0 || i >= len(rest) || rest[i] != ',' {
		return line
	}
	afterComma := rest[i+1:]
	j := 0
	for j < len(afterComma) && afterComma[j] >= '0' && afterComma[j] <= '9' {
		j++
	}
	if j == 0 || j >= len(afterComma) || afterComma[j] != ')' {
		return line
	}
	return line[:open+len(".d.ts(")] + "(--,--)"
}

// confOutputErrorText — the `!!! category TScode: message` lines (the
// twin's PORT deviation: related information renders without the
// `file(line,col)` location).
func confOutputErrorText(first *bool, outputLines *strings.Builder, view confView) {
	newLine := func() string {
		if *first {
			*first = false
			return ""
		}
		return confNewline
	}
	d := view.d
	message := confRemoveTestPathPrefixes(confFlattenDiagnosticMessage(d))
	var errLines []string
	for _, line := range confSplitLineDelimited(message) {
		if len(line) == 0 {
			continue
		}
		errLines = append(errLines, fmt.Sprintf("!!! %s TS%d: %s", d.Category().Name(), d.Code(), line))
	}
	for _, info := range d.RelatedInformation() {
		errLines = append(errLines, fmt.Sprintf("!!! related TS%d: %s", info.Code(), confFlattenDiagnosticMessage(info)))
	}
	for _, e := range errLines {
		outputLines.WriteString(newLine())
		outputLines.WriteString(e)
	}
}

// confGetErrorBaseline — the whole `.errors.txt` body (Go
// iterateErrorBaseline, non-pretty).
func confGetErrorBaseline(inputFiles []confInputFile, views []confView) string {
	sorted := make([]confView, len(views))
	copy(sorted, views)
	sort.SliceStable(sorted, func(i, j int) bool { return confCompareViews(&sorted[i], &sorted[j]) < 0 })

	var result []string

	// the minimal-diagnostics block
	top := confMinimalDiagnosticsToString(sorted)
	top = confRemoveTestPathPrefixes(top)
	topLines := make([]string, 0, 8)
	for _, line := range confSplitLineDelimited(top) {
		topLines = append(topLines, confReplaceDiagnosticsLocationPrefix(line))
	}
	top = strings.Join(topLines, confNewline)
	result = append(result, top+confNewline+confNewline)

	// global errors
	first := true
	var outputLines strings.Builder
	for _, view := range sorted {
		if view.fileName == "" {
			confOutputErrorText(&first, &outputLines, view)
		}
	}
	result = append(result, outputLines.String())
	outputLines.Reset()

	// one section per input file
	for _, input := range inputFiles {
		var fileErrors []confView
		for _, view := range sorted {
			if view.fileName == "" {
				continue
			}
			if tspath.ComparePaths(
				confRemoveTestPathPrefixes(view.fileName),
				confRemoveTestPathPrefixes(input.unitName),
				tspath.CaseInsensitive,
			) == 0 {
				fileErrors = append(fileErrors, view)
			}
		}

		// header
		if !first {
			outputLines.WriteString(confNewline)
		} else {
			first = false
		}
		fmt.Fprintf(&outputLines, "==== %s (%d errors) ====", confRemoveTestPathPrefixes(input.unitName), len(fileErrors))

		lineStarts := core.ComputeECMALineStarts(input.content)
		lines := confSplitLineDelimited(input.content)

		for lineIndex, line := range lines {
			thisLineStart := int(lineStarts[lineIndex])
			var nextLineStart int
			if lineIndex == len(lines)-1 {
				nextLineStart = len(input.content)
			} else {
				nextLineStart = int(lineStarts[lineIndex+1])
			}
			if !first {
				outputLines.WriteString(confNewline)
			} else {
				first = false
			}
			outputLines.WriteString("    ")
			outputLines.WriteString(line)
			for _, view := range fileErrors {
				d := view.d
				errStart := d.Pos()
				end := errStart + d.Len()
				if end >= thisLineStart && (errStart < nextLineStart || lineIndex == len(lines)-1) {
					relativeOffset := errStart - thisLineStart
					length := (end - errStart) - max(0, thisLineStart-errStart)
					squiggleStart := max(0, relativeOffset)
					if !first {
						outputLines.WriteString(confNewline)
					} else {
						first = false
					}
					outputLines.WriteString("    ")
					squiggleStart = min(squiggleStart, len(line))
					outputLines.WriteString(confReplaceNonWhitespace(line[:squiggleStart]))
					squiggleEnd := max(squiggleStart, min(squiggleStart+length, len(line)))
					outputLines.WriteString(strings.Repeat("~", utf8.RuneCountInString(line[squiggleStart:squiggleEnd])))
					if lineIndex == len(lines)-1 || nextLineStart > end {
						confOutputErrorText(&first, &outputLines, view)
					}
				}
			}
		}
		result = append(result, outputLines.String())
		outputLines.Reset()
	}

	return strings.Join(result, "")
}

// confReplaceNonWhitespace — Go's `\S` → " " (RE2's `\s` =
// [\t\n\f\r ]).
func confReplaceNonWhitespace(line string) string {
	var out strings.Builder
	out.Grow(len(line))
	for _, c := range line {
		if c == '\t' || c == '\n' || c == '\f' || c == '\r' || c == ' ' {
			out.WriteRune(c)
		} else {
			out.WriteByte(' ')
		}
	}
	return out.String()
}

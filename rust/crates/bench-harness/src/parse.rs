//! `tsc-bench parse` — the Phase A corpus parse race driver (Rust side).
//!
//! Contract: rust/bench/DRIVER-CONTRACT.md (authoritative). Go reference:
//! rust/bench/go-driver/main.go — this module mirrors its structure and
//! decisions exactly (indexCorpus / runPass / parseAndWalk / shapeHash /
//! round3, same field order, same defaults, same error semantics).
//!
//!   tsc-bench parse --corpus <dir> [--threads N] [--iterations K] [--iters K] [--json]
//!
//! The parse itself routes through [`crate::parser_seam`] (panics until the
//! M3 parser wave lands); everything else — corpus walk, threading
//! determinism, node counting, kind histogram, shape_hash, JSON — is final.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::scope;
use std::time::Instant;

use tsc_ast::{Kind, NodeStore, SourceFile};
use tsc_core::scriptkind::ScriptKind;
use tsc_tspath::{
    CaseSensitivity, EXTENSION_CJS, EXTENSION_CTS, EXTENSION_JS, EXTENSION_JSON, EXTENSION_JSX,
    EXTENSION_MJS, EXTENSION_MTS, EXTENSION_TS, EXTENSION_TSX, PathKey, RootedFilePath,
    rooted_file_path_from_absolute,
};

/// Go: `type Kind int16`; the histogram is indexed by the kind ordinal
/// (verified 1:1 with Go's kind_generated.go — see the ast crate's
/// kind-ordinal mirror tests). `Kind::Count` is the count sentinel.
const KIND_COUNT: usize = Kind::Count as usize;

/// Go: `skipDirs` — directory names skipped during the corpus walk.
const SKIP_DIRS: [&str; 6] = ["node_modules", ".git", "dist", "build", "out", ".yarn"];

/// Go: `benchExts` — extensions indexed from the corpus. ".d.ts" ends with
/// ".ts" and is therefore covered, exactly as in Go (filepath.Ext semantics).
const BENCH_EXTS: [&str; 4] = [".ts", ".tsx", ".mts", ".cts"];

/// Go: `benchFile` — one indexed corpus file: text, precomputed parse inputs,
/// byte size. Built before the timed section; reused across all iterations.
pub(crate) struct BenchFile {
    pub(crate) text: String,
    pub(crate) bytes: i64,
    pub(crate) file_name: RootedFilePath,
    pub(crate) path_key: PathKey,
    pub(crate) script_kind: ScriptKind,
}

/// Go: `fileStats` — the per-file result of one parse+walk pass. `hist` is
/// indexed by kind ordinal (ascending ordinals == ascending Vec indices, so
/// the shape_hash iteration order falls out directly).
struct FileStats {
    nodes: i64,
    errors: i64,
    hist: Vec<i64>,
}

impl FileStats {
    fn empty() -> FileStats {
        FileStats { nodes: 0, errors: 0, hist: vec![0; KIND_COUNT] }
    }
}

/// The parse seam injection point: a plain fn pointer (Copy + Send + Sync).
/// Production passes [`crate::parser_seam::bench_parse`]; tests inject a
/// fake parser that builds a fixed tiny tree.
pub(crate) type ParseFn = fn(&BenchFile) -> SourceFile;

// ──────────────────────────────────────────────────────────────────────
// CLI
// ──────────────────────────────────────────────────────────────────────

struct Cli {
    corpus: String,
    threads: usize,
    iterations: i64,
    json: bool,
}

/// Parses `parse` subcommand args. Flag-for-flag identical to the Go driver
/// (Go's `flag` package semantics: `--x v`, `--x=v`, and `-x` all accepted;
/// `--iters K` is an alias that OVERRIDES `--iterations` when > 0, exactly
/// as `if *itersAlias > 0 { *iterations = *itersAlias }` in main.go).
fn parse_cli(args: &[String]) -> Result<Cli, String> {
    let mut corpus = String::new();
    let mut threads: i64 = 1;
    let mut iterations: i64 = 1;
    let mut iters_alias: i64 = 0;
    let mut json = false;

    let mut i = 0;
    let flag_value = |flag: &str, i: &mut usize, inline: Option<&str>| -> Result<String, String> {
        if let Some(v) = inline {
            return Ok(v.to_string());
        }
        *i += 1;
        args.get(*i)
            .cloned()
            .ok_or_else(|| format!("flag needs an argument: -{flag}"))
    };

    while i < args.len() {
        let arg = &args[i];
        let (name, inline) = match arg.split_once('=') {
            Some((n, v)) => (n.trim_start_matches('-'), Some(v)),
            None => (arg.trim_start_matches('-'), None),
        };
        match name {
            "corpus" => corpus = flag_value("corpus", &mut i, inline)?,
            "threads" => {
                threads = flag_value("threads", &mut i, inline)?
                    .parse()
                    .map_err(|e| format!("invalid value for --threads: {e}"))?
            }
            "iterations" => {
                iterations = flag_value("iterations", &mut i, inline)?
                    .parse()
                    .map_err(|e| format!("invalid value for --iterations: {e}"))?
            }
            "iters" => {
                iters_alias = flag_value("iters", &mut i, inline)?
                    .parse()
                    .map_err(|e| format!("invalid value for --iters: {e}"))?
            }
            "json" => {
                // Go's `flag` bool: bare flag means true; `--json=v` parses v.
                json = match inline {
                    None => true,
                    Some(v) => v.parse().map_err(|_| "invalid boolean value for --json".to_string())?,
                }
            }
            other => return Err(format!("parse: flag provided but not defined: -{other}")),
        }
        i += 1;
    }

    if iters_alias > 0 {
        iterations = iters_alias;
    }
    if threads < 1 {
        threads = 1;
    }
    if iterations < 1 {
        iterations = 1;
    }
    Ok(Cli { corpus, threads: threads as usize, iterations, json })
}

/// Entry point for `tsc-bench parse --corpus <dir> [...]`.
pub fn run(args: &[String]) {
    let cli = match parse_cli(args) {
        Ok(cli) => cli,
        Err(err) => {
            eprintln!("tsc-bench: {err}");
            std::process::exit(2);
        }
    };
    if cli.corpus.is_empty() {
        eprintln!("tsc-bench: missing required --corpus <dir>");
        std::process::exit(2);
    }

    let (files, read_errs) = match index_corpus(&cli.corpus) {
        Ok(pair) => pair,
        Err(err) => {
            eprintln!("tsc-bench: {err}");
            std::process::exit(1);
        }
    };

    let total_bytes: i64 = files.iter().map(|f| f.bytes).sum();
    // Unreadable files count as one error each (index_corpus) and are
    // excluded from files/bytes — contract rule 6.
    let mut total_errors = read_errs;

    // ── Timed section: parse + walk only, across all K passes. ──
    // (File IO happened in index_corpus, before `start` — contract rule 8.
    // Like Go, the per-pass merge also falls inside the timed section.)
    let parse: ParseFn = crate::parser_seam::bench_parse;
    let start = Instant::now();
    let mut total_nodes: i64 = 0;
    let mut hist = vec![0i64; KIND_COUNT];
    for _ in 0..cli.iterations {
        let stats = run_pass(&files, cli.threads, parse);
        // Merge in file-index order so aggregation is deterministic
        // regardless of worker scheduling (contract rule 7).
        for s in stats {
            total_nodes += s.nodes;
            total_errors += s.errors;
            merge_hist(&mut hist, &s.hist);
        }
    }
    let wall = start.elapsed();

    let wall_ms = round3(wall.as_secs_f64() * 1000.0);
    let mb_per_s = if wall.as_secs_f64() > 0.0 {
        round3((total_bytes as f64 * cli.iterations as f64 / (1024.0 * 1024.0)) / wall.as_secs_f64())
    } else {
        0.0
    };

    let shape_hash = shape_hash(total_nodes, total_errors, &hist);

    if cli.json {
        // Fixed key order (contract); numbers formatted exactly like Go's
        // encoding/json (shortest round-trip, no trailing ".0" — see go_float).
        println!(
            "{{\"driver\":\"tsc-bench\",\"op\":\"parse\",\"files\":{},\"bytes\":{},\"iterations\":{},\"threads\":{},\"wall_ms\":{},\"mb_per_s\":{},\"nodes\":{},\"errors\":{},\"shape_hash\":\"{}\"}}",
            files.len(),
            total_bytes,
            cli.iterations,
            cli.threads,
            go_float(wall_ms),
            go_float(mb_per_s),
            total_nodes,
            total_errors,
            shape_hash
        );
        return;
    }
    println!("driver:      tsc-bench");
    println!("op:          parse");
    println!("files:       {}", files.len());
    println!("bytes:       {total_bytes}");
    println!("iterations:  {}", cli.iterations);
    println!("threads:     {}", cli.threads);
    println!("wall_ms:     {:.3}", wall_ms);
    println!("mb_per_s:    {:.3}", mb_per_s);
    println!("nodes:       {total_nodes}");
    println!("errors:      {total_errors}");
    println!("shape_hash:  {shape_hash}");
}

// ──────────────────────────────────────────────────────────────────────
// Corpus indexing (Go: indexCorpus)
// ──────────────────────────────────────────────────────────────────────

/// Walks the corpus deterministically and reads every indexed file fully
/// into memory before any timing starts. Returns (files, read_errors).
///
/// PORT-notes vs Go:
/// - Abs: `std::path::absolute` is lexical like Go's `filepath.Abs`
///   (neither resolves symlinks).
/// - WalkDir does not follow symlinks; `entry.file_type().is_dir()` is the
///   same lstat-style check, so symlinked directories are indexed as files
///   (and then fail to read, +1 error) identically on both sides.
/// - Go's `os.ReadFile` accepts arbitrary bytes; Rust `String` requires
///   UTF-8, so non-UTF-8 sources take the lossy path (never on the TS
///   corpus; text is never used for counting, `bytes` counts raw bytes).
/// - Non-UTF-8 PATHS count as read errors (+1, walk continues); Go would
///   read them fine — never occurs on the race corpus.
fn index_corpus(dir: &str) -> Result<(Vec<BenchFile>, i64), String> {
    let abs = std::path::absolute(dir).map_err(|err| format!("resolving corpus dir: {err}"))?;

    let mut paths: Vec<PathBuf> = Vec::new();
    let mut read_errs: i64 = 0;
    walk(&abs, &abs, &mut paths, &mut read_errs)?;

    // WalkDir is depth-first lexical per directory; sort globally by path so
    // both drivers walk the identical order (contract CLI rule).
    paths.sort();

    let mut files = Vec::with_capacity(paths.len());
    for path in paths {
        let data = match std::fs::read(&path) {
            Ok(data) => data,
            // Unreadable file: +1 error, continue (contract rule 6).
            Err(_) => {
                read_errs += 1;
                continue;
            }
        };
        let Some(path_str) = path.to_str() else {
            read_errs += 1;
            continue;
        };
        let file_name = rooted_file_path_from_absolute(path_str);
        files.push(BenchFile {
            bytes: data.len() as i64,
            text: String::from_utf8_lossy(&data).into_owned(),
            path_key: CaseSensitivity::CaseSensitive.path_key(&file_name.as_path()),
            script_kind: script_kind_from_file_name(&file_name),
            file_name,
        });
    }
    Ok((files, read_errs))
}

/// Recursive corpus walk (Go: `filepath.WalkDir` callback).
fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>, read_errs: &mut i64) -> Result<(), String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        // Unreadable root is fatal; unreadable entries count as errors and
        // the walk continues.
        Err(err) => {
            if dir == root {
                return Err(format!("walking corpus: {err}"));
            }
            *read_errs += 1;
            return Ok(());
        }
    };
    let mut items: Vec<(String, PathBuf, bool)> = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                *read_errs += 1;
                continue;
            }
        };
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        items.push((entry.file_name().to_string_lossy().into_owned(), entry.path(), is_dir));
    }
    items.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, path, is_dir) in items {
        if is_dir {
            // `root` is never a child entry, so (unlike Go's `path !=
            // absCorpus` guard) every recursed dir is skippable by name.
            if path.file_name().is_some_and(is_skip_dir) {
                continue;
            }
            walk(root, &path, out, read_errs)?;
        } else if path.file_name().is_some_and(is_bench_ext) {
            out.push(path);
        }
    }
    Ok(())
}

fn is_skip_dir(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else { return false };
    SKIP_DIRS.contains(&name)
}

/// Go: `benchExts[strings.ToLower(filepath.Ext(path))]` — the extension is
/// the suffix from the final "." of the final path element (so ".d.ts" ends
/// with ".ts" and is covered), lowercased.
fn is_bench_ext(name: &OsStr) -> bool {
    BENCH_EXTS.contains(&lower_ext(name).as_str())
}

fn lower_ext(name: &OsStr) -> String {
    let s = name.to_str().unwrap_or("");
    match s.rfind('.') {
        Some(i) => s[i..].to_lowercase(),
        None => String::new(),
    }
}

/// Go: `core.GetScriptKindFromFileName` (tsc/internal/core/core.go:525) —
/// mapped by final (any-)extension, lowercased. Since the driver only
/// indexes .ts/.tsx/.mts/.cts the effective mapping is TSX for ".tsx", TS
/// for the rest (contract rule 1); the full mapping is ported anyway.
fn script_kind_from_file_name(file_name: &RootedFilePath) -> ScriptKind {
    let extension = file_name.any_extension(&[], CaseSensitivity::CaseSensitive);
    if !extension.is_empty() {
        match extension.to_lowercase().as_str() {
            EXTENSION_JS | EXTENSION_CJS | EXTENSION_MJS => return ScriptKind::JS,
            EXTENSION_JSX => return ScriptKind::JSX,
            EXTENSION_TS | EXTENSION_CTS | EXTENSION_MTS => return ScriptKind::TS,
            EXTENSION_TSX => return ScriptKind::TSX,
            EXTENSION_JSON => return ScriptKind::JSON,
            _ => {}
        }
    }
    ScriptKind::Unknown
}

// ──────────────────────────────────────────────────────────────────────
// Pass machinery (Go: runPass / parseAndWalk)
// ──────────────────────────────────────────────────────────────────────

/// Parses+walks every file once, using `threads` worker threads over the
/// indexed file list. Each pass fully joins before returning, so a pass's
/// per-file stats slots are each written by exactly one worker and merged
/// by the caller in file-index order — output is identical at any thread
/// count (contract rule 7).
fn run_pass(files: &[BenchFile], threads: usize, parse: ParseFn) -> Vec<FileStats> {
    if files.is_empty() {
        return Vec::new();
    }
    if threads == 1 {
        return files.iter().map(|f| parse_and_walk(f, parse)).collect();
    }
    // Work-stealing over an atomic counter, one Mutex-guarded slot per file
    // index (Go writes `stats[i]` from goroutines; the Mutex is the Rust
    // borrow-checker's price for the same disjoint-slot discipline).
    let slots: Vec<Mutex<FileStats>> = files.iter().map(|_| Mutex::new(FileStats::empty())).collect();
    let next = AtomicUsize::new(0);
    scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                if i >= files.len() {
                    break;
                }
                let stats = parse_and_walk(&files[i], parse);
                *slots[i].lock().unwrap() = stats;
            });
        }
    });
    slots.into_iter().map(|slot| slot.into_inner().unwrap()).collect()
}

/// Parses one file through the seam and counts (contract rule 2):
///
/// - `nodes`: the SourceFile root + every node reachable through the
///   non-JSDoc `for_each_child` walker, INCLUDING `EndOfFileToken`,
///   EXCLUDING NodeList wrappers (the walker visits list children, never
///   the list — mirrors Go's visitNodeList). The walker visitor returns
///   false to continue (self-recursing Go visitor); here an explicit stack
///   replaces the recursion (count + histogram are order-independent, so
///   the visited SET is what must match, and it does).
/// - `errors`: `len(sf.diagnostics())` — parse diagnostics only (rule 3).
fn parse_and_walk(f: &BenchFile, parse: ParseFn) -> FileStats {
    let sf = parse(f);
    let mut stats = FileStats::empty();
    let mut stack = vec![sf.as_node_id()];
    while let Some(id) = stack.pop() {
        let node = NodeStore::node(&sf, id);
        stats.nodes += 1;
        stats.hist[node.kind_value() as usize] += 1;
        node.for_each_child(&mut |child| {
            stack.push(child);
            false // keep going (true would stop the traversal)
        });
    }
    stats.errors = sf.diagnostics().len() as i64;
    stats
}

fn merge_hist(total: &mut [i64], one: &[i64]) {
    for (t, c) in total.iter_mut().zip(one) {
        *t += c;
    }
}

// ──────────────────────────────────────────────────────────────────────
// shape_hash + rounding (Go: shapeHash / round3)
// ──────────────────────────────────────────────────────────────────────

/// FNV-1a 64 fold of one line: `h ^= b; h *= 0x100000001b3` (wrapping).
fn fold(h: &mut u64, s: &str) {
    for b in s.bytes() {
        *h ^= u64::from(b);
        *h = h.wrapping_mul(0x0100_0000_01b3);
    }
}

/// The exact byte stream the recipe folds (exposed for tests).
fn hash_lines(total_nodes: i64, total_errors: i64, hist: &[i64]) -> Vec<String> {
    let mut lines = vec![format!("nodes={total_nodes}\n"), format!("errors={total_errors}\n")];
    // Vec index == kind ordinal, so ascending index order IS the required
    // ascending-ordinal order; zero-count kinds are skipped.
    for (ordinal, count) in hist.iter().enumerate() {
        if *count > 0 {
            lines.push(format!("{ordinal}:{count}\n"));
        }
    }
    lines
}

/// Contract shape_hash recipe — must match the Go driver byte-for-byte.
fn shape_hash(total_nodes: i64, total_errors: i64, hist: &[i64]) -> String {
    let mut h: u64 = 0xcbf29ce484222325; // FNV-1a 64 offset basis
    for line in hash_lines(total_nodes, total_errors, hist) {
        fold(&mut h, &line);
    }
    format!("{h:016x}") // %016x, lowercase
}

/// Go: `math.Round(x*1000)/1000` (half away from zero, like Rust's round).
fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

/// Formats a float the way Go's `encoding/json` does (shortest round-trip
/// decimal; integral values carry no ".0" — Rust's `Display` for f64 already
/// matches in the driver's value domain, e.g. `0` → "0", `254.5` → "254.5").
fn go_float(x: f64) -> String {
    debug_assert!(x.is_finite());
    format!("{x}")
}

// ──────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tsc_ast::{NodeFactory, NodeList, SourceFileParseOptions, TextRange, TokenFlags};
    use tsc_tspath::rooted_file_path_from_normalized;

    macro_rules! svec {
        ($($s:expr),* $(,)?) => { &[$($s.to_string()),*] };
    }

    // ── (a) shape_hash recipe against hand-computed FNV-1a vectors ──
    //
    // FNV-1a 64: h = 0xcbf29ce484222325; per byte: h ^= b; h *= 0x100000001b3
    // (mod 2^64).
    //
    // Step-by-step for fold("nodes=1\n") from the offset basis
    // (verified by running the recipe's arithmetic, byte per byte):
    //
    //   h0 = 0xcbf29ce484222325
    //   'n' → 0xaf63e34c8601f871
    //   'o' → 0x08b35907b5589afa  (folded in order; each step XOR then multiply)
    //   'd' → 0x215ae619258eba7a
    //   'e' → 0x3c2f1bbad18642ad
    //   's' → 0xca4efc7207239f3a
    //   '=' → 0xe7d5fcc2218738e5
    //   '1' → 0x77d551def8c5903c
    //   '\n'→ 0x650a53e0b7b40bc2
    //
    // Full recipe vectors (continuing the fold over the remaining lines):
    //   A: nodes=2, errors=0, hist{1:2}  → lines "nodes=2\n" "errors=0\n"
    //      "1:2\n" → 303146fe34b96e68
    //   B: nodes=3, errors=1, hist{1:1, 2:2} → lines "nodes=3\n" "errors=1\n"
    //      "1:1\n" "2:2\n" → a36b8e5caf8bb849
    #[test]
    fn fnv1a_fold_intermediate_vector() {
        let mut h: u64 = 0xcbf29ce484222325;
        fold(&mut h, "nodes=1\n");
        assert_eq!(h, 0x650a53e0b7b40bc2);
    }

    #[test]
    fn shape_hash_recipe_vectors() {
        let mut hist = vec![0i64; KIND_COUNT];
        hist[1] = 2;
        assert_eq!(shape_hash(2, 0, &hist), "303146fe34b96e68");

        let mut hist = vec![0i64; KIND_COUNT];
        hist[1] = 1;
        hist[2] = 2;
        assert_eq!(shape_hash(3, 1, &hist), "a36b8e5caf8bb849");
    }

    // ── (c) histogram fold + ordinal-order formatting ──
    #[test]
    fn hash_lines_ascending_ordinal_order_and_format() {
        let mut hist = vec![0i64; KIND_COUNT];
        hist[5] = 3;
        hist[1] = 2; // inserted out of order — must emit ascending
        assert_eq!(
            hash_lines(2, 0, &hist),
            vec!["nodes=2\n".to_string(), "errors=0\n".to_string(), "1:2\n".to_string(), "5:3\n".to_string()]
        );
        // Zero-count kinds are skipped entirely.
        assert_eq!(hash_lines(0, 0, &vec![0i64; KIND_COUNT]).len(), 2);
    }

    #[test]
    fn histogram_fold_sums_slots() {
        let mut total = vec![0i64; KIND_COUNT];
        let mut one = vec![0i64; KIND_COUNT];
        one[1] = 4;
        one[300] = 1;
        merge_hist(&mut total, &one);
        merge_hist(&mut total, &one);
        assert_eq!(total[1], 8);
        assert_eq!(total[300], 2);
    }

    // ── CLI parity with the Go driver ──
    #[test]
    fn cli_defaults_and_iters_alias() {
        let args = svec!["--corpus", "/x"];
        let cli = parse_cli(args).unwrap();
        assert_eq!(cli.threads, 1);
        assert_eq!(cli.iterations, 1);
        assert!(!cli.json);

        // --iters overrides --iterations when > 0 (Go: `if *itersAlias > 0`).
        let cli = parse_cli(svec!["--corpus", "/x", "--iterations", "3", "--iters", "7", "--json"]).unwrap();
        assert_eq!(cli.iterations, 7);
        assert!(cli.json);

        // --iters 0 does NOT override (Go's condition is strictly > 0).
        let cli = parse_cli(svec!["--corpus", "/x", "--iterations", "3", "--iters", "0"]).unwrap();
        assert_eq!(cli.iterations, 3);

        // Go flag forms: --x=v and -x.
        let cli = parse_cli(svec!["--corpus=/x", "-threads", "4", "-iterations=2"]).unwrap();
        assert_eq!(cli.corpus, "/x");
        assert_eq!(cli.threads, 4);
        assert_eq!(cli.iterations, 2);

        // Sub-1 values clamp to 1 (Go: `if *threads < 1`).
        let cli = parse_cli(svec!["--corpus", "/x", "--threads", "0", "--iterations", "-5"]).unwrap();
        assert_eq!(cli.threads, 1);
        assert_eq!(cli.iterations, 1);

        assert!(parse_cli(svec!["--corpus"]).is_err()); // missing value
        assert!(parse_cli(svec!["--wat", "1"]).is_err()); // unknown flag
        // No args parses fine; run() enforces the required --corpus.
        assert!(parse_cli(svec![]).unwrap().corpus.is_empty());
    }

    // ── scriptKind mapping (contract rule 1) ──
    #[test]
    fn script_kind_mapping_mirrors_go() {
        let cases = [
            ("/c/a.ts", ScriptKind::TS),
            ("/c/a.d.ts", ScriptKind::TS), // ends in ".ts"
            ("/c/a.mts", ScriptKind::TS),
            ("/c/a.cts", ScriptKind::TS),
            ("/c/a.tsx", ScriptKind::TSX),
            ("/c/A.TSX", ScriptKind::TSX), // Go lowercases the extension
            // Full mapping ported even though the walk never indexes these:
            ("/c/a.js", ScriptKind::JS),
            ("/c/a.cjs", ScriptKind::JS),
            ("/c/a.mjs", ScriptKind::JS),
            ("/c/a.jsx", ScriptKind::JSX),
            ("/c/a.json", ScriptKind::JSON),
            ("/c/a.txt", ScriptKind::Unknown),
            ("/c/noext", ScriptKind::Unknown),
        ];
        for (path, expected) in cases {
            assert_eq!(script_kind_from_file_name(&rooted_file_path_from_normalized(path)), expected, "{path}");
        }
    }

    // ── (b) corpus walk skip/sort/extension logic over a tempdir fixture ──
    #[test]
    fn corpus_walk_skip_sort_extension() {
        let root = temp_fixture("walk");
        write(&root.join("b.tsx"), "let x = 1;\n");
        write(&root.join("a.ts"), "let a = 1;\n");
        write(&root.join("c.mts"), "");
        write(&root.join("d.cts"), "");
        write(&root.join("e.d.ts"), "declare const e: number;\n");
        write(&root.join("UP.TS"), ""); // Go lowercases: indexed
        write(&root.join("skip.js"), ""); // wrong extension
        write(&root.join("noext"), "");
        write(&root.join("x.txt"), "");
        for skip in SKIP_DIRS {
            let dir = root.join(skip);
            std::fs::create_dir_all(&dir).unwrap();
            write(&dir.join("inside.ts"), "SHOULD NOT BE INDEXED");
        }
        let sub = root.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        write(&sub.join("z.ts"), "");

        let (files, read_errs) = index_corpus(root.to_str().unwrap()).unwrap();
        let names: Vec<&str> = files.iter().map(|f| f.file_name.base_name()).collect();
        // Sorted by path, skip dirs excluded, .d.ts/.TS indexed, others not.
        assert_eq!(
            names,
            vec!["UP.TS", "a.ts", "b.tsx", "c.mts", "d.cts", "e.d.ts", "z.ts"]
        );
        assert_eq!(read_errs, 0);
        // Bytes come from the raw file content (UTF-8 passthrough):
        // b.tsx "let x = 1;\n" (11) + a.ts "let a = 1;\n" (11)
        // + e.d.ts "declare const e: number;\n" (25); the rest are empty.
        assert_eq!(files.iter().map(|f| f.bytes).sum::<i64>(), 47);
        // Precomputed parse inputs (contract rule 4): FileName + PathKey.
        for f in &files {
            assert_eq!(f.path_key.as_string(), f.file_name.as_string());
            assert_ne!(f.file_name.as_string(), "");
        }
        let tsx = files.iter().find(|f| f.file_name.base_name() == "b.tsx").unwrap();
        assert_eq!(tsx.script_kind, ScriptKind::TSX);
        let ts = files.iter().find(|f| f.file_name.base_name() == "a.ts").unwrap();
        assert_eq!(ts.script_kind, ScriptKind::TS);

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_file_counts_one_error_and_walk_continues() {
        let root = temp_fixture("unreadable");
        write(&root.join("a.ts"), "let a = 1;\n");
        write(&root.join("b.ts"), "let b = 1;\n");
        // A dangling symlink is indexed (lstat-style walk, Go ditto) but
        // unreadable: +1 error, excluded from files/bytes, walk continues.
        std::os::unix::fs::symlink(root.join("does-not-exist.ts"), root.join("dead.ts")).unwrap();

        let (files, read_errs) = index_corpus(root.to_str().unwrap()).unwrap();
        assert_eq!(read_errs, 1);
        assert_eq!(files.len(), 2);
        assert_eq!(files.iter().map(|f| f.bytes).sum::<i64>(), 22);

        // The read error feeds the run's `errors` total and therefore the
        // shape_hash (contract rule 6): simulate the fold.
        let hist = vec![0i64; KIND_COUNT];
        assert_eq!(shape_hash(0, 1, &hist).len(), 16);

        std::fs::remove_dir_all(&root).unwrap();
    }

    // ── (d) totals scale with iterations; determinism at threads 1 vs 2 ──
    //
    // Fake parser injected at the seam: builds a fixed tiny tree
    //     SourceFile → statements:[BinaryExpression(x + 1)] + EndOfFile
    // i.e. 6 nodes per file per pass:
    //     SourceFile, BinaryExpression, Identifier, NumericLiteral,
    //     PlusToken, EndOfFile.
    fn fake_parse(f: &BenchFile) -> SourceFile {
        let opts = SourceFileParseOptions {
            file_name: f.file_name.clone(),
            path_key: f.path_key.clone(),
            external_module_indicator_options: Default::default(),
        };
        let mut file = SourceFile::new(0, opts, f.text.clone(), None, None);
        let (bin, eof) = {
            let mut fac = NodeFactory::new(&mut file);
            let x = fac.new_identifier("x");
            let one = fac.new_numeric_literal("1", TokenFlags::NONE);
            let plus = fac.new_token(Kind::PlusToken);
            let bin = fac.new_binary_expression(None, x, None, plus, one);
            let eof = fac.new_token(Kind::EndOfFile);
            (bin, eof)
        };
        file.payload_mut().statements =
            Some(NodeList { loc: TextRange::undefined(), nodes: vec![bin].into_boxed_slice() });
        file.payload_mut().end_of_file_token = Some(eof);
        file
    }

    fn bench_files() -> Vec<BenchFile> {
        (0..3)
            .map(|i| {
                let name = format!("/corpus/f{i}.ts");
                let file_name = rooted_file_path_from_normalized(&name);
                BenchFile {
                    path_key: CaseSensitivity::CaseSensitive.path_key(&file_name.as_path()),
                    script_kind: ScriptKind::TS,
                    file_name,
                    text: format!("let x{i} = {i};\n"),
                    bytes: 12,
                }
            })
            .collect()
    }

    fn run_all(files: &[BenchFile], threads: usize, iterations: i64) -> (i64, i64, Vec<i64>, String) {
        let parse: ParseFn = fake_parse;
        let mut nodes = 0i64;
        let mut errors = 0i64;
        let mut hist = vec![0i64; KIND_COUNT];
        for _ in 0..iterations {
            for s in run_pass(files, threads, parse) {
                nodes += s.nodes;
                errors += s.errors;
                merge_hist(&mut hist, &s.hist);
            }
        }
        (nodes, errors, hist.clone(), shape_hash(nodes, errors, &hist))
    }

    #[test]
    fn totals_scale_with_iterations() {
        let files = bench_files();
        // 3 files × 6 nodes × K — run totals scale with K (contract rule 5).
        let (n1, e1, hist1, hash1) = run_all(&files, 1, 1);
        assert_eq!(n1, 18);
        assert_eq!(e1, 0); // fake tree carries no diagnostics
        assert_eq!(hist1[Kind::EndOfFile as usize], 3);
        assert_eq!(hist1[Kind::SourceFile as usize], 3);
        assert_eq!(hist1[Kind::BinaryExpression as usize], 3);

        let (n3, e3, hist3, hash3) = run_all(&files, 1, 3);
        assert_eq!(n3, 54);
        assert_eq!(e3, 0);
        assert_eq!(hist3[Kind::EndOfFile as usize], 9);
        assert_eq!(hist3[Kind::SourceFile as usize], 9);
        assert_eq!(hist3[Kind::BinaryExpression as usize], 9);
        for (a, b) in hist1.iter().zip(&hist3) {
            assert_eq!(b, &(a * 3));
        }
        assert_ne!(hash1, hash3); // different totals → different digest
        assert_eq!(hash1.len(), 16);
    }

    #[test]
    fn json_counts_and_shape_hash_identical_at_any_thread_count() {
        let files = bench_files();
        for iterations in [1, 3] {
            let one = run_all(&files, 1, iterations);
            let two = run_all(&files, 2, iterations);
            let eight = run_all(&files, 8, iterations);
            assert_eq!(one.0, two.0);
            assert_eq!(one.1, two.1);
            assert_eq!(one.2, two.2);
            assert_eq!(one.3, two.3);
            assert_eq!(one.0, eight.0);
            assert_eq!(one.3, eight.3); // shape_hash identical at any threads
        }
    }

    #[test]
    fn seam_parse_panics_pending_m3() {
        let files = bench_files();
        let result = std::panic::catch_unwind(|| {
            let parse: ParseFn = crate::parser_seam::bench_parse;
            parse_and_walk(&files[0], parse)
        });
        let Err(err) = result else { panic!("seam must panic while the parser is pending") };
        let msg = err
            .downcast_ref::<&'static str>()
            .copied()
            .or_else(|| err.downcast_ref::<String>().map(|s| s.as_str()));
        assert_eq!(msg, Some("tsc-parser pending M3 (see M3-DISPATCH.md)"));
    }

    // ── output formatting ──
    #[test]
    fn go_float_matches_go_json_encoding() {
        assert_eq!(go_float(0.0), "0"); // Go json: 0.0 marshals as "0"
        assert_eq!(go_float(254.5), "254.5");
        assert_eq!(go_float(31.1), "31.1");
        assert_eq!(go_float(1042.0), "1042");
        assert_eq!(go_float(0.125), "0.125");
    }

    #[test]
    fn round3_halves_away_from_zero_like_math_round() {
        assert_eq!(round3(254.5004), 254.5);
        assert_eq!(round3(254.5005), 254.501); // .5 rounds up (away from zero)
        assert_eq!(round3(0.0004), 0.0);
    }

    // ── helpers ──

    fn temp_fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tsc-bench-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }
}

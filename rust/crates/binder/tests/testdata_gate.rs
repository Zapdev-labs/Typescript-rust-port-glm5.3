//! M4 wave-A exit gate — testdata no-panic bind sweep, mirroring the M3
//! parser gate (rust/crates/parser/tests/testdata_gate.rs).
//!
//! Sweeps every file under `tsc/testdata/fixtures/**` (bench-driver walk
//! rules: lexical per-dir order, SKIP_DIRS, no symlink following) plus the
//! 10 Go fuzz corpus seeds in
//! `tsc/internal/parser/testdata/fuzz/FuzzParser/` through
//! [`tsc_parser::parse_source_file`] + [`tsc_binder::bind_source_file`]
//! under `catch_unwind`.
//!
//! `#[ignore]`d by default so unit/CI runs stay fast; run explicitly:
//!
//! ```text
//! CARGO_TARGET_DIR=/tmp/tsrs-binder \
//!   cargo test -p tsc-binder --test testdata_gate -- --ignored --nocapture
//! ```
//!
//! The test FAILS if any file panics the parser OR the binder (both count:
//! a bind panic on a parseable file is the gate's finding). Unknown
//! scriptKind files are skipped and counted (parse_source_file panics on
//! Unknown by design). The summary carries the gate metrics (symbols,
//! flow nodes, bind diagnostics).

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use tsc_ast::{SourceFile, SourceFileParseOptions};
use tsc_core::scriptkind::ScriptKind;
use tsc_parser::parse_source_file;
use tsc_tspath::{
    CaseSensitivity, EXTENSION_CJS, EXTENSION_CTS, EXTENSION_JS, EXTENSION_JSON,
    EXTENSION_JSX, EXTENSION_MJS, EXTENSION_MTS, EXTENSION_TS, EXTENSION_TSX,
    RootedFilePath, rooted_file_path_from_absolute,
};

/// Go: `skipDirs` — same as the parser gate (bench-driver walk rules).
const SKIP_DIRS: [&str; 6] = ["node_modules", ".git", "dist", "build", "out", ".yarn"];

struct FileRecord {
    path: PathBuf,
    bytes: i64,
    /// Some((symbols, flow_nodes, bind_diagnostics)) when bound ok.
    stats: Option<(i64, i64, i64)>,
}

struct PanicRecord {
    path: PathBuf,
    bytes: i64,
    stage: &'static str,
    message: String,
}

struct GateResult {
    bound: Vec<FileRecord>,
    panics: Vec<PanicRecord>,
    unknown_kind: Vec<PathBuf>,
    read_errors: Vec<PathBuf>,
}

// ─────────────────────────────────────────────────────────────────────────
// Corpus walk + scriptKind mapping — verbatim from the parser gate
// ─────────────────────────────────────────────────────────────────────────

fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>, read_errs: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => {
            if dir == root {
                panic!("testdata gate: cannot walk corpus root {}: unreadable", dir.display());
            }
            read_errs.push(dir.to_path_buf());
            return;
        }
    };
    let mut items: Vec<(String, PathBuf, bool)> = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            read_errs.push(dir.to_path_buf());
            continue;
        };
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        items.push((entry.file_name().to_string_lossy().into_owned(), entry.path(), is_dir));
    }
    items.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, path, is_dir) in items {
        if is_dir {
            if path.file_name().is_some_and(is_skip_dir) {
                continue;
            }
            walk(root, &path, out, read_errs);
        } else {
            out.push(path);
        }
    }
}

fn is_skip_dir(name: &OsStr) -> bool {
    name.to_str().is_some_and(|n| SKIP_DIRS.contains(&n))
}

fn script_kind_from_file_name(file_name: &RootedFilePath) -> ScriptKind {
    let extension = file_name.any_extension(&[], CaseSensitivity::CaseSensitive);
    script_kind_from_extension(&extension)
}

fn script_kind_from_extension(extension: &str) -> ScriptKind {
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

// ─────────────────────────────────────────────────────────────────────────
// Go fuzz corpus parsing (`go test fuzz v1` format) — verbatim from the
// parser gate
// ─────────────────────────────────────────────────────────────────────────

fn parse_corpus_string(line: &str) -> Option<String> {
    let rest = line.strip_prefix("string(")?.strip_suffix(')')?;
    let b = rest.as_bytes();
    if b.len() < 2 || b[0] != b'"' || b[b.len() - 1] != b'"' {
        return None;
    }
    let inner = &rest[1..rest.len() - 1];
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            'a' => out.push('\x07'),
            'b' => out.push('\x08'),
            'f' => out.push('\x0c'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            'v' => out.push('\x0b'),
            '\\' => out.push('\\'),
            '"' => out.push('"'),
            '\'' => out.push('\''),
            'x' => {
                let mut h = String::new();
                for _ in 0..2 {
                    h.push(chars.next()?);
                }
                out.push(u8::from_str_radix(&h, 16).ok()? as char);
            }
            'u' => {
                let mut h = String::new();
                for _ in 0..4 {
                    h.push(chars.next()?);
                }
                out.push(char::from_u32(u32::from_str_radix(&h, 16).ok()?)?);
            }
            'U' => {
                let mut h = String::new();
                for _ in 0..8 {
                    h.push(chars.next()?);
                }
                out.push(char::from_u32(u32::from_str_radix(&h, 16).ok()?)?);
            }
            d @ '0'..='7' => {
                let mut v = d.to_digit(8).unwrap();
                for _ in 0..2 {
                    v = v * 8 + chars.next()?.to_digit(8)?;
                }
                out.push(v as u8 as char);
            }
            _ => return None,
        }
    }
    Some(out)
}

fn read_fuzz_seed(path: &Path) -> Option<(String, String)> {
    let raw = fs::read_to_string(path).ok()?;
    let mut lines = raw.lines();
    let header = lines.next()?;
    if header.trim() != "go test fuzz v1" {
        return None;
    }
    let ext = parse_corpus_string(lines.next()?)?;
    let text = parse_corpus_string(lines.next()?)?;
    Some((ext, text))
}

// ─────────────────────────────────────────────────────────────────────────
// The sweep
// ─────────────────────────────────────────────────────────────────────────

fn truncate_200(s: &str) -> String {
    s.chars().take(200).collect()
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}

fn run_sweep() -> GateResult {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let fixtures = repo.join("tsc/testdata/fixtures");
    let fuzz_dir = repo.join("tsc/internal/parser/testdata/fuzz/FuzzParser");

    let mut result = GateResult {
        bound: Vec::new(),
        panics: Vec::new(),
        unknown_kind: Vec::new(),
        read_errors: Vec::new(),
    };

    let mut paths: Vec<PathBuf> = Vec::new();
    walk(&fixtures, &fixtures, &mut paths, &mut result.read_errors);
    paths.sort();

    for path in paths {
        let data = match fs::read(&path) {
            Ok(data) => data,
            Err(_) => {
                result.read_errors.push(path);
                continue;
            }
        };
        let Some(path_str) = path.to_str() else {
            result.read_errors.push(path);
            continue;
        };
        sweep_one(&path, path_str, &data, None, &mut result);
    }

    let mut seed_paths: Vec<PathBuf> = match fs::read_dir(&fuzz_dir) {
        Ok(entries) => entries.filter_map(|e| e.ok().map(|e| e.path())).collect(),
        Err(_) => panic!(
            "testdata gate: cannot read fuzz corpus dir {}: unreadable",
            fuzz_dir.display()
        ),
    };
    seed_paths.sort();
    for path in seed_paths {
        if !path.is_file() {
            continue;
        }
        let Some((ext, text)) = read_fuzz_seed(&path) else {
            result.read_errors.push(path);
            continue;
        };
        let seed_name = format!("{}.{}", path.display(), ext.trim_start_matches('.'));
        sweep_one(&path, &seed_name, text.as_bytes(), Some(&ext), &mut result);
    }

    result
}

fn sweep_one(
    path: &Path,
    path_str: &str,
    data: &[u8],
    ext_override: Option<&str>,
    result: &mut GateResult,
) {
    let file_name = rooted_file_path_from_absolute(path_str);
    let script_kind = match ext_override {
        Some(ext) => script_kind_from_extension(ext),
        None => script_kind_from_file_name(&file_name),
    };
    if script_kind == ScriptKind::Unknown {
        result.unknown_kind.push(path.to_path_buf());
        return;
    }
    let text = String::from_utf8_lossy(data).into_owned();
    let path_key = CaseSensitivity::CaseSensitive.path_key(&file_name.as_path());
    let opts = SourceFileParseOptions {
        file_name,
        path_key,
        external_module_indicator_options: Default::default(),
    };

    // Parse under catch_unwind — the parse panic still counts as a sweep
    // finding for this gate (it must not hide behind a bind run).
    let parsed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        parse_source_file(opts, &text, script_kind)
    }));
    let mut sf: SourceFile = match parsed {
        Ok(sf) => sf,
        Err(payload) => {
            result.panics.push(PanicRecord {
                path: path.to_path_buf(),
                bytes: data.len() as i64,
                stage: "parse",
                message: truncate_200(&panic_message(payload)),
            });
            return;
        }
    };

    // Bind under catch_unwind — the gate's subject.
    let bound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        tsc_binder::bind_source_file(&mut sf)
    }));
    match bound {
        Ok(Some(result_set)) => {
            let bind_diags = sf.payload().bind_diagnostics.len() as i64;
            result.bound.push(FileRecord {
                path: path.to_path_buf(),
                bytes: data.len() as i64,
                stats: Some((
                    result_set.symbol_count as i64,
                    result_set.flow_nodes.len() as i64,
                    bind_diags,
                )),
            });
        }
        Ok(None) => {
            // bind_once refused a second bind — impossible here (fresh
            // file), but treat it as a finding rather than silently passing.
            result.panics.push(PanicRecord {
                path: path.to_path_buf(),
                bytes: data.len() as i64,
                stage: "bind-twice-refused",
                message: "bind_source_file returned None on a fresh file".to_string(),
            });
        }
        Err(payload) => {
            result.panics.push(PanicRecord {
                path: path.to_path_buf(),
                bytes: data.len() as i64,
                stage: "bind",
                message: truncate_200(&panic_message(payload)),
            });
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Reporting + gate verdict
// ─────────────────────────────────────────────────────────────────────────

fn escape_inline(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\n', "\\n").replace('\r', "\\r").replace('\t', "\\t")
}

#[test]
#[ignore = "M4 wave-A exit gate: cargo test -p tsc-binder --test testdata_gate -- --ignored --nocapture"]
fn testdata_no_panic_bind_gate() {
    let result = {
        let prev_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let join = std::thread::Builder::new()
            .stack_size(512 * 1024 * 1024)
            .spawn(run_sweep)
            .expect("spawn sweep thread");
        let joined = join.join();
        std::panic::set_hook(prev_hook);
        let out = joined.unwrap_or_else(|p| panic!("sweep thread panicked: {}", panic_message(p)));
        out
    };

    let total_files = result.bound.len() + result.panics.len() + result.unknown_kind.len();
    let total_symbols: i64 = result.bound.iter().filter_map(|r| r.stats.map(|s| s.0)).sum();
    let total_flow_nodes: i64 = result.bound.iter().filter_map(|r| r.stats.map(|s| s.1)).sum();
    let total_bind_diags: i64 = result.bound.iter().filter_map(|r| r.stats.map(|s| s.2)).sum();

    println!(
        "GATE: files swept={} bound_ok={} panics={} skipped_unknown_kind={} read_errors={}",
        total_files,
        result.bound.len(),
        result.panics.len(),
        result.unknown_kind.len(),
        result.read_errors.len(),
    );
    if !result.unknown_kind.is_empty() {
        println!("GATE: unknown-scriptKind files (not swept, by-design skip):");
        for p in &result.unknown_kind {
            println!("GATE:   {}", p.display());
        }
    }
    if !result.read_errors.is_empty() {
        println!("GATE: unreadable files (walk continued):");
        for p in &result.read_errors {
            println!("GATE:   {}", p.display());
        }
    }
    println!(
        "GATE: total_symbols={total_symbols} total_flow_nodes={total_flow_nodes} total_bind_diagnostics={total_bind_diags}"
    );

    // Top-20 files by symbol count (sanity: big real sources bind deep).
    let mut ranked: Vec<&FileRecord> = result.bound.iter().collect();
    ranked.sort_by(|a, b| {
        let (an, bn) = (a.stats.map(|s| s.0).unwrap_or(0), b.stats.map(|s| s.0).unwrap_or(0));
        bn.cmp(&an).then_with(|| a.path.cmp(&b.path))
    });
    println!("GATE: top-20 by symbol count:");
    for (i, r) in ranked.iter().take(20).enumerate() {
        let (symbols, flow_nodes, diags) = r.stats.unwrap_or((0, 0, 0));
        println!(
            "GATE: {:>2}. symbols={symbols:<6} flow_nodes={flow_nodes:<6} bind_diags={diags:<4} bytes={:<8} {}",
            i + 1,
            r.bytes,
            r.path.display()
        );
    }

    // Panic groups (stage + distinct message), with the smallest file as repro.
    if !result.panics.is_empty() {
        let mut groups: BTreeMap<(&'static str, String), Vec<&PanicRecord>> = BTreeMap::new();
        for p in &result.panics {
            groups.entry((p.stage, p.message.clone())).or_default().push(p);
        }
        println!("GATE: {} distinct panic group(s):", groups.len());
        for (i, ((stage, message), recs)) in groups.iter().enumerate() {
            let smallest = recs.iter().min_by_key(|r| r.bytes).unwrap();
            println!(
                "GATE: PANIC GROUP {}/{} [{}] ({} file(s): \"{}\")",
                i + 1,
                groups.len(),
                stage,
                recs.len(),
                message
            );
            for r in recs.iter().take(10) {
                println!("GATE:   panicked: {} ({} bytes)", r.path.display(), r.bytes);
            }
            if recs.len() > 10 {
                println!("GATE:   ... and {} more", recs.len() - 10);
            }
            let snippet_bytes = fs::read(&smallest.path).unwrap_or_default();
            let snippet: String = String::from_utf8_lossy(&snippet_bytes).chars().take(300).collect();
            println!(
                "GATE:   minimal repro (smallest file, {} bytes): {}",
                smallest.bytes,
                smallest.path.display()
            );
            println!("GATE:   snippet: \"{}\"", escape_inline(&snippet));
        }
    }

    let verdict = if result.panics.is_empty() { "PASS" } else { "FAIL" };
    println!(
        "GATE: {verdict} files={} bound_ok={} panics={} total_symbols={total_symbols} total_flow_nodes={total_flow_nodes} total_bind_diagnostics={total_bind_diags}",
        total_files,
        result.bound.len(),
        result.panics.len(),
    );
    if !result.panics.is_empty() {
        panic!(
            "testdata bind gate: {} panic(s) across {} panicking file(s)",
            result.panics.len(),
            result.panics.len()
        );
    }
}

//! `tsc-bench program` — the M5 program-layer parity twin (SPEC §8 M5:
//! "tsc <file> on a trivial program loads, binds, parses lib.d.ts, and
//! reports parse-level diagnostics").
//!
//! Go reference: `runProgram` in rust/bench/go-driver/main.go. For every
//! parseable script file in the corpus (the `symbols` twin's walk rule —
//! same skip dirs, every file whose scriptKind is known, raw-path sort),
//! both drivers build a ONE-FILE `Program` the way tsc/internal/
//! compiler's program_test.go drives it, bind it, and emit one NDJSON
//! line per root; `diff` is the verdict.
//!
//!   tsc-bench program --corpus <dir> | --file <path>
//!
//! Call shape (both sides, mirroring program_test.go's `TestProgram` +
//! `TestImportSourceProgram` setups):
//!
//!   vfstest.from_map(<corpus tree at its real absolute paths> +
//!                    <synthetic tsconfig>)
//!       → tsc_bundled::wrap_fs (the bundled lib.d.ts overlay)
//!       → get_parsed_command_line_of_config_file_path (the synthetic
//!         tsconfig — PORT: Go's tests also construct configs via
//!         `tsoptions.NewParsedCommandLine(&opts, files, ...)`; the Rust
//!         tsoptions has no such constructor (`file_names` is not pub),
//!         so BOTH drivers take the written-tsconfig route the Rust test
//!         program_test.rs established: the config text is
//!         `{"compilerOptions":{"target":"esnext"},"files":["<root>"]}`
//!         — the Go `CompilerOptions{Target: ScriptTargetESNext}` corpus
//!         shape — and `files` entries are included verbatim with no
//!         existence check, exactly as Go's literal list)
//!       → new_compiler_host (over a counting FS wrapper, see below)
//!       → new_program → program.bind_source_files
//!
//! PORT notes (decisions shared with the Go driver — mirror there):
//!
//!   - RECORD FORMAT (per root; fields both sides emit identically):
//!     `{"file":"<root>","ok":true,"config_errors":N,"loaded":N,"parsed":N,
//!     "bound":N,"symbols":N,"duplicates":N,"parse_diag":[...],
//!     "program_errors":M,"unresolved":K,"shape_hash":"<16 hex>"}`.
//!     - `config_errors`: `program.get_config_file_parsing_diagnostics()`
//!       length (the synthetic tsconfig's own parse — always clean, but
//!       parity-checked).
//!     - `loaded`: DISTINCT file paths successfully read through the
//!       program's CompilerHost FS during construction — a counting
//!       wrapper (this module) around `wrap_fs`'s overlay exists only in
//!       the drivers, both sides identical: lib.d.ts chain, program
//!       files, package.json probes. This is the "loads ... parses
//!       lib.d.ts" half of the M5 gate made countable.
//!     - `parsed`: `program.get_source_files()` length (the program's
//!       file list, in program order — the ordering corpus test asserts
//!       that order is deterministic; the file names are also folded
//!       into shape_hash).
//!     - `bound`: files with `is_bound()` after `bind_source_files()`
//!       (the "binds" half of the gate).
//!     - `symbols`: the binder's per-file allocation count summed over
//!       the program's files (the `symbols` twin's `alloc` shape; Go's
//!       `Program.SymbolCount()` adds the checker's count via
//!       `ForEachCheckerParallel` — checker-scope, M6 — so BOTH sides
//!       sum the binder's `file.SymbolCount` instead, PORT-noted).
//!     - `duplicates`: `program.duplicate_source_files()` length (the
//!       loader's parsed-then-dropped set).
//!     - `parse_diag`: ONLY parse-level diagnostics (the SPEC gate), one
//!       dump-twin record per diagnostic (`{"file":...,"key":...,
//!       "code":N,"pos":N,"end":N,"args":[...]}`), keyed by file, in
//!       program file order then in-diagnostic order: per file
//!       `file.diagnostics()` then `file.js_diagnostics()`. PORT: this
//!       is `GetSyntacticDiagnostics`' core concat; Go additionally
//!       appends `getAdditionalJSSyntacticDiagnostics` for unchecked-JS
//!       files (option-dependent program-layer checks the Rust port
//!       defers with the visitor/SubtreeFacts surface — see program.rs's
//!       header), so BOTH drivers emit the raw parser+JSDoc set and no
//!       checker-level or JS-extras fields exist in the record (the M6
//!       boundary rule: agree by construction, note the extension).
//!     - `program_errors`: `program.get_program_diagnostics()` length
//!       (option verification + include-processor GLOBAL diagnostics —
//!       unsupported-extension root files, lib-reference misses).
//!     - `unresolved`: `program.get_unresolved_imports()` — count, plus
//!       the sorted names folded into shape_hash. Go's set iteration is
//!       random; the port sorts, the Go driver sorts — count and sorted
//!       names are the shared concepts.
//!   - shape_hash: FNV-1a 64 over the fold lines documented at
//!     [`hash_lines`] (vectors in the tests below).
//!   - Corpus mode seeds the WHOLE walked tree (all known-scriptKind
//!     files, at their real absolute paths) into the map FS so in-matrix
//!     imports resolve inside the VFS; `--file <path>` mode seeds ONLY
//!     that file (showconfig's dual-mode shape; a one-file program has
//!     no siblings by construction).
//!   - The synthetic tsconfig lives at `<corpus>/__program__.tsconfig.json`
//!     (both drivers build the identical path + identical config text —
//!     the same `json_escape` on the root path).
//!   - Panics (program build or bind) emit
//!     `{"file":...,"ok":false,"error":"<stage>: <msg, 200 chars>"}` —
//!     panic payloads are inherently driver-specific, so any ok:false
//!     line IS the divergence finding (the gate matrix is panic-free;
//!     the diff makes that visible).
//!   - Case sensitivity: the map FS is CaseSensitive on BOTH drivers
//!     (POSIX-real-path philosophy — machine-independent parity, unlike
//!     Go's TestProgram which uses CaseInsensitive; PORT-noted).
//!   - Unreadable/non-UTF-8-path files are skipped silently
//!     (indexSymbolsCorpus rule, both sides). Non-UTF-8 file CONTENT is
//!     lossy-converted (`from_utf8_lossy` vs Go's raw `string(data)`) —
//!     the same accepted hazard as the dump/symbols twins (the gate
//!     corpus is UTF-8; PORT-noted).

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex};

use tsc_compiler::{new_compiler_host, new_program, ProgramConfig, ProgramFactories, ProgramHosts, ProgramOptions};
use tsc_tsoptions::tsconfigparsing::ExtendedConfigCache;
use tsc_tsoptions::{CompilerOptionsValue, get_parsed_command_line_of_config_file_path};
use tsc_tspath::{CaseSensitivity, RootedFilePath, rooted_file_path_from_absolute};
use tsc_vfs::Fs;

use crate::parse::{BenchFile, json_escape};
use crate::symbols_seam::index_symbols_corpus;

// ──────────────────────────────────────────────────────────────────────
// CLI (Go: runProgram)
// ──────────────────────────────────────────────────────────────────────

/// Entry point for `tsc-bench program --corpus <dir> | --file <path>`.
pub fn run_program(args: &[String]) {
    let flags = match parse_cli(args) {
        Ok(flags) => flags,
        Err(err) => {
            eprintln!("tsc-bench: {err}");
            std::process::exit(2);
        }
    };
    let mut corpus = String::new();
    let mut file = String::new();
    for (name, value) in flags {
        match name.as_str() {
            "corpus" => corpus = value,
            "file" => file = value,
            _ => unreachable!(),
        }
    }
    if corpus.is_empty() && file.is_empty() {
        eprintln!("tsc-bench program: missing required --corpus <dir> or --file <path>");
        std::process::exit(2);
    }
    let mut line = String::with_capacity(512);
    if !file.is_empty() {
        // --file mode: one root, seeded alone (showconfig's dual-mode shape).
        // Exit codes match the Go driver: unreadable file → 1, unknown
        // script kind → 2 (both drivers' rule, PORT-noted).
        let abs = std::path::absolute(&file)
            .unwrap_or_else(|err| {
                eprintln!("tsc-bench: resolving file path: {err}");
                std::process::exit(1);
            })
            .to_string_lossy()
            .into_owned();
        let data = match std::fs::read(Path::new(&abs)) {
            Ok(data) => data,
            Err(err) => {
                eprintln!("tsc-bench: reading {abs}: {err}");
                std::process::exit(1);
            }
        };
        let file_name = rooted_file_path_from_absolute(&abs);
        let script_kind = crate::parse::script_kind_from_file_name(&file_name);
        if script_kind == tsc_core::scriptkind::ScriptKind::Unknown {
            eprintln!("tsc-bench program: {abs} is not a parseable script file");
            std::process::exit(2);
        }
        let root = BenchFile {
            bytes: data.len() as i64,
            text: String::from_utf8_lossy(&data).into_owned(),
            path_key: CaseSensitivity::CaseSensitive.path_key(&file_name.as_path()),
            script_kind,
            file_name,
        };
        let seed = vec![bench_file_clone(&root)];
        let dir = parent_dir_string(root.file_name.as_string());
        line.clear();
        emit_program_line(&mut line, &dir, &root, &seed);
        println!("{line}");
        return;
    }
    let files = index_symbols_corpus(&corpus).unwrap_or_else(|err| {
        eprintln!("tsc-bench: {err}");
        std::process::exit(1);
    });
    let abs_corpus = std::path::absolute(&corpus)
        .unwrap_or_else(|err| {
            eprintln!("tsc-bench: resolving corpus dir: {err}");
            std::process::exit(1);
        })
        .to_string_lossy()
        .into_owned();
    for f in &files {
        line.clear();
        emit_program_line(&mut line, &abs_corpus, f, &files);
        println!("{line}");
    }
}

/// Parses the subcommand's flag grammar (Go `flag` semantics: `--x v`,
/// `--x=v`, `-x`).
fn parse_cli(args: &[String]) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let (name, inline) = match args[i].split_once('=') {
            Some((n, v)) => (n.trim_start_matches('-').to_string(), Some(v.to_string())),
            None => (args[i].trim_start_matches('-').to_string(), None),
        };
        if name != "corpus" && name != "file" {
            return Err(format!("program: flag provided but not defined: -{name}"));
        }
        let value = match inline {
            Some(v) => v,
            None => {
                i += 1;
                args.get(i)
                    .cloned()
                    .ok_or_else(|| format!("program: flag needs an argument: -{name}"))?
            }
        };
        out.push((name, value));
        i += 1;
    }
    Ok(out)
}

/// Clones a `BenchFile` for the `--file` seed list (a fresh handle; the
/// program owns its copies after that).
fn bench_file_clone(f: &BenchFile) -> BenchFile {
    BenchFile {
        bytes: f.bytes,
        text: f.text.clone(),
        path_key: f.path_key.clone(),
        script_kind: f.script_kind,
        file_name: f.file_name.clone(),
    }
}

/// `<dir>` of a rooted path string (the synthetic tsconfig's directory —
/// Go `filepath.Dir` of the absolute input path).
fn parent_dir_string(rooted: &str) -> String {
    match rooted.rfind('/') {
        Some(0) => "/".to_string(),
        Some(idx) => rooted[..idx].to_string(),
        None => rooted.to_string(),
    }
}

// ──────────────────────────────────────────────────────────────────────
// Counting FS (both drivers wrap the CompilerHost's FS identically)
// ──────────────────────────────────────────────────────────────────────

/// `countingFS` — wraps the bundled-overlay FS and records every DISTINCT
/// path whose `read_file` succeeds (the `loaded` observable). Every other
/// method delegates untouched.
struct CountingFs {
    inner: Arc<dyn Fs>,
    read: Mutex<HashSet<String>>,
}

impl CountingFs {
    fn new(inner: Arc<dyn Fs>) -> Self {
        CountingFs {
            inner,
            read: Mutex::new(HashSet::new()),
        }
    }

    /// The distinct successfully-read paths (order-free; the caller sorts).
    fn loaded_names(&self) -> Vec<String> {
        let read = self.read.lock().expect("counting fs poisoned");
        read.iter().cloned().collect()
    }
}

impl Fs for CountingFs {
    fn case_sensitivity(&self) -> CaseSensitivity {
        self.inner.case_sensitivity()
    }

    fn file_exists(&self, path: &RootedFilePath) -> bool {
        self.inner.file_exists(path)
    }

    fn read_file(&self, path: &RootedFilePath) -> Option<String> {
        let contents = self.inner.read_file(path)?;
        self.read
            .lock()
            .expect("counting fs poisoned")
            .insert(path.as_string().to_string());
        Some(contents)
    }

    fn write_file(&self, path: &RootedFilePath, data: &str) -> std::io::Result<()> {
        self.inner.write_file(path, data)
    }

    fn append_file(&self, path: &RootedFilePath, data: &str) -> std::io::Result<()> {
        self.inner.append_file(path, data)
    }

    fn remove(&self, path: &tsc_tspath::RootedPath) -> std::io::Result<()> {
        self.inner.remove(path)
    }

    fn chtimes(
        &self,
        path: &tsc_tspath::RootedPath,
        atime: std::time::SystemTime,
        mtime: std::time::SystemTime,
    ) -> std::io::Result<()> {
        self.inner.chtimes(path, atime, mtime)
    }

    fn directory_exists(&self, path: &tsc_tspath::RootedDirectoryPath) -> bool {
        self.inner.directory_exists(path)
    }

    fn get_accessible_entries(&self, path: &tsc_tspath::RootedDirectoryPath) -> tsc_vfs::Entries {
        self.inner.get_accessible_entries(path)
    }

    fn stat(&self, path: &tsc_tspath::RootedPath) -> Option<Arc<dyn tsc_vfs::FileInfo>> {
        self.inner.stat(path)
    }

    fn realpath(&self, path: &tsc_tspath::RootedPath) -> tsc_tspath::RootedPath {
        self.inner.realpath(path)
    }
}

// ──────────────────────────────────────────────────────────────────────
// Per-root program construction + line emission (Go: emitProgramLine)
// ──────────────────────────────────────────────────────────────────────

/// The always-miss extended-config cache (Go's nil cache — program_test's
/// `nil_extended_config_cache`).
fn nil_extended_config_cache() -> ExtendedConfigCache {
    Arc::new(|_: &str| -> Option<CompilerOptionsValue> { None })
}

/// One parsed-diagnostic record (file-keyed, the dump twin's shape).
struct ParseDiagRecord<'a> {
    file: &'a str,
    diagnostic: &'a tsc_ast::Diagnostic,
}

/// The record fields the shape_hash folds (built per root; grouping them
/// keeps `hash_lines` under the too-many-arguments lint).
struct HashInput<'a> {
    root: &'a str,
    config_errors: usize,
    loaded: &'a [String],
    program_files: &'a [String],
    bound: usize,
    symbols: i64,
    duplicates: usize,
    parse_diags: &'a [ParseDiagRecord<'a>],
    program_errors: usize,
    unresolved: &'a [String],
}

/// The exact byte stream the shape_hash folds (the recipe both drivers
/// MUST reproduce — see the tests for FNV-1a vectors):
///
/// ```text
/// file=<root>\n
/// config_errors=N\n
/// loaded=N\n
/// loadedfile=<path>\n          (sorted)
/// parsed=N\n
/// programfile=<fileName>\n     (program order)
/// bound=N\n
/// symbols=N\n
/// duplicates=N\n
/// diag=<fileName>|<key>|<code>|<pos>|<end>|<arg1>\x01<arg2>...\n
///                              (program order, then diagnostics() then
///                               js_diagnostics() order within a file)
/// program_errors=M\n
/// unresolved=K\n
/// unresolvedname=<name>\n     (sorted)
/// ```
fn hash_lines(input: &HashInput<'_>) -> Vec<String> {
    let mut lines = vec![
        format!("file={}\n", input.root),
        format!("config_errors={}\n", input.config_errors),
        format!("loaded={}\n", input.loaded.len()),
    ];
    for name in input.loaded {
        lines.push(format!("loadedfile={name}\n"));
    }
    lines.push(format!("parsed={}\n", input.program_files.len()));
    for name in input.program_files {
        lines.push(format!("programfile={name}\n"));
    }
    lines.push(format!("bound={}\n", input.bound));
    lines.push(format!("symbols={}\n", input.symbols));
    lines.push(format!("duplicates={}\n", input.duplicates));
    for record in input.parse_diags {
        let d = record.diagnostic;
        lines.push(format!(
            "diag={}|{}|{}|{}|{}|{}\n",
            record.file,
            d.message_key(),
            d.code(),
            d.pos(),
            d.end(),
            d.message_args().join("\u{1}")
        ));
    }
    lines.push(format!("program_errors={}\n", input.program_errors));
    lines.push(format!("unresolved={}\n", input.unresolved.len()));
    for name in input.unresolved {
        lines.push(format!("unresolvedname={name}\n"));
    }
    lines
}

/// FNV-1a 64: `h = 0xcbf29ce484222325`; per byte: `h ^= b;
/// h *= 0x100000001b3` (wrapping).
fn fold(h: &mut u64, s: &str) {
    for b in s.as_bytes() {
        *h ^= u64::from(*b);
        *h = h.wrapping_mul(0x0100_0000_01b3);
    }
}

/// The shape_hash over [`hash_lines`] — lowercase 16-hex (`%016x`).
fn shape_hash(lines: &[String]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for line in lines {
        fold(&mut h, line);
    }
    format!("{h:016x}")
}

/// Emits one root's NDJSON line (Go: `emitProgramLine`). A program-build or
/// bind panic becomes an ok:false line instead of aborting the run (any
/// ok:false IS a divergence finding — panic payloads are driver-specific
/// and never byte-shared).
fn emit_program_line(line: &mut String, corpus_abs: &str, root: &BenchFile, seed: &[BenchFile]) {
    let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        build_bound_program(corpus_abs, root, seed)
    }));
    let (program, counting) = match built {
        Ok(built) => built,
        Err(payload) => {
            emit_failure_line(root, line, "program", &panic_message(payload));
            return;
        }
    };

    // ── Collect the record fields (all deterministic: program order for
    //    file lists, sorted for read/unresolved sets). ──
    let files = program.get_source_files();
    let program_files: Vec<String> = files
        .iter()
        .map(|file| file.file_name().as_string().to_string())
        .collect();
    let mut bound = 0usize;
    let mut symbols = 0i64;
    for file in files {
        if file.is_bound() {
            bound += 1;
        }
        symbols += i64::from(file.payload().symbol_count);
    }
    let duplicates = program.duplicate_source_files().len();
    let mut parse_diags: Vec<ParseDiagRecord<'_>> = Vec::new();
    for file in files {
        for d in file.diagnostics() {
            parse_diags.push(ParseDiagRecord {
                file: file.file_name().as_string(),
                diagnostic: d,
            });
        }
        for d in file.js_diagnostics() {
            parse_diags.push(ParseDiagRecord {
                file: file.file_name().as_string(),
                diagnostic: d,
            });
        }
    }
    let program_errors = program.get_program_diagnostics().len();
    let config_errors = program.get_config_file_parsing_diagnostics().len();
    let unresolved = program.get_unresolved_imports();
    let mut loaded = counting.loaded_names();
    loaded.sort();

    let lines = hash_lines(&HashInput {
        root: root.file_name.as_string(),
        config_errors,
        loaded: &loaded,
        program_files: &program_files,
        bound,
        symbols,
        duplicates,
        parse_diags: &parse_diags,
        program_errors,
        unresolved: &unresolved,
    });
    let hash = shape_hash(&lines);

    // ── The NDJSON line (field order is the contract). ──
    line.push_str("{\"file\":\"");
    json_escape(root.file_name.as_string(), line);
    line.push_str("\",\"ok\":true,\"config_errors\":");
    line.push_str(&config_errors.to_string());
    line.push_str(",\"loaded\":");
    line.push_str(&loaded.len().to_string());
    line.push_str(",\"parsed\":");
    line.push_str(&program_files.len().to_string());
    line.push_str(",\"bound\":");
    line.push_str(&bound.to_string());
    line.push_str(",\"symbols\":");
    line.push_str(&symbols.to_string());
    line.push_str(",\"duplicates\":");
    line.push_str(&duplicates.to_string());
    line.push_str(",\"parse_diag\":[");
    for (di, record) in parse_diags.iter().enumerate() {
        if di > 0 {
            line.push(',');
        }
        let d = record.diagnostic;
        line.push_str("{\"file\":\"");
        json_escape(record.file, line);
        line.push_str("\",\"key\":\"");
        json_escape(d.message_key(), line);
        line.push_str("\",\"code\":");
        line.push_str(&d.code().to_string());
        line.push_str(",\"pos\":");
        line.push_str(&d.pos().to_string());
        line.push_str(",\"end\":");
        line.push_str(&d.end().to_string());
        line.push_str(",\"args\":[");
        for (ai, arg) in d.message_args().iter().enumerate() {
            if ai > 0 {
                line.push(',');
            }
            line.push('"');
            json_escape(arg, line);
            line.push('"');
        }
        line.push_str("]}");
    }
    line.push_str("],\"program_errors\":");
    line.push_str(&program_errors.to_string());
    line.push_str(",\"unresolved\":");
    line.push_str(&unresolved.len().to_string());
    line.push_str(",\"shape_hash\":\"");
    line.push_str(&hash);
    line.push_str("\"}");
}

/// Builds the one-file program over the seeded VFS and binds it
/// (program_test.rs's `build_corpus_program` call shape, with the counting
/// host FS).
fn build_bound_program(
    corpus_abs: &str,
    root: &BenchFile,
    seed: &[BenchFile],
) -> (tsc_compiler::Program, Arc<CountingFs>) {
    let config_path =
        rooted_file_path_from_absolute(&format!("{corpus_abs}/__program__.tsconfig.json"));
    let mut config_text = String::from(r#"{"compilerOptions":{"target":"esnext"},"files":[""#);
    json_escape(root.file_name.as_string(), &mut config_text);
    config_text.push_str(r#""]}"#);

    let mut entries: Vec<(String, String)> = Vec::with_capacity(seed.len() + 1);
    for f in seed {
        entries.push((f.file_name.as_string().to_string(), f.text.clone()));
    }
    entries.push((config_path.as_string().to_string(), config_text));
    let fs = tsc_bundled::wrap_fs(tsc_vfs::vfstest::from_map(
        entries,
        CaseSensitivity::CaseSensitive,
    ));

    let config = get_parsed_command_line_of_config_file_path(&config_path, fs.as_ref(), None);

    let counting = Arc::new(CountingFs::new(fs));
    let host = new_compiler_host(
        Arc::clone(&counting) as Arc<dyn Fs>,
        tsc_bundled::lib_path(),
        nil_extended_config_cache(),
        None,
    );
    let mut program = new_program(ProgramOptions {
        config: ProgramConfig {
            config: Arc::new(config),
            ..Default::default()
        },
        hosts: ProgramHosts { host: Some(host) },
        factories: ProgramFactories::default(),
    });
    program.bind_source_files();
    (program, counting)
}

/// The ok:false NDJSON line (`stage` is "program"/"bind").
fn emit_failure_line(f: &BenchFile, line: &mut String, stage: &str, message: &str) {
    line.push_str("{\"file\":\"");
    json_escape(f.file_name.as_string(), line);
    line.push_str("\",\"ok\":false,\"error\":\"");
    json_escape(&format!("{stage}: {message}"), line);
    line.push_str("\"}");
}

/// Extracts the message from a Rust panic payload (truncated to 200 chars,
/// the gate's rule — a full panic payload is not a shared format).
fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    let msg = if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    };
    msg.chars().take(200).collect()
}

// ──────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── (a) the FNV-1a 64 fold against hand-computed vectors ──
    //
    // FNV-1a 64: h = 0xcbf29ce484222325; per byte: h ^= b; h *= 0x100000001b3
    // (published vectors: "a" 0xaf63dc4c8601ec8c, "b" 0xaf63df4c8601f1a5,
    // "foobar" 0x85944171f73967e8 — verified against the draft-eastlake
    // test vectors).

    #[test]
    fn fnv1a_fold_intermediate_vector() {
        let mut h: u64 = 0xcbf29ce484222325;
        fold(&mut h, "a");
        assert_eq!(h, 0xaf63dc4c8601ec8c);
        let mut h: u64 = 0xcbf29ce484222325;
        fold(&mut h, "b");
        assert_eq!(h, 0xaf63df4c8601f1a5);
        let mut h: u64 = 0xcbf29ce484222325;
        fold(&mut h, "foobar");
        assert_eq!(h, 0x85944171f73967e8);
        // the empty fold is the offset basis
        assert_eq!(shape_hash(&[]), format!("{:016x}", 0xcbf29ce484222325u64));
    }

    #[test]
    fn program_hash_recipe_vectors() {
        // Hand-computed (independently of this crate's fold) over the
        // documented line set:
        let lines = hash_lines(&HashInput {
            root: "/corpus/trivial.ts",
            config_errors: 0,
            loaded: &[],
            program_files: &[],
            bound: 0,
            symbols: 0,
            duplicates: 0,
            parse_diags: &[],
            program_errors: 0,
            unresolved: &[],
        });
        assert_eq!(lines.len(), 9);
        assert_eq!(shape_hash(&lines), "de8c587ddb1d7276");

        // With one diag record, one unresolved name:
        let sf = crate::parser_seam::parse_source_file(
            tsc_ast::SourceFileParseOptions::default(),
            "let x = ;",
            tsc_core::scriptkind::ScriptKind::TS,
        );
        let d = &sf.diagnostics()[0];
        let record = ParseDiagRecord {
            file: "/corpus/bad.ts",
            diagnostic: d,
        };
        let lines = hash_lines(&HashInput {
            root: "/corpus/trivial.ts",
            config_errors: 0,
            loaded: &[],
            program_files: &[],
            bound: 0,
            symbols: 0,
            duplicates: 0,
            parse_diags: &[record],
            program_errors: 1,
            unresolved: &["some-missing-package".to_string()],
        });
        // the diag fold line carries the diagnostic's real key/args
        assert!(lines
            .iter()
            .any(|line| line.starts_with("diag=/corpus/bad.ts|")));
        assert!(lines
            .iter()
            .any(|line| line == "unresolvedname=some-missing-package\n"));
        // ...and the vector is over the real diagnostic — assert the fold
        // is byte-stable for the fixed synthetic fields:
        let synthetic_lines = vec![
            "file=/corpus/trivial.ts\n".to_string(),
            "config_errors=0\n".to_string(),
            "loaded=0\n".to_string(),
            "parsed=0\n".to_string(),
            "bound=0\n".to_string(),
            "symbols=0\n".to_string(),
            "duplicates=0\n".to_string(),
            "diag=/corpus/bad.ts|Unterminated string literal.|1002|0|5|arg1\u{1}arg2\n"
                .to_string(),
            "program_errors=1\n".to_string(),
            "unresolved=1\n".to_string(),
            "unresolvedname=some-missing-package\n".to_string(),
        ];
        assert_eq!(shape_hash(&synthetic_lines), "62b2ff83cc733009");

        // Order sensitivity: swapping two diag lines changes the hash.
        let order_a = vec![
            "diag=a|k1|1|0|1|x\n".to_string(),
            "diag=b|k2|2|2|3|y\n".to_string(),
        ];
        let order_b = vec![
            "diag=b|k2|2|2|3|y\n".to_string(),
            "diag=a|k1|1|0|1|x\n".to_string(),
        ];
        assert_eq!(shape_hash(&order_a), "54b26999ac98471d");
        assert_eq!(shape_hash(&order_b), "dcd64cf635d0ab89");
        assert_ne!(shape_hash(&order_a), shape_hash(&order_b));
    }

    // ── (b) the counting FS records distinct successful reads only ──

    #[test]
    fn counting_fs_records_distinct_successful_reads() {
        let fs = tsc_vfs::vfstest::from_map(
            [("/src/a.ts".to_string(), "let a = 1;\n".to_string())],
            CaseSensitivity::CaseSensitive,
        );
        let counting = CountingFs::new(fs);
        let a = rooted_file_path_from_absolute("/src/a.ts");
        let missing = rooted_file_path_from_absolute("/src/missing.ts");
        assert!(counting.read_file(&a).is_some());
        assert!(counting.read_file(&a).is_some());
        assert!(counting.read_file(&missing).is_none());
        let mut loaded = counting.loaded_names();
        loaded.sort();
        assert_eq!(loaded, vec!["/src/a.ts".to_string()]);
    }

    // ── (c) a full one-file program over the bundled overlay ──
    //
    // The M5 gate itself, in miniature: `let x = 1;` loads, parses the
    // esnext lib chain, binds, reports zero parse diagnostics.

    #[test]
    fn builds_a_bound_program_over_the_bundled_lib_chain() {
        let root = BenchFile {
            bytes: 10,
            text: "let x = 1;\n".to_string(),
            path_key: CaseSensitivity::CaseSensitive
                .path_key(&rooted_file_path_from_absolute("/corpus/trivial.ts").as_path()),
            script_kind: tsc_core::scriptkind::ScriptKind::TS,
            file_name: rooted_file_path_from_absolute("/corpus/trivial.ts"),
        };
        let (program, counting) =
            build_bound_program("/corpus", &root, std::slice::from_ref(&root));
        let files = program.get_source_files();
        assert_eq!(program.get_config_file_parsing_diagnostics().len(), 0);
        // the esnext default lib chain is fully loaded (99 files — the
        // esnextLibs list program_test.go asserts) + the root
        assert_eq!(files.len(), 100);
        assert_eq!(files[0].file_name().as_string(), "bundled:///libs/lib.es5.d.ts");
        assert_eq!(files[99].file_name().as_string(), "/corpus/trivial.ts");
        // every file bound, symbols allocated, parse diagnostics clean
        let mut bound = 0;
        let mut symbols = 0i64;
        for file in files {
            assert!(file.is_bound());
            bound += 1;
            symbols += i64::from(file.payload().symbol_count);
        }
        assert_eq!(bound, 100);
        assert!(symbols > 0);
        assert!(program.get_program_diagnostics().is_empty());
        assert!(program.get_unresolved_imports().is_empty());
        assert_eq!(program.duplicate_source_files().len(), 0);
        // the loaded set covers the lib chain + the root (distinct paths)
        let mut loaded = counting.loaded_names();
        loaded.sort();
        assert!(loaded.contains(&"bundled:///libs/lib.es5.d.ts".to_string()));
        assert!(loaded.contains(&"/corpus/trivial.ts".to_string()));
        assert!(loaded.len() >= 100);
    }

    // ── (d) the missing-import fixture shape ──

    #[test]
    fn missing_bare_import_lands_in_unresolved_imports() {
        let root = BenchFile {
            bytes: 44,
            text: "import { lost } from \"totally-missing-package\";\nlet y = lost;\n".to_string(),
            path_key: CaseSensitivity::CaseSensitive
                .path_key(&rooted_file_path_from_absolute("/corpus/missing.ts").as_path()),
            script_kind: tsc_core::scriptkind::ScriptKind::TS,
            file_name: rooted_file_path_from_absolute("/corpus/missing.ts"),
        };
        let (program, _counting) =
            build_bound_program("/corpus", &root, std::slice::from_ref(&root));
        assert_eq!(
            program.get_unresolved_imports(),
            vec!["totally-missing-package".to_string()]
        );
        // relative specifiers never enter the set (Go's
        // IsExternalModuleNameRelative filter)
        let root2 = BenchFile {
            bytes: 30,
            text: "import x from \"./missing-sibling\";\nlet z = x;\n".to_string(),
            path_key: CaseSensitivity::CaseSensitive
                .path_key(&rooted_file_path_from_absolute("/corpus/relative.ts").as_path()),
            script_kind: tsc_core::scriptkind::ScriptKind::TS,
            file_name: rooted_file_path_from_absolute("/corpus/relative.ts"),
        };
        let (program2, _counting2) =
            build_bound_program("/corpus", &root2, std::slice::from_ref(&root2));
        assert!(program2.get_unresolved_imports().is_empty());
    }
}

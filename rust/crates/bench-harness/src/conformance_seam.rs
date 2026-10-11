//! `tsc-bench conformance` — the M6 conformance twin (SPEC §8 M6: the
//! staged baseline climb tracked in rust/CONFORMANCE.md).
//!
//! Go reference: `runConformance` in rust/bench/go-driver/conformance.go
//! (which mirrors rust/crates/tsc-testrunner's compiler_runner.rs — the
//! ported Go testrunner). For every enumerated compiler/conformance case
//! file (sorted full-path order, the skip list, the `-test.run` filter,
//! `--limit`), both drivers parse the case into @filename units, construct
//! the per-case tsconfig (the case's own options marshaled through the
//! generated table), build the program at the wired depth (`--stage
//! parse|bind|check` — since the W3 checker wave the check stage drives the
//! ported checker spine per in-test unit), collect the depth's diagnostics, render the `.errors.txt` baseline content, and
//! emit one NDJSON line per (case, configuration):
//! `{"case","baseline","verdict","errors_count","shape_hash"}` —
//! `diff(1)` over the two drivers' outputs is the verdict (the twin
//! discipline; the climb % is the match fraction vs
//! tsc/testdata/baselines/reference, tracked in rust/CONFORMANCE.md).
//!
//!   tsc-bench conformance --corpus <dir> --baselines <dir>
//!       [--suite name] [--limit N] [--filter pattern]
//!       [--stage parse|bind|check] [--local-root <dir>]
//!
//! Record contract (both drivers byte-identical):
//!   - `case`: the case path relative to the corpus root (posix).
//!   - `baseline`: `<suite>/<configuredName>.errors.txt` (the reference
//!     tree's relative path).
//!   - `verdict`: `match` (content equals the reference, or both sides
//!     expect none) | `mismatch` (reference exists and differs) |
//!     `missing-baseline` (content but no reference — baseline-accept
//!     territory). `errors_count: -1` is the panic sentinel.
//!   - `errors_count`: the diagnostics count at the wired depth
//!     (tsconfig-parse + per-file parse + program diagnostics).
//!   - `shape_hash`: FNV-1a 64 (lowercase 16-hex) over the case, baseline,
//!     errors count and one fold line per diagnostic in the render sort
//!     order (the recipe in tsc-testrunner's compiler_runner.rs).
//!
//! Summary lines go to STDERR (stdout stays pure NDJSON for the diff).

use std::path::PathBuf;

use tsc_testrunner::{CaseRecord, ConformanceConfig, Stage};

/// Entry point for `tsc-bench conformance ...`.
pub fn run_conformance(args: &[String]) {
    let mut corpus = String::new();
    let mut baselines = String::new();
    let mut suite = String::new();
    let mut limit: usize = 0;
    let mut filter = String::new();
    let mut stage = Stage::Bind;
    let mut local_root: Option<PathBuf> = None;
    let mut dump_filter = String::new();
    let mut i = 0;
    while i < args.len() {
        let (name, inline) = match args[i].split_once('=') {
            Some((name, value)) => (name.trim_start_matches('-').to_string(), Some(value.to_string())),
            None => (args[i].trim_start_matches('-').to_string(), None),
        };
        let value = match inline {
            Some(value) => value,
            None => {
                i += 1;
                match args.get(i) {
                    Some(value) => value.clone(),
                    None => {
                        eprintln!("tsc-bench conformance: flag needs an argument: -{name}");
                        std::process::exit(2);
                    }
                }
            }
        };
        match name.as_str() {
            "corpus" => corpus = value,
            "baselines" => baselines = value,
            "suite" => suite = value,
            "limit" => match value.parse::<usize>() {
                Ok(parsed) => limit = parsed,
                Err(err) => {
                    eprintln!("tsc-bench conformance: invalid --limit {value:?}: {err}");
                    std::process::exit(2);
                }
            },
            "filter" => filter = value,
            "stage" => match Stage::parse(&value) {
                Ok(parsed) => stage = parsed,
                Err(err) => {
                    eprintln!("tsc-bench conformance: {err}");
                    std::process::exit(2);
                }
            },
            "local-root" => local_root = Some(PathBuf::from(&value)),
            "dump-filter" => dump_filter = value,
            other => {
                eprintln!("tsc-bench conformance: flag provided but not defined: -{other}");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    if corpus.is_empty() || baselines.is_empty() {
        eprintln!("tsc-bench conformance: missing required --corpus <dir> and --baselines <dir>");
        std::process::exit(2);
    }
    if suite.is_empty() {
        suite = "conformance".to_string();
    }

    let cases_root = match std::path::absolute(&corpus) {
        Ok(path) => path,
        Err(err) => {
            eprintln!("tsc-bench: resolving corpus dir: {err}");
            std::process::exit(1);
        }
    };
    let baselines_root = match std::path::absolute(&baselines) {
        Ok(path) => path,
        Err(err) => {
            eprintln!("tsc-bench: resolving baselines dir: {err}");
            std::process::exit(1);
        }
    };

    let config = ConformanceConfig {
        cases_root,
        baselines_root,
        suite,
        limit,
        filter,
        stage,
        local_root,
    };
    let mut stdout = std::io::stdout().lock();
    // Cell counters keep the sink an FnMut over shared state (the runner
    // owns the call loop; the summary reads them afterwards).
    use std::cell::Cell;
    let matched = Cell::new(0usize);
    let mismatched = Cell::new(0usize);
    let missing = Cell::new(0usize);
    let panics = Cell::new(0usize);
    let records = Cell::new(0usize);
    let mut emit = |record: &CaseRecord| {
        use std::io::Write;
        writeln!(stdout, "{}", record.to_ndjson()).expect("write record");
        // The W3 check-stage divergence bisect read: print the shape_hash's
        // fold lines for matching cases to STDERR (stdout stays pure NDJSON;
        // the go-driver twin implements the same flag on the same lines).
        if !dump_filter.is_empty() && record.case.contains(&dump_filter) {
            let mut stderr = std::io::stderr().lock();
            for line in record.dump_lines.iter().flatten() {
                let _ = write!(stderr, "DUMP {line}");
            }
        }
        records.set(records.get() + 1);
        match record.verdict {
            tsc_testrunner::Verdict::Match => matched.set(matched.get() + 1),
            tsc_testrunner::Verdict::Mismatch => {
                if record.errors_count < 0 {
                    panics.set(panics.get() + 1);
                }
                mismatched.set(mismatched.get() + 1);
            }
            tsc_testrunner::Verdict::MissingBaseline => missing.set(missing.get() + 1),
        }
    };
    if let Err(err) = tsc_testrunner::run_conformance(&config, &mut emit) {
        eprintln!("tsc-bench conformance: {err}");
        std::process::exit(2);
    }
    let (matched, mismatched) = (matched.get(), mismatched.get());
    let (missing, panics, records) = (missing.get(), panics.get(), records.get());
    eprintln!(
        "conformance: {records} cases, {matched} match ({:.1}%), {mismatched} mismatch, {missing} missing-baseline, {panics} panic",
        if records > 0 { matched as f64 * 100.0 / records as f64 } else { 0.0 },
    );
}

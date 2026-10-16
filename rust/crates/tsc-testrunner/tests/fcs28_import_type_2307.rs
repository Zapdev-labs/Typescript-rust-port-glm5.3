//! FCS28 pins (full-corpus check sweep, tranche 28): the TS2307
//! module-not-found residual family — the import-type half.
//!
//! Root cause: the checker's `getTypeFromImportTypeNode` used the PUBLIC
//! `ResolveExternalModuleName` carrier (exports.rs), which hard-codes
//! ignoreErrors=true — while Go's checker.go:25116 calls the INTERNAL
//! resolver with ignoreErrors=false plus the import-attributes type
//! (`getTypeFromImportAttributes(ast.GetImportAttributes(node))`). Every
//! import-type node's module-not-found diagnostic was silently swallowed
//! (the single largest residual pool: 340 rust-missing TS2307 instances
//! across 81 records → 24 after the fix). The same tranche landed the
//! checkImportType attributes half (Go checker.go:3375-3383):
//! checkGrammarImportAttributeValues + the reporting
//! getResolutionModeOverride (TS1453 on a resolution-mode value that is
//! neither "import" nor "require").
//!
//! The records below are the GO twin's exact values (rig census 2026-10-15,
//! the t28d_full_go stream) — all six records moved from rust-missing-2307
//! divergent to byte-identical.

use std::path::PathBuf;

use tsc_testrunner::{CaseRecord, ConformanceConfig, Stage, Verdict};

fn verdict_str(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Match => "match",
        Verdict::Mismatch => "mismatch",
        Verdict::MissingBaseline => "missing-baseline",
    }
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..").canonicalize().unwrap()
}

fn run_filter(filter: &str) -> Vec<(String, String, i64, String)> {
    let config = ConformanceConfig {
        cases_root: repo_root().join("tsc/testdata/tests/cases/conformance"),
        baselines_root: repo_root().join("tsc/testdata/baselines/reference"),
        suite: "conformance".to_string(),
        limit: 0,
        filter: filter.to_string(),
        stage: Stage::Check,
        local_root: None,
    };
    let mut records: Vec<(String, String, i64, String)> = Vec::new();
    tsc_testrunner::run_conformance(&config, |record: &CaseRecord| {
        records.push((
            record.baseline.clone(),
            verdict_str(record.verdict).to_string(),
            record.errors_count,
            record.shape_hash.to_string(),
        ));
    })
    .expect("conformance run");
    records
}

#[track_caller]
fn assert_twin_records(filter: &str, expected: &[(&str, &str, i64, &str)]) {
    let records = run_filter(filter);
    assert_eq!(records.len(), expected.len(), "record count for filter {filter:?}");
    for (record, want) in records.iter().zip(expected) {
        assert_eq!(
            (record.0.as_str(), record.1.as_str(), record.2, record.3.as_str()),
            *want,
            "record {} for filter {filter:?}",
            record.0
        );
    }
}

/// The keystone cluster: node/nodeModules{ImportAttributes,}TypeMode*
/// — `import("pkg", { with: { "resolution-mode": ... } })` type nodes.
/// Before the fix the rust twin emitted NO TS2307 for the unresolvable
/// bare `pkg` and NO TS1453 for the "foobar" resolution-mode values
/// (129 vs 152 errors). All 4 module variants byte-identical now.
#[test]
fn fcs28_type_mode_declaration_emit_errors_2307_1453_pin() {
    assert_twin_records(
        "node/nodeModulesImportAttributesTypeModeDeclarationEmitErrors.ts",
        &[
            ("conformance/nodeModulesImportAttributesTypeModeDeclarationEmitErrors(module=node16).errors.txt", "mismatch", 152, "c1df439942ec7d82"),
            ("conformance/nodeModulesImportAttributesTypeModeDeclarationEmitErrors(module=node18).errors.txt", "mismatch", 152, "8199f3651dad97a4"),
            ("conformance/nodeModulesImportAttributesTypeModeDeclarationEmitErrors(module=node20).errors.txt", "mismatch", 152, "7bea9377978cc7f9"),
            ("conformance/nodeModulesImportAttributesTypeModeDeclarationEmitErrors(module=nodenext).errors.txt", "mismatch", 152, "c93780aa9e54bb90"),
        ],
    );
}

/// salsa/moduleExportAssignment7 — 28 `import('./mod')` import-type
/// references across index.ts type annotations and main.js JSDoc @param
/// tags; the rust twin emitted ZERO of the 58 errors' 2307 half before
/// (30 vs 58).
#[test]
fn fcs28_module_export_assignment7_import_type_2307_pin() {
    assert_twin_records(
        "salsa/moduleExportAssignment7.ts",
        &[(
            "conformance/moduleExportAssignment7.errors.txt",
            "mismatch",
            58,
            "0bad35799ff893c7",
        )],
    );
}

/// types/import/importTypeLocal — the import-type resolution keystone
/// (local-qualifier import types over an unresolvable relative specifier).
/// (The `-test.run` filter reads only the second `/` component — the
/// basename — and its literal prefix stops at the regex metachar `.`, so
/// `x/importTypeLocal.ts` also selects importTypeLocalMissing.ts — the
/// same family's cluster-5 record; both are pinned.)
#[test]
fn fcs28_import_type_local_2307_pin() {
    assert_twin_records(
        "x/importTypeLocal.ts",
        &[
            ("conformance/importTypeLocal.errors.txt", "mismatch", 35, "69a4bda9e872acdc"),
            ("conformance/importTypeLocalMissing.errors.txt", "mismatch", 36, "aa45ed78b6784dcd"),
        ],
    );
}

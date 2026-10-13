//! FCS6 pins (full-corpus check sweep, tranche 6): the count-diff families
//! this tranche collapsed —
//!
//! 1. The machinery TS2318 family (moduleResolution/bundler +
//!    node/nodeModules* shape): the per-unit semantic seam
//!    (`semantic_diagnostics_of_unit`) used ensure_script_kind
//!    (Unknown forced to TS) where Go's confSemanticDiagnosticsOfUnit
//!    switches on GetScriptKindFromFileName and returns nil for
//!    ScriptKindUnknown — extensionless (`/project/e`) and unsupported-
//!    extension (`/project/e.txt`) units were checked, emitting the
//!    initializeTypeChecker machinery TS2318s (Array/Boolean/Object/...,
//!    10 per unit per config variant) Go never reports (532 rust-extra
//!    instances on the tranche-5 corpus).
//!
//! 2. The TS2834/2835 + TS2307 module-not-found ladder
//!    (node/nodeModules1 shape): `resolveExternalModule`'s tail ladder
//!    (Go checker.go:15690-15746) is REAL now — the TS2732 json arm, the
//!    extensionless-ESM arms (TS2835 "Did you mean './index.mjs'?" /
//!    TS2834 "Consider adding an extension") through the real mode
//!    derivation (the context-specifier walk + the emit-syntax worker:
//!    a .cts file's dynamic `import()` is ESM-mode under node16), then
//!    the plain 2307. The tranche-5 corpus carried 47 extra TS2307s +
//!    40 missing TS2834/2835s on nodeModules1 alone. NOTE: nodeModules1's
//!    own twin records still diverge by 7 (Go's per-unit program resolves
//!    "./index.js" → the unit itself via node16 extension substitution —
//!    the per-unit resolveModuleName seam is the next tranche's family);
//!    the ladder is pinned at unit scale in
//!    checker/tests/fcs6_module_not_found_ladder.rs instead.
//!
//! 3. The TS2683 implicit-this family (types/thisType/
//!    thisTypeInObjectLiterals2 shape): getContextualThisParameterType
//!    (checker.go:12260) is REAL now — the containing-object-literal arm
//!    gives object-literal methods their own (widened) literal type as
//!    'this' under noImplicitThis (78 rust-extra 2683s on this case).
//!    NOTE: thisTypeInObjectLiterals2's own twin records still diverge
//!    (downstream widened-literal-type/parameter-inference residuals:
//!    TS7006/TS2339/TS18046 — the next tranche's family); the 2683
//!    family is pinned by the comparisonOperator records' exactness and
//!    the checker-side ladder pins.
//!
//! 4. The TS2367 no-overlap family (expressions/binaryOperators/
//!    comparisonOperator shape): isTypeEqualityComparableTo
//!    (checker.go:13105) is REAL now — `(target.flags &
//!    TypeFlagsNullable) != 0 || isTypeComparableTo(source, target)`; the
//!    W-era approximation (check_type_comparable_to both directions)
//!    over-reported "no overlap" on every nullable-target comparison
//!    (64 rust-extra 2367s on the IsNull case).

use std::path::PathBuf;

use tsc_testrunner::{CaseRecord, ConformanceConfig, Stage, Verdict};

/// The twin's verdict vocabulary (`Verdict::as_str` is the NDJSON writer's
/// own private method; the vocabulary is the twin contract).
fn verdict_str(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Match => "match",
        Verdict::Mismatch => "mismatch",
        Verdict::MissingBaseline => "missing-baseline",
    }
}

/// The repo root (tsc/ + the corpus trees live there).
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

/// Runs the filter and asserts the exact (baseline, verdict, errors_count,
/// shape_hash) set — the Go twin's records for the same cases (rig census
/// 2026-10-14, the fcs6-dumps1 go stream).
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

/// The machinery TS2318 family: bundlerImportTsExtensions (the /project/e
/// and /project/e.txt units; 80 rust-extra TS2318s pre-fix). The records
/// are the GO twin's exact values — 4 config variants
/// (allowImportingTsExtensions x noEmit).
#[test]
fn bundler_import_ts_extensions_2318_family_pin() {
    assert_twin_records(
        "conformance/^bundlerImportTsExtensions",
        &[
            ("conformance/bundlerImportTsExtensions(allowimportingtsextensions=false,noemit=false).errors.txt", "mismatch", 132, "aa32f8954db2bb75"),
            ("conformance/bundlerImportTsExtensions(allowimportingtsextensions=false,noemit=true).errors.txt", "mismatch", 129, "391f51861ca6ba76"),
            ("conformance/bundlerImportTsExtensions(allowimportingtsextensions=true,noemit=false).errors.txt", "mismatch", 133, "9f065e40135e7c6a"),
            ("conformance/bundlerImportTsExtensions(allowimportingtsextensions=true,noemit=true).errors.txt", "mismatch", 129, "8494c4b574b57c39"),
        ],
    );
}

/// The TS2367 no-overlap family: comparisonOperatorWithOneOperandIsNull —
/// every `x == null` / `null == x` comparison is equality-comparable (the
/// nullable-target short-circuit); the W-era approximation reported "no
/// overlap" on all of them (64 rust-extra 2367s pre-fix).
#[test]
fn comparison_operator_null_2367_family_pin() {
    assert_twin_records(
        "conformance/^comparisonOperatorWithOneOperandIsNull",
        &[(
            "conformance/comparisonOperatorWithOneOperandIsNull.errors.txt",
            "mismatch",
            74,
            "94a9b990f67479fa",
        )],
    );
}

/// Same family, the undefined-operand twin (56 rust-extra 2367s pre-fix).
#[test]
fn comparison_operator_undefined_2367_family_pin() {
    assert_twin_records(
        "conformance/^comparisonOperatorWithOneOperandIsUndefined",
        &[(
            "conformance/comparisonOperatorWithOneOperandIsUndefined.errors.txt",
            "mismatch",
            170,
            "cbc5c43a004f95b4",
        )],
    );
}

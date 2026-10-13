//! FCS5 pins (full-corpus check sweep, tranche 5): the count-diff families
//! this tranche collapsed —
//!
//! 1. The phantom Duplicate_identifier_2300 family
//!    (es6/computedProperties/computedPropertyNames10_ES5.ts shape): the
//!    port's `get_property_name_for_property_name_node` returned "" where
//!    Go returns the `InternalSymbolNameMissing` sentinel for non-literal-
//!    like computed names, so `getEffectivePropertyNameForPropertyName
//!    Node`'s gate treated every computed name as the REAL (empty) name and
//!    `checkGrammarObjectLiteralExpression`'s seen map collided them all on
//!    "" — one phantom TS2300 per computed-name object-literal method (8
//!    extra diagnostics on this case; Go emits none). The fix restores the
//!    sentinel + ports Go checker.go:19126's
//!    tryGetNameFromType(getTypeOfExpression) real path.
//!
//! 2. The TS2711 dynamic-import family: `checkImportCallExpression` now
//!    funnels every exit through `createPromiseReturnType` (Go
//!    checker.go:8486) and `createPromiseReturnType` carries Go's
//!    `core.IfElse(ast.IsImportCall(fn), ...)` message split — the
//!    A_dynamic_import_call_returns_a_Promise diagnostics (824 missing on
//!    the tranche-4 corpus) fire natively.
//!
//! 3. The native config-diag spans (tsoptions): `validateJsonOptionValue` /
//!    `convertJsonOptionOfEnumType` / `convertJsonOption` carry Go's
//!    AST params (propertyAssignment.Initializer + sourceFile), so the
//!    TS5024/TS6046 config-route diagnostics carry the real initializer
//!    span natively (the tranche-4 runner seam reconstruction now no-ops).
//!    The fcs4_config_span pins keep asserting the same record values —
//!    the native span equals the reconstructed one byte-for-byte; the
//!    autoAccessor6-style cases are covered there, so this file pins the
//!    computedPropertyNames10 + dynamicImport families.

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
/// shape_hash) set — the Go twin's records for the same cases.
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

/// computedPropertyNames10_ES5 (the computed-name method zoo): pre-fix the
/// port emitted 8 phantom TS2300s (10/19 vs Go's identical records were
/// 18/19); the records below are the GO twin's exact post-fix values, which
/// the Rust runner now produces byte-for-byte (rig census 2026-10-13).
#[test]
fn computed_property_names10_2300_family_pin() {
    assert_twin_records(
        "conformance/^computedPropertyNames10_ES5",
        &[
            ("conformance/computedPropertyNames10_ES5(target=es2015).errors.txt", "missing-baseline", 10, "05cf9c87d5870101"),
            ("conformance/computedPropertyNames10_ES5(target=es5).errors.txt", "missing-baseline", 11, "310294e7ddc2d74c"),
        ],
    );
}

/// The TS2711 family: dynamicImport/importCallExpression1ES2020.ts — the
/// A_dynamic_import_call_returns_a_Promise diagnostics (one per dynamic
/// import call site in a Promise-less lib config) were MISSING on the whole
/// tranche-4 corpus (824 instances); the createPromiseReturnType funnel
/// emits them natively now.
#[test]
fn dynamic_import_promise_2711_family_pin() {
    assert_twin_records(
        "conformance/^importCallExpression1ES2020",
        &[(
            "conformance/importCallExpression1ES2020.errors.txt",
            "missing-baseline",
            30,
            "f726e18574a8ed58",
        )],
    );
}

/// Same family, the check-return-type case (a mismatch record whose twin
/// errors_count includes the 2711s).
#[test]
fn dynamic_import_promise_2711_check_returntype_pin() {
    assert_twin_records(
        "conformance/^importCallExpressionCheckReturntype1",
        &[(
            "conformance/importCallExpressionCheckReturntype1.errors.txt",
            "mismatch",
            42,
            "eb4e382607c0265c",
        )],
    );
}

/// Same family, importCallExpression4ES2020 (the largest of the
/// importCallExpression ES2020 quad).
#[test]
fn dynamic_import_promise_2711_family_pin_4() {
    assert_twin_records(
        "conformance/^importCallExpression4ES2020",
        &[(
            "conformance/importCallExpression4ES2020.errors.txt",
            "missing-baseline",
            40,
            "e84103c18632fc7d",
        )],
    );
}

//! FCS4 pins (full-corpus check sweep, tranche 4): the 26-case
//! tsbaseline.rs:273 panic family — config-route option diagnostics
//! (TS5024 boolean / TS6046 enum) that arrived file-paired to the synthetic
//! tsconfig with UndefinedTextRange (-1,-1) spans and panicked the
//! baseline's column slice. The tsoptions port dropped the AST params from
//! `validateJsonOptionValue`/`convertJsonOptionOfEnumType` ("command-line
//! path only" wave), so the runner seam re-derives Go's `valueExpression`
//! loc from the byte-identical config text (compiler_runner.rs
//! `file_diagnostic_view` → `config_option_value_loc`).
//!
//! Each pin asserts the EXACT twin record (verdict, errors_count,
//! shape_hash) the Go driver (rust/bench/go-driver) produces for the same
//! corpus case — the shape_hash fold includes every diagnostic's
//! `file|key|code|pos|end|args`, so the pins pin the RECONSTRUCTED span
//! (Go's `propertyAssignment.Initializer` loc) transitively.
//!
//! The remaining corpus divergence in these families (decoratorExpression.2
//! 11-vs-13, autoAccessor11, builtinIteratorReturn 25-vs-26,
//! arbitraryModuleNamespaceIdentifiers_module 21-vs-14, ...) is the
//! count-diff census, not this span family — the pins cover only the
//! panic-recovered records.

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

/// Runs the filter and asserts ONE record appears with the exact twin
/// values (for cases whose filter widens — the `-test.run` literal-prefix
/// decoder stops at regex metacharacters, so `^usingDeclarations.12`'s
/// prefix is `usingDeclarations`).
#[track_caller]
fn assert_twin_record_in(filter: &str, baseline: &str, verdict: &str, count: i64, hash: &str) {
    let records = run_filter(filter);
    let found = records.iter().find(|record| record.0 == baseline);
    let Some(found) = found else {
        panic!("no record for {baseline:?} under filter {filter:?}");
    };
    assert_eq!(
        (found.1.as_str(), found.2, found.3.as_str()),
        (verdict, count, hash),
        "record {baseline:?} under filter {filter:?}"
    );
}

/// The `@useDefineForClassFields: *` family: the `"*"` string in the
/// synthetic tsconfig fails boolean validation (TS5024) — Go's diagnostic
/// carries the `"*"` initializer's loc; pre-fix the port's (-1,-1) file-
/// paired twin panicked the render (3 of the 26).
#[test]
fn auto_accessor6_span_pin() {
    assert_twin_records(
        "conformance/^autoAccessor6",
        &[
            ("conformance/autoAccessor6(target=es2015).errors.txt", "missing-baseline", 12, "b44e4c01bafd34aa"),
            ("conformance/autoAccessor6(target=es2022).errors.txt", "missing-baseline", 12, "bcf640129d9f0a24"),
            ("conformance/autoAccessor6(target=esnext).errors.txt", "missing-baseline", 12, "0f85508eebf8ac19"),
        ],
    );
}

/// Same family (`@useDefineForClassFields: *`, 3 more of the 26).
#[test]
fn auto_accessor7_span_pin() {
    assert_twin_records(
        "conformance/^autoAccessor7",
        &[
            ("conformance/autoAccessor7(target=es2015).errors.txt", "missing-baseline", 11, "ce51405a888ab81f"),
            ("conformance/autoAccessor7(target=es2022).errors.txt", "missing-baseline", 11, "8cf3bb4ed922712d"),
            ("conformance/autoAccessor7(target=esnext).errors.txt", "missing-baseline", 11, "7799614bf8200fd8"),
        ],
    );
}

/// The TS6046 enum arm (`@target: *,-es3` marshals the `"*"` — the
/// Argument_for_0_option_must_be diagnostic carries the `"*"` initializer
/// loc; 1 of the 26).
#[test]
fn call_chain_with_super_span_pin() {
    assert_twin_records(
        "conformance/^callChainWithSuper",
        &[(
            "conformance/callChainWithSuper.errors.txt",
            "missing-baseline",
            11,
            "3b5bb1348bed80f5",
        )],
    );
}

/// The TS5024 boolean arm via usingDeclarations.12 (`@useDefineForClassFields:
/// *`; 1 of the 26 — the record is matched inside the widened filter).
#[test]
fn using_declarations_12_span_pin() {
    assert_twin_record_in(
        "conformance/^usingDeclarations.12",
        "conformance/usingDeclarations.12.errors.txt",
        "missing-baseline",
        11,
        "df05c820dea9a887",
    );
}

/// The TS5024 boolean arm (`@strict: *` on the JS case; 1 of the 26).
#[test]
fn call_with_missing_void_undefined_unknown_any_in_js_span_pin() {
    assert_twin_records(
        "conformance/^callWithMissingVoidUndefinedUnknownAnyInJs",
        &[(
            "conformance/callWithMissingVoidUndefinedUnknownAnyInJs.errors.txt",
            "missing-baseline",
            47,
            "2c8b86784d074f6d",
        )],
    );
}

/// The `@useDefineForClassFields: *` family (4 of the 26;
/// esDecorators-classDeclaration-parameterProperties).
#[test]
fn es_decorators_parameter_properties_span_pin() {
    assert_twin_records(
        "conformance/^esDecorators-classDeclaration-parameterProperties",
        &[
            ("conformance/esDecorators-classDeclaration-parameterProperties(target=es2015).errors.txt", "missing-baseline", 11, "4f23468fa58baac7"),
            ("conformance/esDecorators-classDeclaration-parameterProperties(target=es2022).errors.txt", "missing-baseline", 11, "5f0b05750283f5a9"),
            ("conformance/esDecorators-classDeclaration-parameterProperties(target=es5).errors.txt", "missing-baseline", 12, "915d8ce786949967"),
            ("conformance/esDecorators-classDeclaration-parameterProperties(target=esnext).errors.txt", "missing-baseline", 11, "4d9d64483de80594"),
        ],
    );
}

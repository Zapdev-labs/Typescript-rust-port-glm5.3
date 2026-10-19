//! FCS59 record pins (full-corpus check sweep, micro-tranche 59 — the
//! "FCS tranche 58" census epoch's queued targets): the record-level
//! twins for the tranche's clusters —
//!
//!  * the TS7055 generator-yield family (generators/
//!    generatorReturnTypeInferenceNonStrict): the port reported TS7010
//!    "return type" x4 where Go reports TS7055 "yield type" (the
//!    function-family implicit-any arm selects the YIELD message for
//!    WideningKind.GeneratorYield, checker.go:18743; plus the Reparsed
//!    overload arm, checker.go:18736-18742).
//!
//!  * the TS2536 intersection-indexed-access family (types/intersection/
//!    intersectionReduction + intersectionReductionStrict + the keyof
//!    collaterals): Go's everyType in checkIndexedAccessIndexType
//!    distributes over UNION constituents only — an intersection index
//!    type checks as a whole; the port walked intersection members
//!    individually so the mapped-type `U[P & keyof U]` access over-fired
//!    TS2536.
//!
//!  * the TS7022 loop-carried-initializer family (controlFlow/
//!    controlFlowIterationErrors + controlFlowIterationErrorsAsync): the
//!    getQuickTypeOfExpression CALL arm re-landing (checker.go:7573-
//!    7583) resolves un-annotated `let y = f(x)` initializers without
//!    re-checking the arguments (no mid-walk flow-state reset), so the
//!    loop-carried circularity never forms.
//!
//!  * the TS1117 spread-seam collaterals (types/spread/objectSpreadNegative
//!    both target variants + types/members/duplicatePropertyNames): the
//!    re-landing exposed the tranche-26 "spread seam" — the 1117
//!    duplicate-property grammar arm early-returned after the first
//!    report where Go (grammarchecks.go:1121) CONTINUES, so every later
//!    duplicate property now reports too.
//!
//! ("FCS tranche 58" PORT notes in-tree; pins named fcs59 by census epoch.)

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
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .unwrap()
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
    assert_eq!(
        records.len(),
        expected.len(),
        "record count for filter {filter:?}"
    );
    for (record, want) in records.iter().zip(expected) {
        assert_eq!(
            (record.0.as_str(), record.1.as_str(), record.2, record.3.as_str()),
            *want,
            "record {} for filter {filter:?}",
            record.0
        );
    }
}

/// The generator-yield record: TS7055 "yield type" where the port
/// over-reported TS7010 "return type" x4.
#[test]
fn fcs59_generator_yield_type_records_pin() {
    assert_twin_records(
        "generators/^generatorReturnTypeInferenceNonStrict\\.ts",
        &[(
            "conformance/generatorReturnTypeInferenceNonStrict.errors.txt",
            "mismatch",
            24,
            "960c9dfa16c1fe6b",
        )],
    );
}

/// The intersection-indexed-access records: the mapped-type
/// `U[P & keyof U]` access no longer over-fires TS2536.
#[test]
fn fcs59_intersection_reduction_records_pin() {
    assert_twin_records(
        "intersection/^intersectionReduction\\.ts",
        &[(
            "conformance/intersectionReduction.errors.txt",
            "mismatch",
            13,
            "c94d96a3b665b2bf",
        )],
    );
    assert_twin_records(
        "intersection/^intersectionReductionStrict\\.ts",
        &[(
            "conformance/intersectionReductionStrict.errors.txt",
            "mismatch",
            15,
            "5307910533903267",
        )],
    );
}

/// The loop-carried-initializer records: the h3/h4-style
/// `let y = f(x); x = y + 1` loops no longer over-report TS7022 (and the
/// async twin no longer the await-shaped x5 variant).
#[test]
fn fcs59_control_flow_iteration_errors_records_pin() {
    assert_twin_records(
        "controlFlow/^controlFlowIterationErrors\\.ts",
        &[(
            "conformance/controlFlowIterationErrors.errors.txt",
            "mismatch",
            15,
            "752ab59980b49a80",
        )],
    );
    assert_twin_records(
        "controlFlow/^controlFlowIterationErrorsAsync\\.ts",
        &[(
            "conformance/controlFlowIterationErrorsAsync.errors.txt",
            "mismatch",
            29,
            "a20b12a8c9f6b2fd",
        )],
    );
}

/// The TS1117 spread-seam collaterals: objectSpreadNegative (both target
/// variants — the es5 variant's baseline is missing upstream, the
/// verdict is the twin's own) + duplicatePropertyNames.
#[test]
fn fcs59_spread_duplicate_1117_records_pin() {
    assert_twin_records(
        "spread/^objectSpreadNegative\\.ts",
        &[
            (
                "conformance/objectSpreadNegative(target=es2015).errors.txt",
                "mismatch",
                31,
                "0d980e6e53589f18",
            ),
            (
                "conformance/objectSpreadNegative(target=es5).errors.txt",
                "missing-baseline",
                32,
                "8fcce09b25282f00",
            ),
        ],
    );
    assert_twin_records(
        "members/^duplicatePropertyNames\\.ts",
        &[(
            "conformance/duplicatePropertyNames.errors.txt",
            "mismatch",
            23,
            "b612f2b6339b95f3",
        )],
    );
}

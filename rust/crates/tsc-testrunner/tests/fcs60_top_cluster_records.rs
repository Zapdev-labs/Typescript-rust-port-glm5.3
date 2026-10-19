//! FCS60 record pins (full-corpus check sweep, micro-tranche 59 — the "FCS
//! tranche 59" PORT notes in-tree; pins named fcs60 by census epoch): the
//! TS2345 inference-candidate family — getCommonSupertype's
//! strictNullChecks arm (filterType + getNullableType re-addition), the
//! union-of-fresh-literals sibling optional-undefined widening
//! (getPropertiesOfContext/getSiblingsOfContext), and newAnonymousType's
//! getNamedMembers ordering. 9 records converged (2345 rust-extras gone),
//! 0 newly-divergent.

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

/// The a9e/arr records: the strictNullChecks common-supertype arm — the
/// extra TS2345s on the later arguments of [undefined, {x,z}, {x,y}]
/// candidate lists are gone.
#[test]
fn fcs60_type_argument_inference_2345_records_pin() {
    assert_twin_records(
        "functionCalls/^typeArgumentInference\\.ts",
        &[(
            "conformance/typeArgumentInference.errors.txt",
            "mismatch",
            24,
            "47af71c7d8ec551c",
        )],
    );
    assert_twin_records(
        "functionCalls/^typeArgumentInferenceWithConstraints\\.ts",
        &[(
            "conformance/typeArgumentInferenceWithConstraints.errors.txt",
            "mismatch",
            32,
            "01bbbedb82bb1b6b",
        )],
    );
}

/// The tagged-template records (both target variants).
#[test]
fn fcs60_tagged_template_2345_records_pin() {
    assert_twin_records(
        "templates/^taggedTemplateStringsTypeArgumentInference\\.ts",
        &[(
            "conformance/taggedTemplateStringsTypeArgumentInference.errors.txt",
            "mismatch",
            30,
            "e2993a2128b3c8ea",
        )],
    );
    assert_twin_records(
        "templates/^taggedTemplateStringsTypeArgumentInferenceES6\\.ts",
        &[(
            "conformance/taggedTemplateStringsTypeArgumentInferenceES6.errors.txt",
            "mismatch",
            30,
            "0e73628ba53e50ae",
        )],
    );
}

/// The genericCallWithFunctionTypedArguments2 record (the `a: null`
/// signature-inference shape — same candidate family).
#[test]
fn fcs60_generic_call_with_function_typed_arguments2_records_pin() {
    assert_twin_records(
        "typeInference/^genericCallWithFunctionTypedArguments2\\.ts",
        &[(
            "conformance/genericCallWithFunctionTypedArguments2.errors.txt",
            "mismatch",
            13,
            "1143d6d5884a6a3f",
        )],
    );
}

/// The destructuringParameterProperties3 record — the widened-literal
/// member ordering collateral.
#[test]
fn fcs60_destructuring_parameter_properties3_records_pin() {
    assert_twin_records(
        "destructuring/^destructuringParameterProperties3\\.ts",
        &[(
            "conformance/destructuringParameterProperties3.errors.txt",
            "mismatch",
            17,
            "a39536ae6f136686",
        )],
    );
}

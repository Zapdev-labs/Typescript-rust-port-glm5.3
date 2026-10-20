//! FCS65 record pins (full-corpus check sweep, micro-tranche 64 — the
//! "FCS tranche 64" PORT notes in-tree; pins named fcs65 by census
//! epoch): the conditional-relation true-type target family —
//! `structured_type_related_to_worker`'s assignable conditional-source
//! arm now passes `getTrueTypeFromConditionalType(target)` (Go
//! relater.go:3785) instead of the whole conditional target.
//! conditionalTypes1 CONVERGED (70 -> 64 = the Go count);
//! conditionalTypes2 44 -> 42; unionAndIntersectionInference3
//! 25 -> 24; 0 newly-divergent versus the T63 epoch.

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

/// The conditionalTypes1 record — the #21756/#21823 family (the TS2322
/// x6 rust-extras gone; 64 = the Go count).
#[test]
fn fcs65_conditional_types1_records_pin() {
    assert_twin_records(
        "conditional/^conditionalTypes1\\.ts",
        &[(
            "conformance/conditionalTypes1.errors.txt",
            "mismatch",
            64,
            "902db70d4822e618",
        )],
    );
}

/// The conditionalTypes2 record (44 -> 42 with the true-type target fix).
#[test]
fn fcs65_conditional_types2_records_pin() {
    assert_twin_records(
        "conditional/^conditionalTypes2\\.ts",
        &[(
            "conformance/conditionalTypes2.errors.txt",
            "mismatch",
            42,
            "9b199068c2c323f8",
        )],
    );
}

/// The unionAndIntersectionInference3 record (25 -> 24).
#[test]
fn fcs65_union_and_intersection_inference3_records_pin() {
    assert_twin_records(
        "typeInference/^unionAndIntersectionInference3\\.ts",
        &[(
            "conformance/unionAndIntersectionInference3.errors.txt",
            "missing-baseline",
            24,
            "79f2ef541b1787df",
        )],
    );
}

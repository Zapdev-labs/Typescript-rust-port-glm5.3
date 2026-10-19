//! FCS64 record pins (full-corpus check sweep, micro-tranche 63 — the
//! "FCS tranche 63" PORT notes in-tree; pins named fcs64 by census epoch):
//! the never-returning-call flow family — get_type_at_flow_call's
//! never-return arm un-nested to Go's structure (flow.go:308-310, a
//! sibling of the asserts-predicate arm). neverReturningFunctions1
//! converged fully (49 -> 45 = the Go count); 0 newly-divergent versus
//! the T62 epoch.

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

/// The neverReturningFunctions1 record — the f01/f11/f21/f1 x4 family
/// (the TS18048 x4 rust-extras gone; 45 = the Go count).
#[test]
fn fcs64_never_returning_functions1_records_pin() {
    assert_twin_records(
        "controlFlow/^neverReturningFunctions1\\.ts",
        &[(
            "conformance/neverReturningFunctions1.errors.txt",
            "mismatch",
            45,
            "f84afd5121ad97fd",
        )],
    );
}

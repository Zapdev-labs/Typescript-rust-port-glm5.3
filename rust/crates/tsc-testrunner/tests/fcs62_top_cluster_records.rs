//! FCS62 record pins (full-corpus check sweep, micro-tranche 61 — the
//! "FCS tranche 61" PORT notes in-tree; pins named fcs62 by census epoch):
//! the unknown-decomposition union family — is_unknown_like_union_type
//! (Go checker.go:28337) real body (strictNullChecks + union +
//! IsUnknownLikeUnion objectFlags caching; undefined, null, and the empty
//! anonymous object type constituents). keyofAndIndexedAccess converged
//! fully (53 -> 48 = the Go count); unknownControlFlow 34 -> 31 and
//! unknownType1 41 -> 40 improved; 0 newly-divergent versus the T60
//! epoch.

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

/// The keyofAndIndexedAccess record — the f92/fn/Unbounded `T`-family
/// assignability to `{} | null | undefined` (5 rust-extra TS2322/TS2345
/// gone; the record now matches the Go count exactly).
#[test]
fn fcs62_keyof_and_indexed_access_records_pin() {
    assert_twin_records(
        "keyof/^keyofAndIndexedAccess\\.ts",
        &[(
            "conformance/keyofAndIndexedAccess.errors.txt",
            "mismatch",
            48,
            "e13993c917ddb498",
        )],
    );
}

/// The unknownControlFlow record (34 -> 31; the record's ff1-era family
/// shares the unknown-decomposition target unions).
#[test]
fn fcs62_unknown_control_flow_records_pin() {
    assert_twin_records(
        "unknown/^unknownControlFlow\\.ts",
        &[(
            "conformance/unknownControlFlow.errors.txt",
            "mismatch",
            31,
            "55883013ac378f7c",
        )],
    );
}

/// The unknownType1 record (41 -> 40).
#[test]
fn fcs62_unknown_type1_records_pin() {
    assert_twin_records(
        "unknown/^unknownType1\\.ts",
        &[(
            "conformance/unknownType1.errors.txt",
            "mismatch",
            40,
            "de7c2346eddd440b",
        )],
    );
}

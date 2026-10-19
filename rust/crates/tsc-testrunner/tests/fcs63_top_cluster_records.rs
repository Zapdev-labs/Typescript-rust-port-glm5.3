//! FCS63 record pins (full-corpus check sweep, micro-tranche 62 — the
//! "FCS tranche 62" PORT notes in-tree; pins named fcs63 by census epoch):
//! the computed-name guard + binding-pattern-initializer padding families
//! — is_invalid_computed_property_name's expression read (checker.go:27330),
//! padObjectLiteralType's existing-members seeding (checker.go:17225), and
//! padTupleType's real body (checker.go:17251). mappedTypeProperties
//! converged fully (34 -> 22 = the Go count, x12 extras gone) and
//! destructuringWithLiteralInitializers converged fully (14 -> 8); 0
//! newly-divergent versus the T61 epoch.

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

/// The mappedTypeProperties record — the `[placeType in PlaceType]` members
/// of the Before/After/I/C/D containers (the x12 TS2464+TS2304+TS2693
/// rust-extras gone; 22 = the Go count).
#[test]
fn fcs63_mapped_type_properties_records_pin() {
    assert_twin_records(
        "mapped/^mappedTypeProperties\\.ts",
        &[(
            "conformance/mappedTypeProperties.errors.txt",
            "mismatch",
            22,
            "a4f0f8cb3a33e485",
        )],
    );
}

/// The destructuringWithLiteralInitializers record — the f5/g4/g5
/// binding-pattern-initializer parameter types (8 = the Go count).
#[test]
fn fcs63_destructuring_with_literal_initializers_records_pin() {
    assert_twin_records(
        "destructuring/^destructuringWithLiteralInitializers\\.ts",
        &[(
            "conformance/destructuringWithLiteralInitializers.errors.txt",
            "missing-baseline",
            8,
            "7738c1740b5dda8c",
        )],
    );
}

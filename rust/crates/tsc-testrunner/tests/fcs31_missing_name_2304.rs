//! FCS31 pins (full-corpus check sweep, tranche 31): the rust-missing
//! TS2304 "Cannot find name '{0}'" family — 39 instances across 22
//! divergent records before the tranche; the class/interface/class-
//! expression type-parameter constraint visit
//! (type_parameters_of's missing TypeParameterList arms, ast.go:527)
//! and the accessor computed-name check (checker.go:3001's REAL
//! checkComputedPropertyName) now run.
//!
//! The records below are the GO twin's exact values (rig census
//! 2026-10-16, the t31 dump-diff) — every pinned record moved from
//! rust-missing-2304 divergent to byte-identical.

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

/// The typeParameterUsedAsConstraint keystone: the four `T extends Date`
/// class/interface constraint 2304s (GO 20 errors vs RUST 16 before)
/// — byte-identical now.
#[test]
fn fcs31_type_parameter_used_as_constraint_pin() {
    assert_twin_records(
        "x/typeParameterUsedAsConstraint.ts",
        &[(
            "conformance/typeParameterUsedAsConstraint.errors.txt",
            "missing-baseline",
            20,
            "866f38e2007f1ea3",
        )],
    );
}

/// The parserGenericConstraint class-constraint pair (`class C<T extends
/// List<List<T>>>`): the two List 2304s per record (GO 12 vs RUST 10 on
/// constraint 4 before) — byte-identical now.
#[test]
fn fcs31_parser_generic_constraint4_pin() {
    assert_twin_records(
        "x/parserGenericConstraint4.ts",
        &[(
            "conformance/parserGenericConstraint4.errors.txt",
            "mismatch",
            12,
            "2d58282e191cbbb9",
        )],
    );
}

/// The symbolProperty7 accessor computed-name record: the FOURTH
/// `Symbol` 2304 (the `get [Symbol()]()` accessor name — root 2; GO 17
/// vs RUST 16 before) — byte-identical now.
#[test]
fn fcs31_symbol_property7_pin() {
    assert_twin_records(
        "x/symbolProperty7.ts",
        &[(
            "conformance/symbolProperty7.errors.txt",
            "mismatch",
            17,
            "b8b5a7ba1fc94691",
        )],
    );
}

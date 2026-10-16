//! FCS30 pins (full-corpus check sweep, tranche 30): the rust-missing
//! TS2818 "Duplicate identifier '{0}'. Compiler reserves name '{1}' when
//! emitting 'super' references in static initializers" family — 114
//! instances (38 per pre-ES2022 config) across the
//! superInStaticMembers1.ts record before the tranche, 0 after (the
//! GetEnclosingBlockScopeContainer walk at checker.go:8155 setting
//! NodeCheckFlagsContainsSuperPropertyInStaticInitializer +
//! checkCollisionsForDeclarationName for function expression names at
//! checker.go:10346).
//!
//! The records below are the GO twin's exact values (rig census
//! 2026-10-15, the probe2 post-fix stream) — every pinned record moved
//! from rust-missing-2818 divergent to byte-identical.

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

/// The superInStaticMembers1 keystone: every config's rust-missing
/// diagnostics were the 38 TS2818 Reflect reservations per pre-ES2022
/// target (400/401 vs 438/439 errors before, byte-identical now; the
/// es2022/esnext configs emit `super` directly and never reserve).
#[test]
fn fcs30_super_in_static_members1_reflect_2818_pin() {
    assert_twin_records(
        "x/superInStaticMembers1.ts",
        &[
            ("conformance/superInStaticMembers1(target=es2015).errors.txt", "mismatch", 438, "911ef68b63e843e5"),
            ("conformance/superInStaticMembers1(target=es2021).errors.txt", "mismatch", 438, "d6b8357f1f2ec1de"),
            ("conformance/superInStaticMembers1(target=es2022).errors.txt", "missing-baseline", 400, "8dedf13e2f9b8588"),
            ("conformance/superInStaticMembers1(target=es5).errors.txt", "missing-baseline", 439, "2a28d5e8b34c23cd"),
            ("conformance/superInStaticMembers1(target=esnext).errors.txt", "missing-baseline", 400, "98b3b4708ea1fd7f"),
        ],
    );
}

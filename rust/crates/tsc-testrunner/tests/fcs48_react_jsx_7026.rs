//! FCS48 pins (full-corpus check sweep, micro-tranche 48): the react-jsx
//! module-ness seam behind the tsxSpreadChildrenInvalidType TS7026 family
//! (x12 / 2 records). Root: Go's check twin parses the unit through the
//! single-file program's loader, whose parse options carry the REAL
//! GetExternalModuleIndicatorOptions answer — with `jsx:
//! react-jsx|react-jsxdev`, JSX tags force external-module-ness, so a
//! script-scoped `declare namespace JSX` stays module-local and the JSX
//! global fallback in getJsxNamespaceAt fails exactly as Go (empirically
//! traced on the rig: `isModule=true`, `globalsHasJSX=false`,
//! `globalFallback=<nil>` → TS7026 on every intrinsic element). The port's
//! unit re-parse kept the indicator options zeroed (the tranche-32
//! contract); it now computes them through the same helper the loader
//! path uses (compiler_runner.rs, "FCS tranche 48" PORT note).
//!
//! The records below are the GO twin's exact values (t48w_go stream) —
//! before the fix the two react-jsx records each under-fired 6×TS7026.

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

/// The keystone record: under jsx=react-jsx the intrinsic elements now fire
/// the 6×TS7026 per config (`JSX element implicitly has type 'any' because
/// no interface 'JSX.IntrinsicElements' exists`) exactly as Go; the
/// jsx=react records were already identical and must stay put.
#[test]
fn fcs48_react_jsx_intrinsic_elements_7026_pin() {
    assert_twin_records(
        "x/tsxSpreadChildrenInvalidType.tsx",
        &[
            (
                "conformance/tsxSpreadChildrenInvalidType(jsx=react,target=es2015).errors.txt",
                "mismatch",
                16,
                "b742876411264db9",
            ),
            (
                "conformance/tsxSpreadChildrenInvalidType(jsx=react,target=es5).errors.txt",
                "missing-baseline",
                17,
                "fc61130b551a3c2b",
            ),
            (
                "conformance/tsxSpreadChildrenInvalidType(jsx=react-jsx,target=es2015).errors.txt",
                "mismatch",
                23,
                "74f152d0ba95c381",
            ),
            (
                "conformance/tsxSpreadChildrenInvalidType(jsx=react-jsx,target=es5).errors.txt",
                "missing-baseline",
                24,
                "ebd1ae297435f86c",
            ),
        ],
    );
}

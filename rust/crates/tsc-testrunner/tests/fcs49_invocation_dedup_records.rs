//! FCS49 pins (full-corpus check sweep, micro-tranche 49): the record-level
//! twins for the tranche's three clusters —
//!
//!  * templateStringInNewExpression: the invocation-error CHAIN-HEAD fix
//!    (invocationErrorDetails builds Go's head + message-chain — TS2351
//!    head with the TS2761 "Type '{}' has no construct signatures" chain
//!    member — instead of the bare inner TS2761; the corpus records that
//!    fired only the inner message now match the Go twin);
//!  * importCallExpressionSpecifierNotStringTypeError: the dynamic-import
//!    TS7036 double-fire fix (the silent isTypeAssignableTo port — Go
//!    checker.go:8487 — each non-string specifier fires exactly one
//!    TS7036 where the port double-fired through the assignability
//!    elaboration reporter);
//!  * thisPropertyAssignmentCircular: the per-unit seam's
//!    SortAndDeduplicateDiagnostics pass (Go program.go:1675 — the real
//!    compiler pipeline applies it to every semantic-diagnostic read,
//!    program.go:733; the seam's raw read surfaced the go checker's
//!    re-check duplicates — 10 raw TS2683 at one span — where the
//!    upstream baseline and the real pipeline carry 1; both drivers now
//!    apply the pass in their unit seams).
//!
//! The records below are the twins' exact post-fix values (rig-verified
//! rust == go).

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

/// `new \`template\`` fires the TS2351 head ("This expression is not
/// constructable") with the chained TS2761 inner message — one diagnostic
/// at the template span, exactly as Go (was: a bare TS2761 with the
/// apparent-type arg). The basename filter also picks up the ES6 twin.
#[test]
fn fcs49_template_string_in_new_expression_head_pin() {
    assert_twin_records(
        "x/templateStringInNewExpression.ts",
        &[
            (
                "conformance/templateStringInNewExpression.errors.txt",
                "mismatch",
                11,
                "52618417dc8e062d",
            ),
            (
                "conformance/templateStringInNewExpressionES6.errors.txt",
                "mismatch",
                11,
                "3996d9ff87a010e1",
            ),
        ],
    );
}

/// Every non-string dynamic-import specifier fires exactly one TS7036
/// with the specifier's type as the single arg (was: a double-fire — the
/// assignability elaboration reporter's copy plus the explicit error).
#[test]
fn fcs49_import_specifier_not_string_single_fire_pin() {
    assert_twin_records(
        "x/importCallExpressionSpecifierNotStringTypeError.ts",
        &[(
            "conformance/importCallExpressionSpecifierNotStringTypeError.errors.txt",
            "mismatch",
            21,
            "3fc1dbefde4ef545",
        )],
    );
}

/// The per-unit seam applies the compiler pipeline's
/// SortAndDeduplicateDiagnostics (both drivers): the go checker's 10 raw
/// TS2683 re-check duplicates at one span collapse to the pipeline's 1 —
/// the upstream baseline's shape (was: rust 1+1 vs go 1+10, divergent).
#[test]
fn fcs49_this_property_assignment_circular_dedup_pin() {
    assert_twin_records(
        "x/thisPropertyAssignmentCircular.ts",
        &[(
            "conformance/thisPropertyAssignmentCircular.errors.txt",
            "mismatch",
            13,
            "08b3d8fb28da2f49",
        )],
    );
}

//! FCS56 record pins (full-corpus check sweep, micro-tranche 56): the
//! record-level twins for the tranche's three clusters —
//!
//!  * the exhaustiveSwitchStatements1 TS2366 x7 family (single record):
//!    Go's isLiteralType UNION arm (checker.go:25931-25934) scoped into
//!    `computeExhaustiveSwitchStatement` (flow.rs) — exhaustive literal
//!    union/enum discriminant switches now mark the unmatched-value
//!    bypass flow unreachable, so the record's seven spurious TS2366s
//!    (and the TS2322 x2 collateral on the same reachability shift) are
//!    gone; rust == go at 18 errors (was 25-vs-18 rust-extra).
//!
//!  * the jsdoc TS8030 x11/x7 family: the JSDoc @type full-signature
//!    checks (Go checker.go:3471-3475 + 10341 + 10372-10376) now run in
//!    both checkFunctionOrMethodDeclaration and
//!    checkFunctionExpressionOrObjectLiteralMethod, firing TS8030 on
//!    @type tags whose signature cannot contextually call the function;
//!    checkJsdocTypeTag6 matches the Go twin exactly (was 11-vs-15), with
//!    the checkJsdocTypeTagOnObjectProperty1/2,
//!    jsdocFunction_missingReturn and typeTagCircularReferenceOn
//!    ConstructorFunction records converging on the same root (the
//!    check_all_code_paths error-node fallback lands the FullSignature
//!    node, Go checker.go:3779-3782).
//!
//!  * the nodeModulesImportModeDeclarationEmit TS1453 x8 family: the
//!    getResolutionModeOverride(node, isTypeOnly) call in
//!    checkImportAttributes (Go checker.go:5569) fires TS1453 on a bad
//!    resolution-mode value of a TYPE-ONLY import (reportErrors =
//!    isTypeOnly) and drives the
//!    X_resolution_mode_can_only_be_set_for_type_only_imports gate — all
//!    eight records (AttributesMode x4 + Errors1 x4, one per module
//!    variant) match the Go twin exactly (were 1-short each).
//!
//! Residuals pinned at the rust side (still divergent, queued for a later
//! tranche): checkJsdocTypeTag5 15/fc207dd86291aa67 (go twin
//! 17/13593e5edf287ad4 — the TS2322 x2 assignability sub-family) and
//! jsdocThisType 31/43c31bba2b6efc90 (go twin 30/b9e69bffbc7d6681 — the
//! pre-existing TS2683 x1 @348 rust-extra; this tranche's 8030 x2
//! converged).
//!
//! Verified on the t56 full-corpus plain sweep (both drivers, 7,567
//! records): divergent 308 -> 294, ND 0, NI 14.

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

/// The exhaustiveSwitchStatements1 record — the seven spurious TS2366s
/// (every exhaustive union/enum switch function) and the TS2322 x2
/// collateral are gone; rust == go (was 25-vs-18 rust-extra).
#[test]
fn fcs56_exhaustive_switch_statements1_record_pin() {
    assert_twin_records(
        "dir/exhaustiveSwitchStatements1.ts",
        &[(
            "conformance/exhaustiveSwitchStatements1.errors.txt",
            "mismatch",
            18,
            "bb9a7785b30fa595",
        )],
    );
}

/// The checkJsdocTypeTag6 record — the four missing TS8030s (the
/// non-callable `{number}` tag, the wrong-parameter-count tags) now fire;
/// rust == go (was 11-vs-15).
#[test]
fn fcs56_check_jsdoc_type_tag6_record_pin() {
    assert_twin_records(
        "dir/checkJsdocTypeTag6.ts",
        &[(
            "conformance/checkJsdocTypeTag6.errors.txt",
            "mismatch",
            15,
            "e09b29e33d4da844",
        )],
    );
}

/// The nodeModulesImportAttributesModeDeclarationEmitErrors records —
/// the missing TS1453 (bad resolution-mode value on the type-only
/// import) now fires on the attribute VALUE node; rust == go for all
/// four module variants (were one short each).
#[test]
fn fcs56_import_attributes_mode_emit_errors_records_pin() {
    assert_twin_records(
        "dir/nodeModulesImportAttributesModeDeclarationEmitErrors.ts",
        &[
            (
                "conformance/nodeModulesImportAttributesModeDeclarationEmitErrors(module=node16).errors.txt",
                "mismatch",
                37,
                "bb4585163aecf840",
            ),
            (
                "conformance/nodeModulesImportAttributesModeDeclarationEmitErrors(module=node18).errors.txt",
                "mismatch",
                37,
                "632dd728b571f3ca",
            ),
            (
                "conformance/nodeModulesImportAttributesModeDeclarationEmitErrors(module=node20).errors.txt",
                "mismatch",
                37,
                "a2c4a234402bdb33",
            ),
            (
                "conformance/nodeModulesImportAttributesModeDeclarationEmitErrors(module=nodenext).errors.txt",
                "mismatch",
                37,
                "efff1e31ec352ee6",
            ),
        ],
    );
}

/// The nodeModulesImportModeDeclarationEmitErrors1 records — same
/// TS1453 root over the `with { "resolution-mode": "foobar" }` variant;
/// rust == go for all four module variants (were one short each).
#[test]
fn fcs56_import_mode_emit_errors1_records_pin() {
    assert_twin_records(
        "dir/nodeModulesImportModeDeclarationEmitErrors1.ts",
        &[
            (
                "conformance/nodeModulesImportModeDeclarationEmitErrors1(module=node16).errors.txt",
                "mismatch",
                37,
                "0277adaf2ad4080e",
            ),
            (
                "conformance/nodeModulesImportModeDeclarationEmitErrors1(module=node18).errors.txt",
                "mismatch",
                37,
                "d520b43c79fbb5c4",
            ),
            (
                "conformance/nodeModulesImportModeDeclarationEmitErrors1(module=node20).errors.txt",
                "mismatch",
                37,
                "f06f56d1abc60be9",
            ),
            (
                "conformance/nodeModulesImportModeDeclarationEmitErrors1(module=nodenext).errors.txt",
                "mismatch",
                37,
                "7f899d345370afbc",
            ),
        ],
    );
}

/// RESIDUAL pin — checkJsdocTypeTag5: still divergent (the TS2322 x2
/// number-vs-string assignability sub-family, queued for a later
/// tranche). Pinned at the rust side to guard the tranche's collateral:
/// this tranche's TS8030 x1 converged, so the record must stay at
/// exactly 15/fc207dd86291aa67 (go twin 17/13593e5edf287ad4).
#[test]
fn fcs56_check_jsdoc_type_tag5_residual_record_pin() {
    assert_twin_records(
        "dir/checkJsdocTypeTag5.ts",
        &[(
            "conformance/checkJsdocTypeTag5.errors.txt",
            "mismatch",
            15,
            "fc207dd86291aa67",
        )],
    );
}

/// RESIDUAL pin — jsdocThisType: still divergent (the pre-existing
/// TS2683 x1 @348 rust-extra, queued for a later tranche). Pinned at the
/// rust side to guard the tranche's collateral: this tranche's TS8030
/// x2 converged, so the record must stay at exactly 31/43c31bba2b6efc90
/// (go twin 30/b9e69bffbc7d6681).
#[test]
fn fcs56_jsdoc_this_type_residual_record_pin() {
    assert_twin_records(
        "dir/jsdocThisType.ts",
        &[(
            "conformance/jsdocThisType.errors.txt",
            "mismatch",
            31,
            "43c31bba2b6efc90",
        )],
    );
}

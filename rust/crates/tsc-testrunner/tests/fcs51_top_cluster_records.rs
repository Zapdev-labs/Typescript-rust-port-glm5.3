//! FCS51 record pins (full-corpus check sweep, micro-tranche 51): the
//! record-level twins for the tranche's clusters —
//!
//!  * the getFlowTypeOfDestructuring SYNTHETIC-ELEMENT-ACCESS port
//!    (dependentDestructuredVariables): the W3 permissive stub returned
//!    the declared type unconditionally, so the f22 shape (`const
//!    { payload } = action` after `if (action.payload)`) kept the
//!    18048 possibly-undefined extras; the port builds Go's synthetic
//!    ElementAccessExpression (checker.go:18247-18290) and the record
//!    matches the Go twin exactly (was 67 vs 65 — the fcs50 record pin
//!    in fcs50_top_cluster_records.rs carries the updated values);
//!  * the catch-clause family (catchClauseWithTypeAnnotation +
//!    jsdocCatchClauseWithTypeAnnotation): TWO roots — the spurious
//!    TS7005-with-empty-args on rejected catch annotations (Go returns
//!    errorType silently, checker.go:17081-17083) and the missing
//!    TS2492 catch-clause redeclaration walk (checker.go:4291-4297 — the
//!    locals-walk now runs over the binder's node-locals surface); both
//!    records match the Go twin exactly (were 29 vs 27 each);
//!  * the unchecked-JS spelling-suggestion family (spellingUncheckedJS):
//!    onFailedToResolveSymbol's suggestion arm now uses the REAL
//!    isUncheckedJSSuggestion probe, so the JS-file did-you-mean
//!    diagnostics downgrade to suggestions (dropped from the error
//!    streams) exactly like Go (checker.go:1639-1646) — the record
//!    matches the Go twin exactly (was 34 vs 30);
//!  * the residuals this tranche measured but did NOT fix (kept
//!    measurable): declarationsAndAssignments (the TS2403 x8
//!    subsequent-variable-declaration family — unaffected by the
//!    synthetic-element port; a separate root, queued) and
//!    dependentDestructuredVariablesNoCrash2 (the TS7010
//!    implicit-return arg shape — a separate root, queued).
//!
//! The records below are the twins' exact post-fix values (rig-verified
//! rust == go for the converged records).

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

/// The catch-clause record — both roots fixed: no spurious 7005 on the
/// rejected annotations AND the TS2492 redeclaration walk
/// (`catch (x) { let x }` shapes); rust matches the Go twin exactly
/// (was: 29 vs 27 — 7005 x3 extra + 2492 x1 missing).
#[test]
fn fcs51_catch_clause_with_type_annotation_record_pin() {
    assert_twin_records(
        "x/catchClauseWithTypeAnnotation.ts",
        &[(
            "conformance/catchClauseWithTypeAnnotation.errors.txt",
            "mismatch",
            27,
            "d32524ed36395a48",
        )],
    );
}

/// The catch-clause jsdoc twin — same two roots; rust matches the Go
/// twin exactly (was: 29 vs 27).
#[test]
fn fcs51_jsdoc_catch_clause_with_type_annotation_record_pin() {
    assert_twin_records(
        "x/jsdocCatchClauseWithTypeAnnotation.ts",
        &[(
            "conformance/jsdocCatchClauseWithTypeAnnotation.errors.txt",
            "mismatch",
            27,
            "1d59ab367cadc2c2",
        )],
    );
}

/// The unchecked-JS spelling record — the JS-file did-you-mean family
/// downgrades to suggestions (not errors); rust matches the Go twin
/// exactly (was: 34 vs 30 — 2552 x4 extra).
#[test]
fn fcs51_spelling_unchecked_js_record_pin() {
    assert_twin_records(
        "x/spellingUncheckedJS.ts",
        &[(
            "conformance/spellingUncheckedJS.errors.txt",
            "missing-baseline",
            30,
            "c8b2b45374d75bf6",
        )],
    );
}

/// The residual records this tranche measured but did not fix — the pins
/// keep the queued roots measurable:
///  * declarationsAndAssignments: the TS2403 x8
///    subsequent-variable-declaration family (42 vs 34 — NOT the
///    flow-narrowing destructuring root; queued);
///  * dependentDestructuredVariablesNoCrash2: the TS7010
///    implicit-return arg shape (16 vs 16, hash-only; queued).
#[test]
fn fcs51_residual_records_pin() {
    assert_twin_records(
        "x/declarationsAndAssignments.ts",
        &[(
            "conformance/declarationsAndAssignments.errors.txt",
            "mismatch",
            42,
            "5aa176fcfdaef524",
        )],
    );
    assert_twin_records(
        "x/dependentDestructuredVariablesNoCrash2.ts",
        &[(
            "conformance/dependentDestructuredVariablesNoCrash2.errors.txt",
            "mismatch",
            16,
            "754aad05f5fe09d7",
        )],
    );
}

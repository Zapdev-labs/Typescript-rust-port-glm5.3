//! FCS50 record pins (full-corpus check sweep, micro-tranche 50): the
//! record-level twins for the tranche's three clusters —
//!
//!  * typeGuardFunctionErrors: the type-predicate validation family —
//!    checkTypePredicate's 2677 NewDiagnosticChain head (with the
//!    elaboration as the chain tail) plus the REAL signatureHasRestParameter
//!    probe (the 1229 arm) replace the port's inverted pair and dead rest
//!    arm; the record now matches the Go twin exactly (was divergent);
//!  * ModuleWithExportedAndNonExportedFunctions: the TS2551
//!    spelling-suggestion family — the Go-exact isPropertyAccessible keeps
//!    accessible candidates in the completion-validity filter, so
//!    `fng2` answers 2551 "Did you mean 'fng'?" instead of the plain 2339
//!    (was divergent);
//!  * dependentDestructuredVariables: the dependent-destructured-variable
//!    narrowing family — getNarrowedTypeOfSymbol's location-flow-node fix +
//!    isSomeSymbolAssigned guard + the second (rest-tuple parameters) arm
//!    collapse the record's 16-diag divergence to the 2-diag
//!    getFlowTypeOfDestructuring residual (the synthetic-element-access
//!    mechanism — typeofsymbol.rs's W3 stub — still queued; the record's
//!    pre-fix divergence was 2339 x14 + 2345 x8 + 18048 x3 shaped).
//!
//! The records below are the twins' exact post-fix values (rig-verified
//! rust == go for the first two; the third is the residual rust value).

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

/// The type-predicate validation record — 2677 heads (checker.go:3130's
/// NewDiagnosticChain) + the 1229 rest-parameter arm; rust now matches the
/// Go twin's exact record values (was: divergent — bare 2741/2322/2339
/// extras where Go carries 2677/1229/2551).
#[test]
fn fcs50_type_guard_function_errors_record_pin() {
    assert_twin_records(
        "x/typeGuardFunctionErrors.ts",
        &[(
            "conformance/typeGuardFunctionErrors.errors.txt",
            "mismatch",
            101,
            "a891cbb547c0a84a",
        )],
    );
}

/// The spelling-suggestion record — `fng2` on `typeof A` answers 2551
/// "Did you mean 'fng'?" (the completion-validity filter keeps the
/// accessible candidate); rust matches the Go twin exactly (was:
/// divergent — plain 2339).
#[test]
fn fcs50_module_with_exported_and_non_exported_functions_record_pin() {
    assert_twin_records(
        "x/ModuleWithExportedAndNonExportedFunctions.ts",
        &[(
            "conformance/ModuleWithExportedAndNonExportedFunctions.errors.txt",
            "mismatch",
            14,
            "4b1ff9a4446dd0ff",
        )],
    );
}

/// The dependent-destructured-variable narrowing record — the 16-diag
/// divergence collapsed to the 2-diag getFlowTypeOfDestructuring residual
/// (rust 67 vs go 65) in FCS tranche 50; FCS tranche 51's
/// synthetic-element-access port (checker.go:18247-18290) then converged
/// the record to the Go twin exactly (65 — the 18048 x2 f22 shape now
/// narrows through the synthetic `action["payload"]` reference).
#[test]
fn fcs50_dependent_destructured_variables_record_pin() {
    // The basename prefix also picks up the six
    // dependentDestructuredVariables* sibling records — five of them
    // converged to the Go twin exactly in FCS tranche 50 (collateral of
    // the arm-1/arm-2 fixes); NoCrash2 carries its own residual hash (the
    // TS7010 implicit-return arg shape — a separate root, still queued).
    assert_twin_records(
        "x/dependentDestructuredVariables",
        &[
            (
                "conformance/dependentDestructuredVariables.errors.txt",
                "mismatch",
                65,
                "e8574e88b6b538e9",
            ),
            (
                "conformance/dependentDestructuredVariablesFromNestedPatterns.errors.txt",
                "missing-baseline",
                26,
                "36785b4160494cd6",
            ),
            (
                "conformance/dependentDestructuredVariablesNoCrash1.errors.txt",
                "mismatch",
                16,
                "41d44e2293b7bff7",
            ),
            (
                "conformance/dependentDestructuredVariablesNoCrash2.errors.txt",
                "mismatch",
                16,
                "754aad05f5fe09d7",
            ),
            (
                "conformance/dependentDestructuredVariablesNoCrash3.errors.txt",
                "mismatch",
                24,
                "da0965a34bb11bd5",
            ),
            (
                "conformance/dependentDestructuredVariablesReferenceInDeclaration1.errors.txt",
                "mismatch",
                32,
                "e053036d05ba32fd",
            ),
            (
                "conformance/dependentDestructuredVariablesWithExport.errors.txt",
                "missing-baseline",
                10,
                "ca704b5c6d35496c",
            ),
        ],
    );
}

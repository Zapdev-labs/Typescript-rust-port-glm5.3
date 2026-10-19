//! FCS61 record pins (full-corpus check sweep, micro-tranche 60 — the "FCS
//! tranche 60" PORT notes in-tree; pins named fcs61 by census epoch): the
//! const type-parameter family — is_const_type_variable's real walk,
//! createArrayLiteralType's flag pair, checkArrayLiteral's readonly tuple
//! bit, getContextualTypeForElementExpression's nil-drop mapTypeEx walk,
//! and getConstraintTypeFromMappedType's full resolver. 7 records
//! converged (jsdocTemplateTag6, typeParameterConstModifiersWith
//! Intersection, keyofAndIndexedAccess2, mappedTypeErrors{,2},
//! mappedTypes6, recursiveTypeReferences1), 0 newly-divergent versus the
//! T59 epoch (verified by a clean T59-HEAD rebuild bisect sweep).

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

/// The jsdocTemplateTag6 record — `@template const T` with fresh tuple
/// argument candidates: the ArrayLiteral flag pair merges the two
/// candidates and the const walk preserves them (10 errors, the Go twin).
#[test]
fn fcs61_jsdoc_template_tag6_records_pin() {
    assert_twin_records(
        "jsdoc/^jsdocTemplateTag6\\.ts",
        &[(
            "conformance/jsdocTemplateTag6.errors.txt",
            "missing-baseline",
            10,
            "93a5d40453727926",
        )],
    );
}

/// The typeParameterConstModifiersWithIntersection record — the const walk
/// over a `T & { a: 1 }` constraint union (10 errors, the Go twin).
#[test]
fn fcs61_const_modifiers_with_intersection_records_pin() {
    assert_twin_records(
        "typeParameterLists/^typeParameterConstModifiersWithIntersection\\.ts",
        &[(
            "conformance/typeParameterConstModifiersWithIntersection.errors.txt",
            "missing-baseline",
            10,
            "74848a4575d8fc4a",
        )],
    );
}

/// The keyofAndIndexedAccess2 record (10 errors worth of mapped-constraint
/// collateral converged; the keyofAndIndexedAccess sibling remains queued
/// for a later tranche).
#[test]
fn fcs61_keyof_and_indexed_access2_records_pin() {
    assert_twin_records(
        "keyof/^keyofAndIndexedAccess2\\.ts",
        &[(
            "conformance/keyofAndIndexedAccess2.errors.txt",
            "mismatch",
            44,
            "8600b6ce4854458e",
        )],
    );
}

/// The mappedTypeErrors{,2} records — the mapped-constraint full-resolver
/// convergence.
#[test]
fn fcs61_mapped_type_errors_records_pin() {
    assert_twin_records(
        "mapped/^mappedTypeErrors\\.ts",
        &[(
            "conformance/mappedTypeErrors.errors.txt",
            "mismatch",
            42,
            "ccc93bb3b52e492a",
        )],
    );
    assert_twin_records(
        "mapped/^mappedTypeErrors2\\.ts",
        &[(
            "conformance/mappedTypeErrors2.errors.txt",
            "mismatch",
            16,
            "406845d676743785",
        )],
    );
}

/// The mappedTypes6 record.
#[test]
fn fcs61_mapped_types6_records_pin() {
    assert_twin_records(
        "mapped/^mappedTypes6\\.ts",
        &[(
            "conformance/mappedTypes6.errors.txt",
            "mismatch",
            25,
            "0260b53e5ea5daa4",
        )],
    );
}

/// The recursiveTypeReferences1 record (the union-object-literal merge
/// collateral).
#[test]
fn fcs61_recursive_type_references1_records_pin() {
    assert_twin_records(
        "recursiveTypes/^recursiveTypeReferences1\\.ts",
        &[(
            "conformance/recursiveTypeReferences1.errors.txt",
            "mismatch",
            54,
            "aca6c81814c1efe1",
        )],
    );
}

/// The typeParameterConstModifiers record — the const walk + the
/// array-literal merge + the readonly bit + the union-contextual element
/// walk moved the record 18→17 (Go: 16); the remaining extra is the
/// thingMapped conditional-instantiation root queued for a later
/// tranche. Pinned at the current twin-progress value.
#[test]
fn fcs61_const_modifiers_progress_records_pin() {
    assert_twin_records(
        "typeParameterLists/^typeParameterConstModifiers\\.ts",
        &[(
            "conformance/typeParameterConstModifiers.errors.txt",
            "mismatch",
            17,
            "c981fd734b1af83d",
        )],
    );
}

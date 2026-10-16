//! FCS29 pins (full-corpus check sweep, tranche 29): the rust-missing
//! TS2318 "Cannot find global type 'X'" family — 130 instances across
//! 86 records before the tranche, 6 stragglers after (all distinct roots,
//! reported in $HOME/.fcs29-continuation.md).
//!
//! The records below are the GO twin's exact values (rig census 2026-10-15,
//! the t29f_full_go stream) — every pinned record moved from
//! rust-missing-2318 divergent to byte-identical.
//!
//! (The `-test.run` filter reads only the second `/` component — the
//! basename — and its literal prefix stops at the regex metachar `.`.)

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

/// The TypedPropertyDescriptor keystone: legacy decorators on auto-accessor
/// properties — every config's only rust-missing diagnostic was the
/// location-less 2318 of the synthetic descriptor type (15 vs 14 errors
/// before, 15 both drivers now).
#[test]
fn fcs29_auto_accessor_experimental_decorators_2318_pin() {
    assert_twin_records(
        "x/autoAccessorExperimentalDecorators.ts",
        &[
            ("conformance/autoAccessorExperimentalDecorators(target=es2015).errors.txt", "mismatch", 15, "f0a15ed7565f2c19"),
            ("conformance/autoAccessorExperimentalDecorators(target=es2022).errors.txt", "mismatch", 15, "9b2a84744498d023"),
            ("conformance/autoAccessorExperimentalDecorators(target=esnext).errors.txt", "mismatch", 15, "6580f4775fcb210a"),
        ],
    );
}

/// The ES-decorator context keystone: esDecoratorsMetadata1 — the
/// ClassDecoratorContext + ClassMethodDecoratorContext 2318 pair of the
/// synthetic ES signatures (10 vs 12 errors before, 12 both drivers now).
#[test]
fn fcs29_es_decorators_metadata1_context_2318_pin() {
    assert_twin_records(
        "x/esDecoratorsMetadata1.ts",
        &[
            ("conformance/esDecoratorsMetadata1(target=es2015).errors.txt", "missing-baseline", 12, "1d4ca662c9a3bcf5"),
            ("conformance/esDecoratorsMetadata1(target=es2022).errors.txt", "missing-baseline", 12, "9b863d922a44a7a3"),
        ],
    );
}

/// The Iterable keystone: the empty array binding pattern at ES2015
/// (checker.go:18370 createIterableType(c.anyType)) — 10 vs 11 errors
/// before, 11 both drivers now.
#[test]
fn fcs29_empty_array_binding_pattern_iterable_2318_pin() {
    assert_twin_records(
        "x/emptyArrayBindingPatternParameter01.ts",
        &[(
            "conformance/emptyArrayBindingPatternParameter01.errors.txt",
            "missing-baseline",
            11,
            "a359595d5ce8bf4a",
        )],
    );
}

/// The Extract keystone: getExtractStringType's memoized
/// reportErrors=true resolver row (checker.go:27242) — 13 vs 14 errors
/// before, 14 both drivers now.
#[test]
fn fcs29_type_guards_type_parameters_extract_2318_pin() {
    assert_twin_records(
        "x/typeGuardsTypeParameters.ts",
        &[(
            "conformance/typeGuardsTypeParameters.errors.txt",
            "missing-baseline",
            14,
            "8697ab95cf334762",
        )],
    );
}

/// The NodeCanBeDecorated GATE pin: legacy decorators on private-named
/// members never reach the synthetic signature construction — this record
/// must stay at the go twin's 12 errors with NO extra TypedPropertyDescriptor
/// 2318 (the gate was the tranche's newly-divergent guard).
#[test]
fn fcs29_private_names_and_decorators_gate_pin() {
    assert_twin_records(
        "x/privateNamesAndDecorators.ts",
        &[(
            "conformance/privateNamesAndDecorators.errors.txt",
            "mismatch",
            12,
            "256408915579c9cb",
        )],
    );
}

/// The NodeCanBeDecorated GATE pin (overloads): legacy decorators on
/// body-less method signatures — both configs stay at the go twin's
/// 11/12 errors with no 2318.
#[test]
fn fcs29_decorator_on_class_method_overload1_gate_pin() {
    assert_twin_records(
        "x/decoratorOnClassMethodOverload1.ts",
        &[
            ("conformance/decoratorOnClassMethodOverload1(target=es2015).errors.txt", "mismatch", 11, "6a8dd51fa48c32c8"),
            ("conformance/decoratorOnClassMethodOverload1(target=es5).errors.txt", "missing-baseline", 12, "2f46137078c52518"),
        ],
    );
}

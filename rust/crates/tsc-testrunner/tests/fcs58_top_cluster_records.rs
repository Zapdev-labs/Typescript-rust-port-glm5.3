//! FCS58 record pins (full-corpus check sweep, micro-tranche 58): the
//! record-level twins for the tranche's clusters —
//!
//!  * the for-of/using TS2448 family (5 records, 7 instances: for-of55,
//!    ES5For-of17 + ES5For-of20 both target variants,
//!    awaitUsingDeclarationsInForOf.4, awaitUsingDeclarationsInForAwaitOf
//!    .2): the loop variable referenced in the ITERATED expression of its
//!    own for-of/await-using now reports TS2448
//!    ("Block-scoped variable used before its declaration") — Go's
//!    `isImmediatelyUsedInInitializerOfBlockScopedVariable` checks the
//!    use site against `grandparent.Expression()` (checker.go:2127); the
//!    port passed the loop initializer (the declaration side). All
//!    records match the Go twin exactly (were missing TS2448 x7/7).
//!
//!  * the privateNames static family (rust extra TS2739 x8/7 over
//!    privateNamesAndStaticFields, privateNamesAndStaticMethods,
//!    privateNameStaticsAndStaticMethods, privateNameInInExpression both
//!    target variants): Go's `getUnmatchedPropertiesWorker` filters
//!    static private-identifier target props (relater.go:981); the port's
//!    `is_static_private_identifier_property` was the M6 S6 `false` stub.
//!
//!  * the privateNames class-pair family (rust extra TS2741 x7/7 over
//!    privateNamesUnique-1/-4/-5, privateNamesConstructorChain-1/-2,
//!    privateNameStaticFieldDerivedClasses,
//!    privateNameStaticAccessorssDerivedClasses): Go's
//!    reportUnmatchedProperty private-identifier arm (relater.go:4379-
//!    4391) reports TS18015 for class sources, and the
//!    getSingleBaseForNonAugmentingSubtype normalization (checker.go:
//!    28621) + isThislessInterface heritage resolution (checker.go:17743)
//!    make interface sources report the TS2322 head with the chain — the
//!    bare TS2741 no longer surfaces alone.
//!
//!  * the jsx TS2695 family (7 instances over tsxErrorRecovery2/3,
//!    jsxInvalidEsprimaTestSuite, jsxCheckJsxNoTypeArgumentsAllowed):
//!    the comma-operator error is suppressed inside a parser TS2657 span
//!    (Go checker.go:12774-12781).
//!
//!  * the constructableDecoratorOnClass01 residual: the decorator
//!    no-call-signatures branch uses node.Expression() as the error
//!    target (Go checker.go:8981), so the chained TS1238 head span
//!    matches Go's (20,28).
//!
//! Verified on the rig real-corpus runs (both drivers, all target
//! records identical post-fix). Filters are anchored escaped forms — the
//! runner consumes only the literal prefix before the first regex
//! metachar, and several basenames have sibling cases.

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

/// The for-of/using TS2448 records: for-of55, ES5For-of17 and
/// ES5For-of20 (both target variants each) — the RHS loop-variable
/// reference now reports TS2448; rust == go on every record.
#[test]
fn fcs58_for_of_2448_records_pin() {
    assert_twin_records(
        "x/^for-of55\\.ts",
        &[(
            "conformance/for-of55.errors.txt",
            "mismatch",
            12,
            "f45aceab88c08ad4",
        )],
    );
    assert_twin_records(
        "x/^ES5For-of17\\.ts",
        &[
            (
                "conformance/ES5For-of17(target=es2015).errors.txt",
                "mismatch",
                12,
                "ee562d3ec99802d3",
            ),
            (
                "conformance/ES5For-of17(target=es5).errors.txt",
                "missing-baseline",
                13,
                "9d1590aa6f7626b6",
            ),
        ],
    );
    assert_twin_records(
        "x/^ES5For-of20\\.ts",
        &[
            (
                "conformance/ES5For-of20(target=es2015).errors.txt",
                "mismatch",
                10,
                "99812de22a0404f9",
            ),
            (
                "conformance/ES5For-of20(target=es5).errors.txt",
                "missing-baseline",
                11,
                "ab9ce5c5bb03a343",
            ),
        ],
    );
    assert_twin_records(
        "x/^awaitUsingDeclarationsInForOf\\.4\\.ts",
        &[(
            "conformance/awaitUsingDeclarationsInForOf.4.errors.txt",
            "mismatch",
            10,
            "3ddee57511030ed1",
        )],
    );
    assert_twin_records(
        "x/^awaitUsingDeclarationsInForAwaitOf\\.2\\.ts",
        &[(
            "conformance/awaitUsingDeclarationsInForAwaitOf.2.errors.txt",
            "mismatch",
            9,
            "1be235fed2007f9f",
        )],
    );
}

/// The privateNames static records (were rust-extra TS2739): the class
/// side comparison no longer lists static private-identifier fields as
/// missing; rust == go on every record (both target variants where the
/// multi-file records have them).
#[test]
fn fcs58_private_names_static_2739_records_pin() {
    assert_twin_records(
        "x/^privateNamesAndStaticFields\\.ts",
        &[(
            "conformance/privateNamesAndStaticFields.errors.txt",
            "mismatch",
            12,
            "cc38406361e3164a",
        )],
    );
    assert_twin_records(
        "x/^privateNamesAndStaticMethods\\.ts",
        &[
            (
                "conformance/privateNamesAndStaticMethods(target=es2022).errors.txt",
                "missing-baseline",
                13,
                "94b3b40a84134ffd",
            ),
            (
                "conformance/privateNamesAndStaticMethods(target=esnext).errors.txt",
                "missing-baseline",
                13,
                "d3bb7a06819128f2",
            ),
        ],
    );
    assert_twin_records(
        "x/^privateNameStaticsAndStaticMethods\\.ts",
        &[
            (
                "conformance/privateNameStaticsAndStaticMethods(target=es2022).errors.txt",
                "missing-baseline",
                13,
                "ab6649e9881e3813",
            ),
            (
                "conformance/privateNameStaticsAndStaticMethods(target=esnext).errors.txt",
                "missing-baseline",
                13,
                "655d12657f40a940",
            ),
        ],
    );
    assert_twin_records(
        "x/^privateNameInInExpression\\.ts",
        &[
            (
                "conformance/privateNameInInExpression(target=es2022).errors.txt",
                "mismatch",
                18,
                "16416c481fbbd016",
            ),
            (
                "conformance/privateNameInInExpression(target=esnext).errors.txt",
                "mismatch",
                18,
                "1a380f493a083647",
            ),
        ],
    );
}

/// The privateNames class-pair records (were rust-extra TS2741): the
/// 18015 arm and the single-base normalization collapse; rust == go on
/// every record.
#[test]
fn fcs58_private_names_2741_records_pin() {
    assert_twin_records(
        "x/^privateNamesUnique-1\\.ts",
        &[(
            "conformance/privateNamesUnique-1.errors.txt",
            "mismatch",
            11,
            "13a30d6bb701d56a",
        )],
    );
    assert_twin_records(
        "x/^privateNamesUnique-4\\.ts",
        &[(
            "conformance/privateNamesUnique-4.errors.txt",
            "mismatch",
            12,
            "36f93424757c3f1b",
        )],
    );
    assert_twin_records(
        "x/^privateNamesUnique-5\\.ts",
        &[(
            "conformance/privateNamesUnique-5.errors.txt",
            "mismatch",
            11,
            "75a96353ccde83f6",
        )],
    );
    assert_twin_records(
        "x/^privateNamesConstructorChain-1\\.ts",
        &[(
            "conformance/privateNamesConstructorChain-1.errors.txt",
            "mismatch",
            11,
            "88b234444522cd7b",
        )],
    );
    assert_twin_records(
        "x/^privateNamesConstructorChain-2\\.ts",
        &[(
            "conformance/privateNamesConstructorChain-2.errors.txt",
            "mismatch",
            11,
            "e485fd946e503a5a",
        )],
    );
    assert_twin_records(
        "x/^privateNameStaticFieldDerivedClasses\\.ts",
        &[(
            "conformance/privateNameStaticFieldDerivedClasses.errors.txt",
            "mismatch",
            12,
            "37e32c136e096c83",
        )],
    );
    assert_twin_records(
        "x/^privateNameStaticAccessorssDerivedClasses\\.ts",
        &[(
            "conformance/privateNameStaticAccessorssDerivedClasses.errors.txt",
            "mismatch",
            14,
            "877b2945d5d36190",
        )],
    );
}

/// The jsx TS2695 records (were rust-extra): the comma-operator error is
/// suppressed inside parser 2657 spans; rust == go on every record.
#[test]
fn fcs58_jsx_2695_records_pin() {
    assert_twin_records(
        "x/^tsxErrorRecovery2\\.tsx",
        &[(
            "conformance/tsxErrorRecovery2.errors.txt",
            "mismatch",
            30,
            "45cefb9053d99999",
        )],
    );
    assert_twin_records(
        "x/^tsxErrorRecovery3\\.tsx",
        &[(
            "conformance/tsxErrorRecovery3.errors.txt",
            "mismatch",
            34,
            "2a522d8251693cb6",
        )],
    );
    assert_twin_records(
        "x/^jsxInvalidEsprimaTestSuite\\.tsx",
        &[(
            "conformance/jsxInvalidEsprimaTestSuite.errors.txt",
            "mismatch",
            432,
            "177edf6fe94ab949",
        )],
    );
    assert_twin_records(
        "x/^jsxCheckJsxNoTypeArgumentsAllowed\\.tsx",
        &[(
            "conformance/jsxCheckJsxNoTypeArgumentsAllowed.errors.txt",
            "mismatch",
            29,
            "6641a6c7798e1235",
        )],
    );
}

/// The constructableDecoratorOnClass01 residual record: the chained
/// TS1238 head now spans the decorator expression like Go's.
#[test]
fn fcs58_constructable_decorator_on_class01_record_pin() {
    assert_twin_records(
        "x/^constructableDecoratorOnClass01\\.ts",
        &[(
            "conformance/constructableDecoratorOnClass01.errors.txt",
            "mismatch",
            11,
            "b07cd5c7fe8614c6",
        )],
    );
}

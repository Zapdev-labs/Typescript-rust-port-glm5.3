//! FCS57 record pins (full-corpus check sweep, micro-tranche 57): the
//! record-level twins for the tranche's three clusters —
//!
//!  * the decorator arity-head family (9 records over
//!    esDecorators-arguments + decoratorOnClass8/Property6/Property7/
//!    Method8, both target variants): the headMessage threaded from
//!    `getDiagnosticHeadMessageForDecoratorResolution`
//!    (Go checker.go:9004) through `reportCallResolutionErrors` into
//!    `getArgumentArityError`/`getTypeArgumentArityError` makes the
//!    "Unable to resolve signature of {class,property,method} decorator
//!    when called as an expression" head (TS1238/1240/1241) the primary
//!    code; the bare TS1278 no longer surfaces alone. All nine records
//!    match the Go twin exactly (were rust-extra TS1278 / missing
//!    TS1238/1240/1241).
//!
//!  * the privateNameNestedClasses family (5 records:
//!    privateNamesInNestedClasses-1/-2, privateNameNestedMethodAccess,
//!    privateNameNestedClassFieldShadowing,
//!    privateNameNestedClassAccessorsShadowing): the shadowed branch of
//!    `checkPrivateIdentifierPropertyAccess` (Go checker.go:11754-11758)
//!    now reports TS18014 ("The property '#x' cannot be accessed on type
//!    'X' within this class because it is shadowed by another private
//!    identifier with the same spelling") with the two related-info
//!    adds; the records match the Go twin exactly (were rust-extra
//!    TS18013 x6 / missing TS18014 x6 over the same positions).
//!
//!  * the parserArrowFunctionExpression TS8010 family (6 records:
//!    parserArrowFunctionExpression10/13/14/15/16/17): the parser's
//!    `type_of` helper now mirrors Go's `node.Type()` FunctionLikeData
//!    default arm (ast/ast.go:685-687), so `checkJSSyntax` reports
//!    TS8010 on the RETURN TYPE of an arrow function in a JS file; the
//!    records match the Go twin exactly (were missing the return-type
//!    8010 each).
//!
//! Verified on the rig real-corpus runs (both drivers, all 20 target
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

/// The decorator arity-head records: esDecorators-arguments and
/// decoratorOnClass8 (both target variants) — the TS1238 class-decorator
/// head now rides the arity error; rust == go on every record.
#[test]
fn fcs57_decorator_class_records_pin() {
    assert_twin_records(
        "x/^esDecorators-arguments\\.ts",
        &[(
            "conformance/esDecorators-arguments.errors.txt",
            "mismatch",
            14,
            "1bb151fc4d26d995",
        )],
    );
    assert_twin_records(
        "x/^decoratorOnClass8\\.ts",
        &[
            (
                "conformance/decoratorOnClass8(target=es2015).errors.txt",
                "mismatch",
                12,
                "b82fc77bf78fa2b1",
            ),
            (
                "conformance/decoratorOnClass8(target=es5).errors.txt",
                "missing-baseline",
                13,
                "dd88fd251d564496",
            ),
        ],
    );
}

/// The decorator arity-head records: decoratorOnClassProperty6/7 (both
/// targets each) — the TS1240 property-decorator head now rides the
/// arity error; rust == go on every record.
#[test]
fn fcs57_decorator_property_records_pin() {
    assert_twin_records(
        "x/^decoratorOnClassProperty6\\.ts",
        &[
            (
                "conformance/decoratorOnClassProperty6(target=es2015).errors.txt",
                "mismatch",
                10,
                "b18e2b973c83d0e3",
            ),
            (
                "conformance/decoratorOnClassProperty6(target=es5).errors.txt",
                "missing-baseline",
                11,
                "baeb3ce20957310f",
            ),
        ],
    );
    assert_twin_records(
        "x/^decoratorOnClassProperty7\\.ts",
        &[
            (
                "conformance/decoratorOnClassProperty7(target=es2015).errors.txt",
                "mismatch",
                10,
                "e2d1d8c47428887c",
            ),
            (
                "conformance/decoratorOnClassProperty7(target=es5).errors.txt",
                "missing-baseline",
                11,
                "5a2318c6cb78eae0",
            ),
        ],
    );
}

/// The decorator arity-head records: decoratorOnClassMethod8 (both
/// targets) — the TS1241 method-decorator head now rides the arity
/// error; rust == go on both records.
#[test]
fn fcs57_decorator_method_records_pin() {
    assert_twin_records(
        "x/^decoratorOnClassMethod8\\.ts",
        &[
            (
                "conformance/decoratorOnClassMethod8(target=es2015).errors.txt",
                "mismatch",
                12,
                "39439d7b46a41b1f",
            ),
            (
                "conformance/decoratorOnClassMethod8(target=es5).errors.txt",
                "missing-baseline",
                13,
                "2e566752be3d97fa",
            ),
        ],
    );
}

/// The shadowed-private-identifier records: the five
/// privateNameNestedClasses-family records now report the TS18014
/// shadowed-private-identifier diagnostic (with both related infos) where
/// the port previously reported the bare TS18013; rust == go.
#[test]
fn fcs57_shadowed_private_identifier_records_pin() {
    for (name, count, hash) in [
        (
            "conformance/privateNameNestedClassAccessorsShadowing.errors.txt",
            13,
            "f3a1d40bb06700bf",
        ),
        (
            "conformance/privateNameNestedClassFieldShadowing.errors.txt",
            11,
            "0ceb9b8e9cd7c67f",
        ),
        (
            "conformance/privateNameNestedMethodAccess.errors.txt",
            12,
            "44deea7cec15cd82",
        ),
        (
            "conformance/privateNamesInNestedClasses-1.errors.txt",
            11,
            "278ba7db042bd661",
        ),
        (
            "conformance/privateNamesInNestedClasses-2.errors.txt",
            11,
            "b62aa1b6d49c2a3f",
        ),
    ] {
        let stem = name.trim_start_matches("conformance/").trim_end_matches(".errors.txt");
        let filter = format!("x/^{stem}\\.ts");
        assert_twin_records(filter.as_str(), &[(name, "mismatch", count, hash)]);
    }
}

/// The JS arrow return-type TS8010 records: the six
/// parserArrowFunctionExpression family records (10/13/14/15/16/17) now
/// fire the 8010 on the arrow's return type like Go; rust == go.
#[test]
fn fcs57_arrow_return_type_8010_records_pin() {
    for (name, count, hash) in [
        (
            "conformance/parserArrowFunctionExpression10.errors.txt",
            26,
            "56cd6c403994878e",
        ),
        (
            "conformance/parserArrowFunctionExpression13.errors.txt",
            26,
            "b083baa390b1ee96",
        ),
        (
            "conformance/parserArrowFunctionExpression14.errors.txt",
            31,
            "c557331e0540b787",
        ),
        (
            "conformance/parserArrowFunctionExpression15.errors.txt",
            18,
            "541720440af83447",
        ),
        (
            "conformance/parserArrowFunctionExpression16.errors.txt",
            18,
            "a615549b3f8f808d",
        ),
        (
            "conformance/parserArrowFunctionExpression17.errors.txt",
            26,
            "f2ed0b37e44575a4",
        ),
    ] {
        let stem = name.trim_start_matches("conformance/").trim_end_matches(".errors.txt");
        let filter = format!("x/^{stem}\\.ts");
        assert_twin_records(filter.as_str(), &[(name, "mismatch", count, hash)]);
    }
}

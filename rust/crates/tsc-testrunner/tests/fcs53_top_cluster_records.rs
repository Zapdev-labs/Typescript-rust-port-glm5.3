//! FCS53 record pins (full-corpus check sweep, micro-tranche 53): the
//! record-level twins for the tranche's three clusters —
//!
//!  * the generatedContextualTyping TS2322 x10 family: the rust
//!    `forEachReturnStatement` walk leaked the return statements of
//!    nested function expressions/arrows into the outer function's
//!    aggregated return type (Go's ast.ForEachReturnStatement —
//!    ast/utilities.go:1157 — traverses only statement containers), so
//!    `var x157: () => () => Base[] = () => { return () => [d1, d2]; }`
//!    and its x158-x179 siblings reported spurious TS2322s; the record
//!    matches the Go twin exactly (was 110 vs 100);
//!  * the declarationsAndAssignments TS2403 x8 family: the tuple-rest
//!    slice (Go checker.go:18156-18166) — `var [...a3] = v` over a tuple
//!    types as the remaining tuple, so the secondary redeclaration stays
//!    identical to `var a3!: [number, number, number]`; the record matches
//!    the Go twin exactly (was 42 vs 34; the t51 residual pin moved here);
//!  * the rewriteRelativeImportExtensions TS2307 x6 family: Go
//!    checker.go:8551 redirects CommonJS require calls in JS files to
//!    resolveExternalModuleTypeByLiteral — the js.js/a.js
//!    `require("./foo.ts")` calls now resolve-and-report their module
//!    specifier; emit.ts (both jsx variants) and emitModuleCommonJS.ts
//!    (both module variants) match the Go twin exactly (were 96 vs 98
//!    and 29 vs 30).
//!
//! plus the collateral convergences the require-call root fixed — the
//! salsa/jsDeclarations require-resolution family (commonJSReexport,
//! exportNestedNamespaces2, jsdocThisType, jsDeclarationsTypeReferences
//! 1-4, jsDeclarationsCommonjsRelativePath, nestedDestructuringOfRequire,
//! requireOfESWithPropertyAccess, requireTwoPropertyAccesses, ...) —
//! pinned at the sweep level by the monotonicity numbers (NI 29, ND 0,
//! divergent 350 -> 321).

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

/// The nested-contextual-typing record — the outer arrows' return unions
/// aggregate only their own returns; rust == go (was 110 vs 100 — the
/// TS2322 x10 x157-x179 family).
#[test]
fn fcs53_generated_contextual_typing_record_pin() {
    assert_twin_records(
        "dir/generatedContextualTyping.ts",
        &[(
            "conformance/generatedContextualTyping.errors.txt",
            "mismatch",
            100,
            "1e31240ade12bac0",
        )],
    );
}

/// The tuple-rest redeclaration record — the rest binding elements slice
/// their tuple parents, so the secondary declarations stay identical;
/// rust == go (was 42 vs 34 — the TS2403 x8 f20/f21 rest family).
#[test]
fn fcs53_declarations_and_assignments_record_pin() {
    assert_twin_records(
        "dir/declarationsAndAssignments.ts",
        &[(
            "conformance/declarationsAndAssignments.errors.txt",
            "mismatch",
            34,
            "eac38fc7e76b0c30",
        )],
    );
}

/// The require-call module-resolution records — the js.js/a.js
/// `require("./foo.ts")` calls resolve-and-report; rust == go for both
/// jsx variants of emit.ts (was 96 vs 98 — TS2307 x2 per record) and
/// both module variants of emitModuleCommonJS.ts (was 29 vs 30).
#[test]
fn fcs53_rewrite_relative_import_extensions_records_pin() {
    // `emit.ts` is all-lowercase and its `.` is a regex any-char — the
    // unanchored filter would accept every case, so the anchored,
    // escaped-dot form pins the exact basename.
    assert_twin_records(
        "dir/^emit\\.ts",
        &[
            (
                "conformance/emit(jsx=preserve).errors.txt",
                "mismatch",
                98,
                "179c5dacc239dc28",
            ),
            (
                "conformance/emit(jsx=react).errors.txt",
                "mismatch",
                98,
                "1778e2934c8ec6b9",
            ),
        ],
    );
    assert_twin_records(
        "dir/emitModuleCommonJS.ts",
        &[
            (
                "conformance/emitModuleCommonJS(module=commonjs).errors.txt",
                "mismatch",
                30,
                "ec39163cd426fd42",
            ),
            (
                "conformance/emitModuleCommonJS(module=nodenext).errors.txt",
                "mismatch",
                30,
                "cba9a4abddcc405f",
            ),
        ],
    );
}

/// The residual record this tranche measured but did not fix —
/// variadicTuples1: the pre-existing TS2345 x2 rust-extra pair
/// (`[T[number], U[number]]` vs `[...T, ...U]`) plus the pre-existing
/// TS2322 x2 rust-missing pair (`${T['length']}` / `${[...T]['length']}`
/// template-literal spans — the reference baseline shows NO errors there,
/// so the go twin over-emits; rust stays baseline-correct). The record's
/// diff set is byte-identical pre- and post-tranche; the tuple-rest slice
/// (cluster C) did not disturb it. ALSO the debug-profile regression
/// guard for FCS tranche 53's infer_tuple_types signed-arithmetic fix:
/// the `[...T, ...U]` inference reaches `middleLength :=
/// targetArity - startLength - endLength` with endLength > targetArity —
/// the former usize subtraction underflowed (attempt to subtract with
/// overflow); the Go-exact isize arithmetic no longer panics.
#[test]
fn fcs53_variadic_tuples1_residual_record_pin() {
    assert_twin_records(
        "dir/^variadicTuples1\\.ts",
        &[(
            "conformance/variadicTuples1.errors.txt",
            "mismatch",
            53,
            "0bad9304a569e792",
        )],
    );
}

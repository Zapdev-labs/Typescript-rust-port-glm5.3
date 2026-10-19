//! FCS54 record pins (full-corpus check sweep, micro-tranche 54): the
//! record-level twins for the tranche's three clusters —
//!
//!  * the classStaticBlockUseBeforeDef TS7008/TS2729 family: the real
//!    `getFlowTypeOfProperty` flow walk (Go checker.go:11688) +
//!    `isPropertyInitializedInStaticBlocks`'s synthetic return-flow probe
//!    (Go checker.go:5067) — the static blocks' `this.x = 1` writes now
//!    infer the un-annotated static properties' types (no spurious
//!    TS7008 "Member 'x' implicitly has an 'any' type") and the
//!    `static y = this.x` read is legal when a preceding block
//!    initializes x (no spurious TS2729). Def1/Def2 (both target
//!    variants) and Def4/Def5 match the Go twin exactly (were
//!    13/10-vs-10, 12/11-vs-11, 10-vs-8, 12/11-vs-11);
//!  * the parser.asyncGenerators.objectLiteralMethods TS2318 x4 family:
//!    the recovered `async * get x() { ... }` member's MISSING synthesized
//!    body answers anyType in getReturnTypeOfSignature (Go
//!    checker.go:20432's NodeIsMissing gate) without running the
//!    async-generator wrap — the location-less "Cannot find global type
//!    'AsyncIterableIterator'" no longer leaks (both alwaysstrict
//!    variants match; were 222-vs-224 / 221-vs-223);
//!  * the interfaceExtendingClassWithProtecteds2 TS2430 x2 family: the
//!    real isValidOverrideOf (Go checker.go:12167) fails the per-base
//!    assignability for the public-own-`x`-shadows-protected-`x` shape, so
//!    the 2430 heads with their protected related-info fire — the record
//!    matches the Go twin exactly (was 13 vs 15; the t52 residual pin
//!    moved to the converged value). Collateral: the
//!    interfaceExtendingClassWithProtecteds / mixinAccessModifiers /
//!    redeclaredProperty records converge on the same root.
//!
//! Verified on the t55 full-corpus plain sweep (both drivers, 7,567
//! records): divergent 321 -> 308, ND 0, NI 13.

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

/// The Def1 record — the blocks after the declarations infer x/z as
/// `number` and the `static y = this.x` read is initialized by the
/// preceding block; rust == go for both target variants (were 13/10
/// rust-extra — TS7008 x2 + TS2729 x1 per record).
#[test]
fn fcs54_static_block_use_before_def1_records_pin() {
    assert_twin_records(
        "dir/classStaticBlockUseBeforeDef1.ts",
        &[
            (
                "conformance/classStaticBlockUseBeforeDef1(target=es2022).errors.txt",
                "missing-baseline",
                10,
                "8536218494cab066",
            ),
            (
                "conformance/classStaticBlockUseBeforeDef1(target=esnext).errors.txt",
                "missing-baseline",
                10,
                "8fcc6fce8b21772b",
            ),
        ],
    );
}

/// The Def2 record — the block BEFORE the declaration still fires TS2729
/// for the write itself (Go's answer) but no longer fires the spurious
/// TS7008; rust == go for both target variants (were 12/11
/// rust-extra).
#[test]
fn fcs54_static_block_use_before_def2_records_pin() {
    assert_twin_records(
        "dir/classStaticBlockUseBeforeDef2.ts",
        &[
            (
                "conformance/classStaticBlockUseBeforeDef2(target=es2022).errors.txt",
                "mismatch",
                11,
                "f36cb2dac8ddfeeb",
            ),
            (
                "conformance/classStaticBlockUseBeforeDef2(target=esnext).errors.txt",
                "mismatch",
                11,
                "ccb33796e9af0c8c",
            ),
        ],
    );
}

/// The Def4 record (auto accessors) — the TS7008 "x any"/"z any" pair is
/// gone; rust == go (was 10 vs 8).
#[test]
fn fcs54_static_block_use_before_def4_record_pin() {
    assert_twin_records(
        "dir/classStaticBlockUseBeforeDef4.ts",
        &[(
            "conformance/classStaticBlockUseBeforeDef4.errors.txt",
            "missing-baseline",
            10,
            "603fcfa8e7987b6b",
        )],
    );
}

/// The Def5 record (auto accessor, block before decl) — the TS7008 is
/// gone; rust == go (was 12/11 rust-extra).
#[test]
fn fcs54_static_block_use_before_def5_record_pin() {
    assert_twin_records(
        "dir/classStaticBlockUseBeforeDef5.ts",
        &[(
            "conformance/classStaticBlockUseBeforeDef5.errors.txt",
            "mismatch",
            11,
            "51c687b1c680c56b",
        )],
    );
}

/// The async-generator object-literal-methods record — the recovered
/// accessor members' missing bodies answer anyType without the
/// AsyncIterableIterator wrap; rust == go for both alwaysstrict variants
/// (were 222 vs 224 / 221 vs 223 — TS2318 x2 per record).
#[test]
fn fcs54_object_literal_methods_records_pin() {
    // All-lowercase basename: the runner's filter consumes only the
    // LITERAL PREFIX before the first regex metachar, and a lowercase
    // unanchored prefix ACCEPTS ALL — the anchored escaped-dot form pins
    // the exact case.
    assert_twin_records(
        "dir/^parser\\.asyncGenerators\\.objectLiteralMethods\\.es2018\\.ts",
        &[
            (
                "conformance/parser.asyncGenerators.objectLiteralMethods.es2018(alwaysstrict=false).errors.txt",
                "missing-baseline",
                222,
                "d0e9e6487a3ad636",
            ),
            (
                "conformance/parser.asyncGenerators.objectLiteralMethods.es2018(alwaysstrict=true).errors.txt",
                "mismatch",
                221,
                "f46b339f6f20abb0",
            ),
        ],
    );
}

/// The protecteds2 record — the I4 TS2430 x2 heads fire (isValidOverrideOf
/// real); rust == go (was 13 vs 15 — the t52 residual, fixed this
/// tranche; the standalone residual pin in fcs52_top_cluster_records.rs
/// carries the same converged value).
#[test]
fn fcs54_interface_protecteds2_record_pin() {
    assert_twin_records(
        "x/interfaceExtendingClassWithProtecteds2.ts",
        &[(
            "conformance/interfaceExtendingClassWithProtecteds2.errors.txt",
            "mismatch",
            15,
            "e02e45bdd4a1a87a",
        )],
    );
}

/// The collateral convergences of the protected-override root — the
/// sibling interfaceExtendingClassWithProtecteds record and the
/// mixinAccessModifiers / redeclaredProperty records ride the same
/// relation arm; rust == go (were divergent at t54).
#[test]
fn fcs54_interface_protecteds_collateral_records_pin() {
    // Anchored + escaped dot: the unanchored form's literal prefix stops
    // at the unescaped ".", contains-matching the ...Protecteds2.ts case
    // as well.
    assert_twin_records(
        "x/^interfaceExtendingClassWithProtecteds\\.ts",
        &[(
            "conformance/interfaceExtendingClassWithProtecteds.errors.txt",
            "mismatch",
            12,
            "ac9608ea1445bda7",
        )],
    );
    assert_twin_records(
        "dir/mixinAccessModifiers.ts",
        &[(
            "conformance/mixinAccessModifiers.errors.txt",
            "mismatch",
            43,
            "e5ab66b0d87cb046",
        )],
    );
    assert_twin_records(
        "dir/redeclaredProperty.ts",
        &[(
            "conformance/redeclaredProperty.errors.txt",
            "mismatch",
            11,
            "5fdaaad27df56d6f",
        )],
    );
}

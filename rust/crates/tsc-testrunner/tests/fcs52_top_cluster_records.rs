//! FCS52 record pins (full-corpus check sweep, micro-tranche 52): the
//! record-level twins for the tranche's three clusters —
//!
//!  * the unicodeEscapesInJsxtags JSX-escape family: the scanner/parser
//!    already decode `\u0061` (parser pin fcs52_jsx_escape_decode_pin.rs);
//!    the three checker `get_text_of_node` sites now read the decoded
//!    `tagName.Text()` like Go jsx.go:145/1200/1228 — the record matches
//!    the Go twin exactly (was 44 vs 38 — 2339 x6 extra with the raw
//!    escape text, 4 of them where Go's decoded `a-b`/`a-c` resolve
//!    against declared IntrinsicElements);
//!  * the contextualTypedSpecialAssignment family: the reparser now
//!    attaches the `/** @type T */ a.b = expr` tag to the assignment
//!    binary (Go reparser.go:373-381) so the RHS object literal is
//!    contextually typed — the record matches the Go twin exactly
//!    (was 31 vs 24 — 7006 x8 extra on the object-literal method
//!    parameters, one missing 2322 where Go checks the un-annotated
//!    second assignment against the declared type);
//!  * the mergedInterfaces* 2320 family: checkInheritedPropertiesAreIdentical
//!    seeds from the interface's declared members and compareProperties
//!    carries Go's accessibility/target-symbol/readonly ladder, so the
//!    2320 "cannot simultaneously extend" chain fires (head-first) and
//!    gates the 2430 loop — mergedInterfacesWithInheritedPrivates3,
//!    mergedInheritedMembersSatisfyAbstractBase,
//!    mergedInterfacesWithMultipleBases4 and
//!    interfaceExtendingClassWithPrivates2 all match the Go twin exactly;
//!
//! plus the collateral convergences the same roots fixed (one test), and
//! the residual record this tranche measured but did NOT fix —
//! interfaceExtendingClassWithProtecteds2: I3's 2320 converged, but the
//! I4 2430 x2 pair stays missing (its own public `x` shadows the bases'
//! protected `x`, Go's inherited-identical gate passes and the 2430 loop
//! must then FAIL assignability on accessibility — a protected-access
//! property-comparison root in the type relation, PRE-EXISTING at t52,
//! unchanged by this tranche; queued).

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

/// The JSX-escape record — decoded tag names resolve against the declared
/// IntrinsicElements; rust == go (was 44 vs 38).
#[test]
fn fcs52_unicode_escapes_in_jsx_tags_record_pin() {
    assert_twin_records(
        "x/unicodeEscapesInJsxtags.tsx",
        &[(
            "conformance/unicodeEscapesInJsxtags.errors.txt",
            "mismatch",
            38,
            "49ef681168bb921d",
        )],
    );
}

/// The @type-on-assignment contextual-typing record — the object-literal
/// methods pick up the typedef's parameter types; rust == go
/// (was 31 vs 24).
#[test]
fn fcs52_contextual_typed_special_assignment_record_pin() {
    assert_twin_records(
        "salsa/contextualTypedSpecialAssignment.ts",
        &[(
            "conformance/contextualTypedSpecialAssignment.errors.txt",
            "mismatch",
            24,
            "00db45d787be5ee9",
        )],
    );
}

/// The merged-interfaces 2320 family — the inherited-private-members
/// record (two merged `interface A` declarations extending C and C2),
/// the abstract-base twin, the multiple-bases twin and the
/// extending-classes twin all match the Go twin exactly (were 2430-extra
/// / 2320-missing divergences).
#[test]
fn fcs52_merged_interfaces_2320_family_record_pin() {
    assert_twin_records(
        "x/mergedInterfacesWithInheritedPrivates3.ts",
        &[(
            "conformance/mergedInterfacesWithInheritedPrivates3.errors.txt",
            "mismatch",
            18,
            "7d0009da34121870",
        )],
    );
    assert_twin_records(
        "x/mergedInheritedMembersSatisfyAbstractBase.ts",
        &[(
            "conformance/mergedInheritedMembersSatisfyAbstractBase.errors.txt",
            "mismatch",
            11,
            "68240113a017f563",
        )],
    );
    assert_twin_records(
        "x/mergedInterfacesWithMultipleBases4.ts",
        &[(
            "conformance/mergedInterfacesWithMultipleBases4.errors.txt",
            "mismatch",
            21,
            "352e3f03508986eb",
        )],
    );
    assert_twin_records(
        "x/interfaceExtendingClassWithPrivates2.ts",
        &[(
            "conformance/interfaceExtendingClassWithPrivates2.errors.txt",
            "mismatch",
            15,
            "8eed575766ffbf48",
        )],
    );
}

/// The collateral convergences the same three roots fixed — type-tag
/// assignment/prototype records (the reparser arm), the jsdocTemplateTag
/// twins and the typedef/import-types record (2322 x2 each), the
/// interfaceWithMultipleBaseTypes 2320, and the
/// callSignaturesThatDifferOnlyByReturnType2 2430. All rust == go.
#[test]
fn fcs52_collateral_convergences_record_pin() {
    assert_twin_records(
        "x/typeTagModuleExports.ts",
        &[(
            "conformance/typeTagModuleExports.errors.txt",
            "mismatch",
            11,
            "c3d24c1febe4ba29",
        )],
    );
    assert_twin_records(
        "x/typeTagPrototypeAssignment.ts",
        &[(
            "conformance/typeTagPrototypeAssignment.errors.txt",
            "mismatch",
            11,
            "6adeae7b18ba4e8d",
        )],
    );
    assert_twin_records(
        "x/jsdocTemplateTag4.ts",
        &[(
            "conformance/jsdocTemplateTag4.errors.txt",
            "mismatch",
            23,
            "4011eac5491fdbe2",
        )],
    );
    assert_twin_records(
        "x/jsdocTemplateTag5.ts",
        &[(
            "conformance/jsdocTemplateTag5.errors.txt",
            "mismatch",
            23,
            "2ad8aa55c15268f0",
        )],
    );
    assert_twin_records(
        "declarations/jsDeclarationsTypedefAndImportTypes.ts",
        &[(
            "conformance/jsDeclarationsTypedefAndImportTypes.errors.txt",
            "missing-baseline",
            24,
            "e253759162018873",
        )],
    );
    // PORT: the filter is tsc's `-test.run` pattern (the "." is a regex
    // any-char), so "interfaceWithMultipleBaseTypes.ts" also matches
    // interfaceWithMultipleBaseTypes2.ts — both records are pinned (both
    // converged this tranche).
    assert_twin_records(
        "declarations/interfaceWithMultipleBaseTypes.ts",
        &[
            (
                "conformance/interfaceWithMultipleBaseTypes.errors.txt",
                "mismatch",
                16,
                "a7d2ae121f799336",
            ),
            (
                "conformance/interfaceWithMultipleBaseTypes2.errors.txt",
                "mismatch",
                11,
                "a3d904ea056060c3",
            ),
        ],
    );
    assert_twin_records(
        "x/callSignaturesThatDifferOnlyByReturnType2.ts",
        &[(
            "conformance/callSignaturesThatDifferOnlyByReturnType2.errors.txt",
            "mismatch",
            10,
            "86d697331c5b0956",
        )],
    );
    assert_twin_records(
        "salsa/propertyAssignmentUseParentType2.ts",
        &[(
            "conformance/propertyAssignmentUseParentType2.errors.txt",
            "mismatch",
            9,
            "fb16381b7818d468",
        )],
    );
}

/// The residual record this tranche measured but did NOT fix — the I4
/// 2430 x2 pair (public own `x` shadows the bases' protected `x`; Go's
/// inherited-identical gate passes and the assignability check fails on
/// accessibility). I3's 2320 converged this tranche (the record went
/// from 3 diagnostic diffs to 2); the residual pair is a separate
/// protected-access property-comparison root in the type relation,
/// pre-existing at t52, queued.
///
/// PORT (FCS tranche 54): the residual is FIXED — the root was the
/// permissive isValidOverrideOf stub (relater.rs); the record now
/// converges with the go twin (15/e02e45bdd4a1a87a, verified on the
/// t55 full-corpus sweep).
#[test]
fn fcs52_residual_records_pin() {
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

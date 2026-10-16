//! FCS32 pins (full-corpus check sweep, tranche 32): the rust-missing
//! TS2354 "This syntax requires an imported helper but module '{0}'
//! cannot be found" family — 104 instances across 72 divergent records
//! before the tranche; the REAL checkExternalEmitHelpers /
//! resolveHelpersModule (checker.go:29105/29195) plus the missing
//! call sites (checkDecorators' firstDecorator block,
//! checkClassExpressionExternalHelpers, the private-field Get/Set/In
//! sites) now run.
//!
//! The records below are the GO twin's exact values (rig census
//! 2026-10-16, the t32 dump-diff) — every pinned record moved from
//! rust-missing-2354 divergent to byte-identical. The `-test.run` filter
//! is a literal-prefix match (compiler_test_filter.rs), so each filter
//! pins EVERY record it matches (the full sibling set).

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

/// The esDecorators classDeclaration trio (classDecorator.1-3): the
/// decorated class's ESDecorate/RunInitializers request at firstDecorator
/// (GO 22 vs RUST 21 on each before) — byte-identical now.
#[test]
fn fcs32_class_declaration_decorators_pin() {
    assert_twin_records(
        "x/esDecorators-classDeclaration-missingEmitHelpers-classDecorator",
        &[
            (
                "conformance/esDecorators-classDeclaration-missingEmitHelpers-classDecorator.1.errors.txt",
                "missing-baseline",
                22,
                "7efc8d6f55c16666",
            ),
            (
                "conformance/esDecorators-classDeclaration-missingEmitHelpers-classDecorator.2.errors.txt",
                "missing-baseline",
                22,
                "72d1637c3c09de77",
            ),
            (
                "conformance/esDecorators-classDeclaration-missingEmitHelpers-classDecorator.3.errors.txt",
                "missing-baseline",
                22,
                "4cb2860101b69cdb",
            ),
        ],
    );
}

/// The esDecorators classExpression set (classDecorator.1-17: `export
/// const C = @dec class {}` variants — the decorator AND the
/// named-evaluation SetFunctionName request, both memoized to one 2354 at
/// firstDecorator; GO 22 vs RUST 21 on each before) — byte-identical
/// now. Enumeration order is the runner's lexical order (1, 10..17,
/// 2..9).
#[test]
fn fcs32_class_expression_decorators_pin() {
    assert_twin_records(
        "x/esDecorators-classExpression-missingEmitHelpers-classDecorator",
        &[
            (
                "conformance/esDecorators-classExpression-missingEmitHelpers-classDecorator.1.errors.txt",
                "missing-baseline",
                22,
                "0eb0d61cfc029a23",
            ),
            (
                "conformance/esDecorators-classExpression-missingEmitHelpers-classDecorator.10.errors.txt",
                "missing-baseline",
                22,
                "77d243f8f84652aa",
            ),
            (
                "conformance/esDecorators-classExpression-missingEmitHelpers-classDecorator.11.errors.txt",
                "missing-baseline",
                22,
                "a8ff81056b3f1e92",
            ),
            (
                "conformance/esDecorators-classExpression-missingEmitHelpers-classDecorator.12.errors.txt",
                "missing-baseline",
                22,
                "408fd2dfa018bf4e",
            ),
            (
                "conformance/esDecorators-classExpression-missingEmitHelpers-classDecorator.13.errors.txt",
                "missing-baseline",
                22,
                "c85dcd834d0390d3",
            ),
            (
                "conformance/esDecorators-classExpression-missingEmitHelpers-classDecorator.14.errors.txt",
                "missing-baseline",
                22,
                "ffb3c38c94280347",
            ),
            (
                "conformance/esDecorators-classExpression-missingEmitHelpers-classDecorator.15.errors.txt",
                "missing-baseline",
                22,
                "bd9d5bb36b1a9572",
            ),
            (
                "conformance/esDecorators-classExpression-missingEmitHelpers-classDecorator.16.errors.txt",
                "missing-baseline",
                22,
                "7a2f49b56b0c0efb",
            ),
            (
                "conformance/esDecorators-classExpression-missingEmitHelpers-classDecorator.17.errors.txt",
                "missing-baseline",
                22,
                "4ac5e8b69904a2ab",
            ),
            (
                "conformance/esDecorators-classExpression-missingEmitHelpers-classDecorator.2.errors.txt",
                "missing-baseline",
                22,
                "7de47b3872f2515b",
            ),
            (
                "conformance/esDecorators-classExpression-missingEmitHelpers-classDecorator.3.errors.txt",
                "missing-baseline",
                22,
                "ce68c33af8699eb7",
            ),
            (
                "conformance/esDecorators-classExpression-missingEmitHelpers-classDecorator.4.errors.txt",
                "missing-baseline",
                22,
                "2177c89645ef8dbd",
            ),
            (
                "conformance/esDecorators-classExpression-missingEmitHelpers-classDecorator.5.errors.txt",
                "missing-baseline",
                22,
                "c7b135c552612e16",
            ),
            (
                "conformance/esDecorators-classExpression-missingEmitHelpers-classDecorator.6.errors.txt",
                "missing-baseline",
                22,
                "43c4f4623ddead85",
            ),
            (
                "conformance/esDecorators-classExpression-missingEmitHelpers-classDecorator.7.errors.txt",
                "missing-baseline",
                22,
                "dec2e997adcea40b",
            ),
            (
                "conformance/esDecorators-classExpression-missingEmitHelpers-classDecorator.8.errors.txt",
                "missing-baseline",
                22,
                "4111b91e5fe9a5bc",
            ),
            (
                "conformance/esDecorators-classExpression-missingEmitHelpers-classDecorator.9.errors.txt",
                "missing-baseline",
                22,
                "13d8bae9b3d45bc2",
            ),
        ],
    );
}

/// The nodeModules ImportHelpersCollisions trio (Collisions / Collisions2
/// / Collisions3, all module variants): the per-file `import * as fs`
/// __importStar request (GO 36/34 vs RUST 34/33 on each before) —
/// byte-identical now.
#[test]
fn fcs32_node_modules_import_helpers_collisions_pin() {
    assert_twin_records(
        "x/^nodeModulesImportHelpersCollisions",
        &[
            (
                "conformance/nodeModulesImportHelpersCollisions(module=node16).errors.txt",
                "mismatch",
                36,
                "f45e11e7bd16821f",
            ),
            (
                "conformance/nodeModulesImportHelpersCollisions(module=node18).errors.txt",
                "mismatch",
                36,
                "6bd09722da0d5f25",
            ),
            (
                "conformance/nodeModulesImportHelpersCollisions(module=node20).errors.txt",
                "mismatch",
                36,
                "417ede6511a22410",
            ),
            (
                "conformance/nodeModulesImportHelpersCollisions(module=nodenext).errors.txt",
                "mismatch",
                36,
                "7e99c60837f21bc1",
            ),
            (
                "conformance/nodeModulesImportHelpersCollisions2(module=node16).errors.txt",
                "mismatch",
                36,
                "0efdef27c3c44d7f",
            ),
            (
                "conformance/nodeModulesImportHelpersCollisions2(module=node18).errors.txt",
                "mismatch",
                36,
                "54ccc7aa76424741",
            ),
            (
                "conformance/nodeModulesImportHelpersCollisions2(module=node20).errors.txt",
                "mismatch",
                36,
                "d53a2877bd22b520",
            ),
            (
                "conformance/nodeModulesImportHelpersCollisions2(module=nodenext).errors.txt",
                "mismatch",
                36,
                "f5886304cb06c571",
            ),
            (
                "conformance/nodeModulesImportHelpersCollisions3(module=node16,target=es2015).errors.txt",
                "mismatch",
                34,
                "f1eade51673f36c8",
            ),
            (
                "conformance/nodeModulesImportHelpersCollisions3(module=node16,target=es5).errors.txt",
                "missing-baseline",
                35,
                "08d745455c73856d",
            ),
            (
                "conformance/nodeModulesImportHelpersCollisions3(module=node18,target=es2015).errors.txt",
                "mismatch",
                34,
                "8aec96f5bcf4ccf6",
            ),
            (
                "conformance/nodeModulesImportHelpersCollisions3(module=node18,target=es5).errors.txt",
                "missing-baseline",
                35,
                "05a2f2a5827791cb",
            ),
            (
                "conformance/nodeModulesImportHelpersCollisions3(module=node20,target=es2015).errors.txt",
                "mismatch",
                34,
                "dd882c12deba4a3f",
            ),
            (
                "conformance/nodeModulesImportHelpersCollisions3(module=node20,target=es5).errors.txt",
                "missing-baseline",
                35,
                "95a4ea3f9f33fb7e",
            ),
            (
                "conformance/nodeModulesImportHelpersCollisions3(module=nodenext,target=es2015).errors.txt",
                "mismatch",
                34,
                "61c2c623eedd72ce",
            ),
            (
                "conformance/nodeModulesImportHelpersCollisions3(module=nodenext,target=es5).errors.txt",
                "missing-baseline",
                35,
                "32375bdbb85d7473",
            ),
        ],
    );
}

/// The privateNameEmitHelpers record: the private-field Set/Get requests
/// (`this.#c = 42`, target es2015 — GO 21 vs RUST 20 before) —
/// byte-identical now.
#[test]
fn fcs32_private_name_emit_helpers_pin() {
    assert_twin_records(
        "x/privateNameEmitHelpers",
        &[(
            "conformance/privateNameEmitHelpers.errors.txt",
            "mismatch",
            21,
            "b16aaddfc2127de4",
        )],
    );
}

/// The using/await-using declaration pair (usingDeclarationsWithImport
/// Helpers + awaitUsing… — the AddDisposableResource/DisposeResources
/// request; GO 10/11 vs RUST 9/10 before) — byte-identical now. The
/// basenames differ only in the `await` case ('using…' vs
/// 'awaitUsing…'), so each filter pins its own record.
#[test]
fn fcs32_using_declarations_import_helpers_pin() {
    assert_twin_records(
        "x/usingDeclarationsWithImportHelpers",
        &[(
            "conformance/usingDeclarationsWithImportHelpers.errors.txt",
            "mismatch",
            10,
            "09aa1c99065fd0b1",
        )],
    );
    assert_twin_records(
        "x/awaitUsingDeclarationsWithImportHelpers",
        &[(
            "conformance/awaitUsingDeclarationsWithImportHelpers.errors.txt",
            "mismatch",
            11,
            "59d836d5dadc2018",
        )],
    );
}

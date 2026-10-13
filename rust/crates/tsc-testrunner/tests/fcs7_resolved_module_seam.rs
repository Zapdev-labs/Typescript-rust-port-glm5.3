//! FCS7 pins (full-corpus check sweep, tranche 7): the per-unit
//! resolveModuleName seam — the tranche's top family.
//!
//! Root cause: Go's per-unit check twin (go-driver conformance.go's
//! confSemanticDiagnosticsOfUnit) builds a 1-file map-FS program — the REAL
//! file loader/module resolver runs against the unit-only FS and fills
//! program.resolvedModules (node16 `.js`→`.ts` substitution resolves
//! "./index.js" to the unit itself; CJS directory/index resolution answers
//! require("./") and require("./index")). The Rust seam's
//! `resolveExternalModule` never resolved (permissive None + the
//! module-not-found ladder): every self-importable reference reported
//! TS2307 where Go resolved, and the import-symbol machinery downstream
//! (TS2339/TS2322/TS2318) diverged.
//!
//! The fix mirrors Go's twin construction exactly (the testrunner builds
//! the 1-file twin program per unit — same parsed config with the file list
//! swapped, no libs — and hands the checker the unit's resolutions cache +
//! SourceFileMayBeEmitted through the Program trait's FCS-tranche-7
//! resolved-module surface), and ports Go checker.go:15508-15746's
//! resolved-module half REAL (the resolution-diagnostic gate, the
//! ResolvedUsingTsExtension arms (TS2305/TS5097/rewriteRelativeImport-
//! Extensions), the TS2306 script arm, the untyped-module arm
//! (TS2666/TS7016 via errorOnImplicitAnyModule), and the AlternateResult
//! chain arm).
//!
//! The importSource tail (same tranche): `import.source(...)` calls resolve
//! NOTHING and promise the global AbstractModuleSource (Go checker.go:
//! 8523-8525 — the machinery TS2318), and the options argument reads
//! getGlobalImportCallOptionsTypeChecked ("ImportCallOptions" TS2318) +
//! the `with` import-attributes type; `import source a from "..."` clauses
//! get the AbstractModuleSource-resolved alias target (getTargetOfImportClause
//! checker.go:14852).
//!
//! The records below are the GO twin's exact values (rig census 2026-10-13,
//! the t7b_full_go stream) — nodeModules1 collapsed from 7-divergent
//! (tranche 6's residual) to byte-identical on all 4 module variants.

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

/// The seam's keystone record: node/nodeModules1.ts — Go's per-unit twin
/// resolves "./index.mjs" to the unit itself (.mjs→.mts substitution) and
/// require("./") / require("./index") via CJS directory/index resolution,
/// while the not-found ladder (TS2834/2835/2307, tranche 6) answers the
/// rest. All 4 module variants byte-identical to the Go twin
/// (tranche 6 left these 7-divergent per record).
#[test]
fn node_modules1_resolves_like_the_go_twin_pin() {
    assert_twin_records(
        "node/nodeModules1.ts",
        &[
            ("conformance/nodeModules1(module=node16).errors.txt", "mismatch", 284, "a60c62843bc55c24"),
            ("conformance/nodeModules1(module=node18).errors.txt", "mismatch", 284, "3aa71a04d3cd04ae"),
            ("conformance/nodeModules1(module=node20).errors.txt", "mismatch", 284, "56b23da020558271"),
            ("conformance/nodeModules1(module=nodenext).errors.txt", "mismatch", 284, "0f8cb9b937f68782"),
        ],
    );
}

/// The importSource tail: import.source CALLS resolve nothing and promise
/// the global AbstractModuleSource (the machinery TS2318 — f.js/g.js/...
/// JS units), and import source DECLARATIONS' alias targets carry it too
/// (getTargetOfImportClause's synthesized symbol).
#[test]
fn import_source11_abstract_module_source_pin() {
    assert_twin_records(
        "importSource/importSource11.ts",
        &[(
            "conformance/importSource11.errors.txt",
            "mismatch",
            173,
            "b98c80e36a901737",
        )],
    );
}

/// The ResolvedUsingTsExtension arm: allowImportingTsExtensions's
/// "./index.ts"-style self-imports resolve to the unit WITH
/// resolvedUsingTsExtension — the TS5097 arm fires per variant when the
/// option is off (and never a spurious TS2307).
#[test]
fn allow_importing_ts_extensions_5097_arm_pin() {
    assert_twin_records(
        "moduleResolution/allowImportingTsExtensions.ts",
        &[
            ("conformance/allowImportingTsExtensions(moduleresolution=bundler).errors.txt", "mismatch", 62, "87b0c28a415a8db3"),
            ("conformance/allowImportingTsExtensions(moduleresolution=classic).errors.txt", "missing-baseline", 63, "9e5f455749ff49fa"),
            ("conformance/allowImportingTsExtensions(moduleresolution=node16).errors.txt", "mismatch", 63, "8c33c06f85e1c011"),
            ("conformance/allowImportingTsExtensions(moduleresolution=nodenext).errors.txt", "mismatch", 63, "8f0de431be5b556f"),
        ],
    );
}

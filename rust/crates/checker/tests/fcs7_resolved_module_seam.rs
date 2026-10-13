//! FCS7 pins (full-corpus check sweep, tranche 7): the resolved-module seam
//! at unit scale — `resolveExternalModule`'s resolved-module half (Go
//! checker.go:15508-15746) reading the Program trait's
//! FCS-tranche-7 surface (get_resolved_module /
//! get_source_file_for_resolved_module / source_file_may_be_emitted).
//!
//! The single-file Program scaffold seeds the resolutions cache the way the
//! testrunner's per-unit twin builds it (Go-driver conformance.go's
//! confSemanticDiagnosticsOfUnit: the 1-file map-FS program's file loader
//! resolves every import of the unit against the unit-only FS — node16
//! `.js`→`.ts` substitution resolves "./index.js" to the unit itself). The
//! record-level parity is pinned in tsc-testrunner/tests/
//! fcs7_resolved_module_seam.rs (nodeModules1 / importSource11 /
//! allowImportingTsExtensions exact twin records); these pins exercise the
//! checker arms through the seam directly.

use std::sync::Arc;

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::compileroptions::ResolutionModeNone;
use tsc_core::options_generated::{CompilerOptions, ModuleKind, ModuleResolutionKind};
use tsc_core::scriptkind::ScriptKind;
use tsc_parser::parse_source_file;
use tsc_tspath::RootedFilePath;

struct TestProgram {
    options: CompilerOptions,
    name: RootedFilePath,
    source_file: SourceFile,
    bind: BindResult,
    binder_symbol_count: u32,
    checker_symbols: Vec<Symbol>,
    /// The unit's resolution answers (the twin program's resolvedModules
    /// entry) — name-keyed (the twin has one file; the runner's real impl
    /// keys by (name, mode) exactly like the loader's ModeAwareCache).
    resolutions: tsc_module::ModeAwareCache<Option<Arc<tsc_module::ResolvedModule>>>,
    may_be_emitted: bool,
}

impl Program for TestProgram {
    fn options(&self) -> &CompilerOptions {
        &self.options
    }
    fn source_files(&self) -> &[SourceFile] {
        std::slice::from_ref(&self.source_file)
    }
    fn bind_source_files(&mut self) {}
    fn node_store(&self) -> &dyn NodeStore {
        &self.source_file
    }
    fn node_store_mut(&mut self) -> &mut dyn NodeStore {
        &mut self.source_file
    }
    fn flow_arena(&self) -> (&[tsc_ast::flow::FlowNode], &[tsc_ast::flow::FlowList]) {
        (&self.bind.flow_nodes, &self.bind.flow_lists)
    }
    fn symbol(&self, id: SymbolId) -> &Symbol {
        if id.0 < self.binder_symbol_count {
            &self.bind.symbols[id.0 as usize]
        } else {
            &self.checker_symbols[(id.0 - self.binder_symbol_count) as usize]
        }
    }
    fn symbol_mut(&mut self, id: SymbolId) -> &mut Symbol {
        if id.0 < self.binder_symbol_count {
            &mut self.bind.symbols[id.0 as usize]
        } else {
            &mut self.checker_symbols[(id.0 - self.binder_symbol_count) as usize]
        }
    }
    fn alloc_symbol(&mut self, symbol: Symbol) -> SymbolId {
        self.checker_symbols.push(symbol);
        SymbolId(self.binder_symbol_count + self.checker_symbols.len() as u32 - 1)
    }
    fn source_file_name(&self, _file: tsc_ast::NodeId) -> String {
        self.name.as_string().to_string()
    }
    fn source_file_meta_data(&self, _file: tsc_ast::NodeId) -> tsc_ast::SourceFileMetaData {
        let implied_node_format = if self.name.extension_is_one_of(&[".mjs", ".mts"]) {
            tsc_core::compileroptions::ResolutionModeESM
        } else if self.name.extension_is_one_of(&[".cjs", ".cts"]) {
            tsc_core::compileroptions::ResolutionModeCommonJS
        } else {
            ResolutionModeNone
        };
        tsc_ast::SourceFileMetaData {
            package_json_type: String::new(),
            package_json_directory: Default::default(),
            implied_node_format,
        }
    }
    fn file_exists(&self, path: &str) -> bool {
        path == self.name.as_string()
    }
    fn get_resolved_module(
        &self,
        file: tsc_ast::NodeId,
        module_reference: &str,
        mode: tsc_core::compileroptions::ResolutionMode,
    ) -> Option<Arc<tsc_module::ResolvedModule>> {
        if file != self.source_file.as_node_id() {
            return None;
        }
        // The twin's cache lookup: the (name, mode) key first (the loader's
        // exact key), then the name-only fallback (the test seeds the
        // None-mode key for every reference; the mode derivations over the
        // meta-data seams produce the same None mode for .ts units).
        self.resolutions
            .get(&tsc_module::ModeAwareCacheKey {
                name: module_reference.to_string(),
                mode,
            })
            .cloned()
            .flatten()
            .or_else(|| {
                self.resolutions
                    .iter()
                    .find(|(key, _)| key.name == module_reference)
                    .and_then(|(_, entry)| entry.clone())
            })
    }
    fn get_source_file_for_resolved_module(
        &self,
        resolved: &tsc_module::ResolvedModule,
    ) -> Option<tsc_ast::NodeId> {
        if resolved.resolved_file_name == self.name {
            Some(self.source_file.as_node_id())
        } else {
            None
        }
    }
    fn source_file_may_be_emitted(&self, file: tsc_ast::NodeId) -> bool {
        file == self.source_file.as_node_id() && self.may_be_emitted
    }
}

/// One resolved self-module answer (the twin program's cache entry for
/// `module_reference`): the unit itself at `resolved_file_name`.
fn resolved_self(name: &str, using_ts_extension: bool) -> tsc_module::ResolvedModule {
    let resolved_file_name = RootedFilePath::from(name);
    let extension = resolved_file_name.extension().to_string();
    tsc_module::ResolvedModule {
        resolved_file_name,
        extension,
        resolved_using_ts_extension: using_ts_extension,
        ..tsc_module::ResolvedModule::default()
    }
}

/// `import { x } from "..."` shapes over a named single-file program
/// (module=node16, moduleResolution=node16 — the conformance default
/// shape) with a seeded resolutions cache.
fn check_named(
    name: &str,
    src: &str,
    script_kind: ScriptKind,
    resolutions: Vec<(&str, Option<tsc_module::ResolvedModule>)>,
) -> (Checker, tsc_ast::NodeId) {
    let mut sf = parse_source_file(Default::default(), src, script_kind);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let options = CompilerOptions {
        module: ModuleKind::Node16,
        module_resolution: ModuleResolutionKind::Node16,
        ..CompilerOptions::default()
    };
    let mut cache = tsc_module::ModeAwareCache::default();
    for (reference, resolved) in resolutions {
        cache.insert(
            tsc_module::ModeAwareCacheKey {
                name: reference.to_string(),
                mode: ResolutionModeNone,
            },
            resolved.map(Arc::new),
        );
    }
    let program = TestProgram {
        options,
        name: RootedFilePath::from(name),
        source_file: sf,
        bind,
        binder_symbol_count,
        checker_symbols: Vec::new(),
        resolutions: cache,
        may_be_emitted: true,
    };
    let mut checker = new_checker(Box::new(program));
    checker.check_source_file(file, false);
    (checker, file)
}

/// The (code, arg0) pairs of the unit's semantic diagnostics (per-file +
/// the globals — the twin's collection read), in order.
fn diag_pairs(c: &mut Checker, _file: tsc_ast::NodeId) -> Vec<(i32, String)> {
    c.semantic_diagnostics()
        .iter()
        .map(|d| (d.code(), d.message_args().first().cloned().unwrap_or_default()))
        .collect()
}

// ── The pins ───────────────────────────────────────────────────────────────

/// Self-resolution: "./index.js" from index.ts resolves to the UNIT ITSELF
/// via node16 `.js`→`.ts` substitution (resolvedUsingTsExtension false —
/// the substitution case) → NO TS2307, and the import's target is the
/// unit's own module symbol, so the imported name resolves (no TS2339).
#[test]
fn js_to_ts_substitution_self_import_resolves_pin() {
    let (mut c, file) = check_named(
        "/x/index.ts",
        "const x = 1;\nexport { x };\nimport { x as y } from \"./index.js\";\nexport { y };\n",
        ScriptKind::TS,
        vec![("./index.js", Some(resolved_self("/x/index.ts", false)))],
    );
    let pairs = diag_pairs(&mut c, file);
    assert!(
        !pairs.iter().any(|(code, _)| *code == 2307),
        "the self-resolved import must NOT report TS2307: {pairs:?}"
    );
    assert!(
        !pairs.iter().any(|(code, _)| *code == 2339),
        "the imported name resolves to the unit's own export — no TS2339: {pairs:?}"
    );
    assert!(
        !pairs.iter().any(|(code, _)| *code == 5097),
        "the .js→.ts substitution is not resolvedUsingTsExtension — no TS5097: {pairs:?}"
    );
}

/// The TS5097 arm: "./index.ts" resolves to the unit itself WITH
/// resolvedUsingTsExtension and allowImportingTsExtensions off → the
/// TS5097 "can only end with a '.ts' extension" error (never a TS2307).
#[test]
fn resolved_using_ts_extension_5097_pin() {
    let (mut c, file) = check_named(
        "/x/index.ts",
        "const x = 1;\nexport { x };\nimport { x as y } from \"./index.ts\";\nexport { y };\n",
        ScriptKind::TS,
        vec![("./index.ts", Some(resolved_self("/x/index.ts", true)))],
    );
    let pairs = diag_pairs(&mut c, file);
    assert!(
        pairs.contains(&(5097, ".ts".to_string())),
        "TS5097 with the '.ts' extension must fire: {pairs:?}"
    );
    assert!(
        !pairs.iter().any(|(code, _)| *code == 2307),
        "the resolved import must NOT report TS2307: {pairs:?}"
    );
}

/// The import.source CALL short-circuit (checker.go:8523): the call
/// resolves NOTHING (no TS2307) and promises the global AbstractModuleSource
/// — the machinery TS2318 in the lib-less twin.
#[test]
fn import_source_call_abstract_module_source_pin() {
    let (mut c, file) = check_named(
        "/x/index.ts",
        "const s = import.source(\"./a.js\");\nexport { s };\n",
        ScriptKind::TS,
        vec![],
    );
    let pairs = diag_pairs(&mut c, file);
    assert!(
        !pairs.iter().any(|(code, _)| *code == 2307),
        "import.source resolves nothing — no TS2307: {pairs:?}"
    );
    assert!(
        pairs.contains(&(2318, "AbstractModuleSource".to_string())),
        "the promised AbstractModuleSource reports its machinery TS2318: {pairs:?}"
    );
}

/// The import source DECLARATION arm (getTargetOfImportClause
/// checker.go:14852): `import source a from "..."` — the alias target's
/// resolved type is the global AbstractModuleSource (no TS2307; the
/// machinery TS2318 when the name is used).
#[test]
fn import_source_declaration_abstract_module_source_pin() {
    let (mut c, file) = check_named(
        "/x/index.ts",
        "import source a from \"./a.js\";\na;\n",
        ScriptKind::TS,
        vec![],
    );
    let pairs = diag_pairs(&mut c, file);
    assert!(
        !pairs.iter().any(|(code, _)| *code == 2307),
        "the source-phase declaration resolves nothing — no TS2307: {pairs:?}"
    );
    assert!(
        pairs.contains(&(2318, "AbstractModuleSource".to_string())),
        "`a`'s AbstractModuleSource type reports its machinery TS2318: {pairs:?}"
    );
}

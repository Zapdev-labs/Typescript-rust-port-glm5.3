//! FCS6 pins (full-corpus check sweep, tranche 6): the module-not-found
//! ladder family — node/nodeModules1(module=node16).ts shape.
//!
//! Root cause: `resolveExternalModule`'s tail ladder (Go checker.go:
//! 15690-15746) was unported — every unresolved module fell to the plain
//! TS2307 `Cannot find module` error where Go reports the TS2732 json arm
//! (`resolveJsonModule` off + a `.json` reference), the TS2835/2834
//! extensionless-ESM arms (mode==ESM + node16/nodenext + an
//! extensionless relative import: the suggested extension via the
//! FileExists probes, else "Consider adding an extension"), or the plain
//! TS2307. The mode derivation itself (Go checker.go:15470-15506's
//! context-specifier walk + compiler/fileloader.go:1037-1100's
//! getModeForUsageLocation/getEmitSyntaxForUsageLocationWorker chain) had
//! to be ported REAL — a `.cts` file's dynamic `import()` is ESM-mode
//! under node16 (ShouldTransformImportCall false), its static imports
//! and `import x = require(...)` are CommonJS-mode.

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::options_generated::{
    CompilerOptions, ModuleKind, ModuleResolutionKind,
};
use tsc_core::scriptkind::ScriptKind;
use tsc_parser::parse_source_file;
use tsc_tspath::RootedFilePath;

// ── The single-file Program scaffold (the fcs5 idiom) + the FCS-tranche-6
// module-resolution-mode seams (name / meta / file_exists — Go:
// file.FileName(), p.sourceFileMetaDatas[], program.FileExists; the
// single-file map-FS twin answer is exact-name equality) ──

struct TestProgram {
    options: CompilerOptions,
    name: RootedFilePath,
    source_file: SourceFile,
    bind: BindResult,
    binder_symbol_count: u32,
    checker_symbols: Vec<Symbol>,
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
        // Go: GetImpliedNodeFormatForFile(fileName, "") — the no-package.json
        // shape (the map-FS twin holds no package.json).
        let implied_node_format = if self
            .name
            .extension_is_one_of(&[".mjs", ".mts"])
        {
            tsc_core::compileroptions::ResolutionModeESM
        } else if self.name.extension_is_one_of(&[".cjs", ".cts"]) {
            tsc_core::compileroptions::ResolutionModeCommonJS
        } else {
            tsc_core::compileroptions::ResolutionModeNone
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
}

/// `import { x } from "..."` shapes checked over a named single-file
/// program (module=node16, moduleResolution=node16, strict=false — the
/// conformance default shape).
fn check_named(name: &str, src: &str) -> (Checker, tsc_ast::NodeId) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let options = CompilerOptions {
        module: ModuleKind::Node16,
        module_resolution: ModuleResolutionKind::Node16,
        ..CompilerOptions::default()
    };
    let program = TestProgram {
        options,
        name: RootedFilePath::from(name),
        source_file: sf,
        bind,
        binder_symbol_count,
        checker_symbols: Vec::new(),
    };
    let mut checker = new_checker(Box::new(program));
    checker.check_source_file(file, false);
    (checker, file)
}

/// The (code, arg0) pairs of the file's diagnostics, in order.
fn diag_pairs(c: &mut Checker, file: tsc_ast::NodeId) -> Vec<(i32, String)> {
    c.get_file_diagnostics(file)
        .iter()
        .map(|d| {
            (
                d.code(),
                d.message_args().first().cloned().unwrap_or_default(),
            )
        })
        .collect()
}

// ── The pins ───────────────────────────────────────────────────────────────

/// The TS2835 arm: an extensionless ESM import from `/x/index.mts` of
/// "./index" — the unit file ITSELF is the FileExists hit, so Go suggests
/// ".mjs" ("Did you mean './index.mjs'?" — exactly the nodeModules1
/// baseline's index.mts(15,22) TS2835).
#[test]
fn mts_extensionless_self_import_suggests_mjs_pin() {
    let (mut c, file) = check_named(
        "/x/index.mts",
        "import { x } from \"./index\";\nexport { x };\n",
    );
    let pairs = diag_pairs(&mut c, file);
    assert!(
        pairs.contains(&(2835, "./index.mjs".to_string())),
        "TS2835 Did-you-mean must fire with './index.mjs': {pairs:?}"
    );
}

/// The TS2834 arm: an extensionless ESM import with no FileExists hit
/// ("./subfolder" — the 1-file FS twin) → "Consider adding an extension"
/// (nodeModules1 baseline's index.mts(16,22) TS2834).
#[test]
fn mts_extensionless_import_no_suggestion_2834_pin() {
    let (mut c, file) = check_named(
        "/x/index.mts",
        "import { x } from \"./subfolder\";\nexport { x };\n",
    );
    let pairs = diag_pairs(&mut c, file);
    assert!(
        pairs.contains(&(2834, String::new())),
        "TS2834 Consider-adding-an-extension must fire: {pairs:?}"
    );
}

/// The TS2732 json arm: a `.json` reference with resolveJsonModule off →
/// "Consider using '--resolveJsonModule'" instead of the plain 2307.
#[test]
fn json_import_without_resolve_json_module_2732_pin() {
    let (mut c, file) = check_named(
        "/x/index.mts",
        "import data from \"./data.json\";\nexport { data };\n",
    );
    let pairs = diag_pairs(&mut c, file);
    assert!(
        pairs.contains(&(2732, "./data.json".to_string())),
        "TS2732 resolveJsonModule arm must fire: {pairs:?}"
    );
    assert!(
        !pairs.iter().any(|(code, _)| *code == 2307),
        "the plain TS2307 must NOT fire for the json reference: {pairs:?}"
    );
}

/// The plain TS2307 fallback: a non-relative (bare) unresolvable module
/// reference — no ladder arm applies.
#[test]
fn bare_unresolved_module_plain_2307_pin() {
    let (mut c, file) = check_named(
        "/x/index.mts",
        "import { x } from \"some-unresolved-module\";\nexport { x };\n",
    );
    let pairs = diag_pairs(&mut c, file);
    assert!(
        pairs.contains(&(2307, "some-unresolved-module".to_string())),
        "plain TS2307 must fire for the bare module: {pairs:?}"
    );
}

/// The mode derivation's keystone: a `.cts` file's DYNAMIC `import()` is
/// ESM-mode under node16 (ShouldTransformImportCall false) → TS2835 for
/// the extensionless "./index" (the twin's 1-file FS holds only
/// index.cts, so the FileExists ladder suggests ".cjs" — the go twin
/// dump's exact `2835|./index.cjs` on /.src/index.cts; the full-corpus
/// baseline's ".mjs" needs the sibling index.mts the twin never loads),
/// while its STATIC import is CommonJS-mode → the plain 2307.
#[test]
fn cts_dynamic_import_esm_mode_static_import_cjs_mode_pin() {
    let (mut c, file) = check_named(
        "/x/index.cts",
        "import { x } from \"./index\";\nexport { x };\nconst p = import(\"./index\");\nexport { p };\n",
    );
    let pairs = diag_pairs(&mut c, file);
    assert!(
        pairs.contains(&(2307, "./index".to_string())),
        "the static extensionless import in a .cts file is CommonJS-mode — plain TS2307: {pairs:?}"
    );
    assert!(
        pairs.contains(&(2835, "./index.cjs".to_string())),
        "the dynamic import() in a .cts file is ESM-mode — TS2835 with the .cts-unit suggestion: {pairs:?}"
    );
}

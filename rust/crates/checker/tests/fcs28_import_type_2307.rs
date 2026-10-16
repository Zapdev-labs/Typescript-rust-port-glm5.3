//! FCS28 pins (TS2307 module-not-found residual family, tranche 28): the
//! import-type half — the residual pool's dominant root.
//!
//! ROOT (both "FCS tranche 28" PORT notes in-tree):
//!
//! 1. THE 316: `getTypeFromImportTypeNode` (typenodes.rs, Go checker.go:
//!    25109-25116) resolves through the INTERNAL resolver —
//!    `resolveExternalModuleName(node, literal, false /*ignoreErrors*/,
//!    getTypeFromImportAttributes(ast.GetImportAttributes(node)))`. The
//!    port called the PUBLIC `ResolveExternalModuleName` carrier
//!    (exports.rs), which hard-codes ignoreErrors=true — every
//!    import-type node's module-not-found was silently swallowed (the
//!    340-instance rust-missing TS2307 family: `import("./mod").A`
//!    annotations, JSDoc `@param {import("./mod").x}` tags,
//!    resolution-mode import types, `typeof import(...)`). Now Go-exact:
//!    errors ON, the attributes type passed, location = the ImportTypeNode
//!    (the context-specifier derivation's `IsLiteralImportTypeNode` arm).
//!
//! 2. The 1453 half: `checkImportType` (statements.rs, Go checker.go:
//!    3375-3383) checks the attribute VALUES (checkGrammarImportAttributeValues
//!    → TS2858) and calls the REPORTING getResolutionModeOverride — Go's
//!    ast/ast.go:2237 grammarErrorOnNode callback fires TS1453 on a
//!    resolution-mode value that is neither "import" nor "require". Both
//!    were permissive stubs; resolution.rs's free helper dropped Go's
//!    callback exactly like the compiler crate's port (its checker.go
//!    callers pass nil), so the callback-bearing `_ex` form + the
//!    `get_resolution_mode_override_checked` wrapper landed this tranche.
//!
//! Expectations verified byte-same against the go-bench twin's check-stage
//! dumps (rig, t28d streams — moduleExportAssignment7, importTypeLocal and
//! the node/TypeMode records all byte-identical post-fix) — pins run
//! WITHOUT libs (the corpus twin seam; the unit-file program resolves
//! nothing, so every reference walks the module-not-found ladder).

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::compileroptions::ResolutionModeNone;
use tsc_core::options_generated::CompilerOptions;
use tsc_core::scriptkind::ScriptKind;
use tsc_parser::parse_source_file;
use tsc_tspath::RootedFilePath;

// ── The single-file Program scaffold (the fcs23+ idiom) ──

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
        tsc_ast::SourceFileMetaData {
            package_json_type: String::new(),
            package_json_directory: Default::default(),
            implied_node_format: ResolutionModeNone,
        }
    }
    fn file_exists(&self, path: &str) -> bool {
        path == self.name.as_string()
    }
}

fn check_with(src: &str) -> (Checker, tsc_ast::NodeId) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let program = TestProgram {
        options: CompilerOptions::default(),
        name: RootedFilePath::from("/x/unit.ts"),
        source_file: sf,
        bind,
        binder_symbol_count,
        checker_symbols: Vec::new(),
    };
    let mut checker = new_checker(Box::new(program));
    checker.check_source_file(file, true);
    (checker, file)
}

/// The (code, args) list of the file's diagnostics, in order.
fn diags(c: &mut Checker, file: tsc_ast::NodeId) -> Vec<(i32, Vec<String>)> {
    c.get_file_diagnostics(file)
        .iter()
        .map(|d| (d.code(), d.message_args().to_vec()))
        .collect()
}

fn codes(ds: &[(i32, Vec<String>)], code: i32) -> Vec<Vec<String>> {
    ds.iter()
        .filter(|(c, _)| *c == code)
        .map(|(_, args)| args.clone())
        .collect()
}

// ── Pin 1: the import-type module-not-found — THE family ──
//
// An unresolvable relative specifier in an import-type node reports
// TS2307 at the string literal (Go checker.go:25116's ignoreErrors=false
// resolution through the module-not-found ladder). Before tranche 28 the
// public-carrier call swallowed this; the qualifier tail (TS2694-style
// namespace resolution) never ran either.

#[test]
fn fcs28_import_type_module_not_found_2307_pin() {
    let (mut c, file) = check_with("type T = import(\"./missing\").A;\n");
    let ds = diags(&mut c, file);
    assert_eq!(codes(&ds, 2307), vec![vec!["./missing".to_string()]]);
    assert_eq!(codes(&ds, 1141).len(), 0, "no string-literal-expected: {ds:?}");
}

// ── Pin 2: the resolution-mode grammar gate — the 1453 half ──
//
// A resolution-mode attribute value that is neither "import" nor "require"
// fires TS1453 (Go checker.go:3375-3383's reporting getResolutionModeOverride
// → ast.go:2237's grammarErrorOnNode) IN ADDITION to the TS2307 of the
// unresolvable bare specifier (checkImportType resolves with errors ON).

#[test]
fn fcs28_import_type_bad_resolution_mode_1453_2307_pin() {
    let (mut c, file) =
        check_with("type T = import(\"pkg\", { with: { \"resolution-mode\": \"bad\" } }).A;\n");
    let ds = diags(&mut c, file);
    assert_eq!(codes(&ds, 1453).len(), 1, "the 1453: {ds:?}");
    assert_eq!(codes(&ds, 2307), vec![vec!["pkg".to_string()]]);
}

// ── Pin 3: valid resolution-mode values — no 1453 ──

#[test]
fn fcs28_import_type_valid_resolution_mode_no_1453_pin() {
    let (mut c, file) =
        check_with("type T = import(\"pkg\", { with: { \"resolution-mode\": \"import\" } }).A;\n");
    let ds = diags(&mut c, file);
    assert_eq!(codes(&ds, 1453).len(), 0, "no 1453 on a valid mode: {ds:?}");
    assert_eq!(codes(&ds, 2307), vec![vec!["pkg".to_string()]]);
}

// ── Pin 4: the attribute-VALUE grammar check (TS2858) ──
//
// A non-string-literal attribute value fires checkGrammarImportAttributeValues
// (Go checker.go:3379); a non-literal-like value also suppresses the 1453
// probe (ast.go:2230's IsStringLiteralLike gate) while the TS2307 still
// reports.

#[test]
fn fcs28_import_type_non_literal_attribute_value_2858_pin() {
    let (mut c, file) =
        check_with("declare const x: string;\ntype T = import(\"pkg\", { with: { \"resolution-mode\": x } }).A;\n");
    let ds = diags(&mut c, file);
    assert_eq!(codes(&ds, 2858).len(), 1, "the 2858: {ds:?}");
    assert_eq!(codes(&ds, 1453).len(), 0, "no 1453 on a non-literal value: {ds:?}");
    assert_eq!(codes(&ds, 2307), vec![vec!["pkg".to_string()]]);
}

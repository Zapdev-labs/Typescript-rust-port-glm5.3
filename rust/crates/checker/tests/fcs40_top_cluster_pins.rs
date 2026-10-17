//! FCS40 pins (full-corpus check sweep, tranche 40): the top-3 census
//! clusters of the fresh t39-final-dump census — TS2856 rust-missing
//! x57/30rec (the node import-attributes CommonJS family), TS2391
//! rust-extra x58/40rec ("Function implementation is missing...", on
//! .d.ts units), TS1155 rust-extra x52/36rec ("'const' declarations
//! must be initialized", on .d.ts units). ("FCS tranche 40" PORT
//! notes in-tree.)
//!
//! TWO roots, both Go-exact:
//!
//! ROOT A — TS2856: `check_import_attributes` (Go checker.go:5576-
//! 5581) fires the CommonJS grammar error when
//! getEmitSyntaxForModuleSpecifierExpression(GetExternalModuleName(
//! declaration)) answers CommonJS — the node16/nodenext .cts-unit
//! import-attributes family. The port had a permissive never-CommonJS
//! stub; the grammarchecks.rs helper (the source-phase arm's
//! get_emit_module_format_of_file stand-in for
//! GetEmitSyntaxForUsageLocation) now answers the arm.
//!
//! ROOT B — TS2391 + TS1155 (one root, both clusters): the check twin
//! (tsc-testrunner's semantic_diagnostics_of_unit) parsed every unit
//! with ANONYMOUS parse options, so the parser's
//! `opts.file_name.is_declaration_file()` (Go parser.go:433
//! parseSourceFileWorker) answered false and .d.ts units never got the
//! parse-time ambient context flag (NodeFlagsAmbient) — the ambient
//! gates of checker.go:3631 (2391) and grammarchecks.go:1564 (1155)
//! then fired on every body-less function / uninitialized const in a
//! declaration unit. The twin now passes the REAL unit name; the pins
//! below pin the parser+checker ambient mechanism from the checker
//! crate (parse with the rooted file name, exactly as the twin now
//! does and as Go's program loader always did).

use tsc_ast::{NodeStore, SourceFile, SourceFileParseOptions, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::options_generated::{CompilerOptions, ModuleKind, ScriptTarget};
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
    /// The per-file emit format (the CheckerUnitProgram override shape —
    /// ROOT A's pin needs the CommonJS answer a .cts unit carries under
    /// node16/nodenext).
    emit_format: ModuleKind,
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
            implied_node_format: tsc_core::compileroptions::ResolutionModeNone,
        }
    }
    fn file_exists(&self, path: &str) -> bool {
        path == self.name.as_string()
    }
    fn get_emit_module_format_of_file(&self, _file: tsc_ast::NodeId) -> ModuleKind {
        self.emit_format
    }
}

/// The check twin's unit parse (post-tranche-40 shape): the REAL rooted
/// unit name rides the parse options, so `opts.file_name.is_declaration_file()`
/// answers for .d.ts units exactly as Go's program-loader parse does.
fn check_unit(
    src: &str,
    name: &str,
    options: CompilerOptions,
    emit_format: ModuleKind,
) -> (Checker, tsc_ast::NodeId) {
    let file_name = RootedFilePath::from(name);
    // Every pin unit is a TS-family extension (.ts/.cts/.d.ts — the
    // twin's script-kind switch answers TS for all of them).
    let mut sf = parse_source_file(
        SourceFileParseOptions {
            file_name,
            ..Default::default()
        },
        src,
        ScriptKind::TS,
    );
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let program = TestProgram {
        options,
        name: RootedFilePath::from(name),
        source_file: sf,
        bind,
        binder_symbol_count,
        checker_symbols: Vec::new(),
        emit_format,
    };
    let mut checker = new_checker(Box::new(program));
    checker.check_source_file(file, true);
    (checker, file)
}

/// The (code, args) list of the checker's WHOLE semantic collection.
fn diags(c: &mut Checker) -> Vec<(i32, Vec<String>)> {
    c.semantic_diagnostics()
        .iter()
        .map(|d| (d.code(), d.message_args().to_vec()))
        .collect()
}

fn count(ds: &[(i32, Vec<String>)], code: i32) -> usize {
    ds.iter().filter(|(c, _)| *c == code).count()
}

// ── ROOT A — TS2856 (the node import-attributes CommonJS family) ──

/// The nodeModulesImportAttributes.ts .cts-unit shape (ROOT A): under
/// nodenext (attributes supported — the Go list is esnext/node18/node20/
/// nodenext/preserve), a unit whose per-file emit format is
/// CommonJS reports TS2856 for `import ... with { ... }` — Go's
/// getEmitSyntaxForModuleSpecifierExpression answer.
#[test]
fn fcs40_import_attributes_cjs_unit_reports_2856_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2022,
        module: ModuleKind::NodeNext,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "import json from \"./package.json\" with { type: \"json\" };\n",
        "/x/unit.cts",
        options,
        ModuleKind::CommonJS,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 2856), 1, "expected one TS2856, got: {ds:?}");
}

/// The index.ts control (ROOT A): the ES-module-format unit under the
/// SAME node16 config does NOT report TS2856 (the emit format is the
/// single determinant — the pre-tranche never-CommonJS stub converged
/// here by accident).
#[test]
fn fcs40_import_attributes_es_format_unit_no_2856_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2022,
        module: ModuleKind::NodeNext,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "import json from \"./package.json\" with { type: \"json\" };\n",
        "/x/index.ts",
        options,
        ModuleKind::ES2015,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 2856), 0, "unexpected TS2856, got: {ds:?}");
}

// ── ROOT B — TS2391 / TS1155 (the .d.ts ambient parse root) ──

/// The umd1.ts foo.d.ts shape (ROOT B): a body-less function
/// declaration in a DECLARATION unit is ambient — no TS2391. The
/// pre-tranche twin's anonymous parse answered is_declaration_file()
/// false, so the checker.go:3631 gate fired the extra.
#[test]
fn fcs40_dts_function_without_body_no_2391_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::CommonJS,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "export var x: number;\nexport function fn(): void;\n",
        "/x/foo.d.ts",
        options,
        ModuleKind::CommonJS,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 2391), 0, "unexpected TS2391 on a .d.ts unit, got: {ds:?}");
}

/// The .ts control (ROOT B): the SAME body-less declaration in a
/// non-declaration unit DOES report TS2391 — the parse-time ambient
/// flag is the only difference.
#[test]
fn fcs40_ts_function_without_body_reports_2391_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::CommonJS,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "export function fn(): void;\n",
        "/x/a.ts",
        options,
        ModuleKind::CommonJS,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 2391), 1, "expected one TS2391, got: {ds:?}");
}

/// The importSource20.ts node_modules/pkg/index.d.ts shape (ROOT B): an
/// uninitialized const in a DECLARATION unit is ambient — no TS1155.
#[test]
fn fcs40_dts_const_without_initializer_no_1155_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2022,
        module: ModuleKind::Node16,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "export const x: number;\n",
        "/x/node_modules/pkg/index.d.ts",
        options,
        ModuleKind::ES2015,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 1155), 0, "unexpected TS1155 on a .d.ts unit, got: {ds:?}");
}

/// The .ts control (ROOT B): the SAME uninitialized const in a
/// non-declaration unit DOES report TS1155 (the grammarchecks.go:1564
/// gate).
#[test]
fn fcs40_ts_const_without_initializer_reports_1155_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2022,
        module: ModuleKind::Node16,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "export const x: number;\n",
        "/x/index.ts",
        options,
        ModuleKind::ES2015,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 1155), 1, "expected one TS1155, got: {ds:?}");
}

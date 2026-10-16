//! FCS32 pins (full-corpus check sweep, tranche 32): the rust-missing
//! TS2354 "This syntax requires an imported helper but module '{0}'
//! cannot be found" family — 104 instances across 72 divergent records
//! before the tranche (the importHelpers/tslib-unresolvable error; the
//! per-unit twin FS never contains tslib, so every helper-requiring
//! syntax reports it once per source file, memoized at the first
//! checkExternalEmitHelpers call).
//!
//! ROOT fixed this tranche ("FCS tranche 32" PORT notes in-tree):
//!
//! `check_external_emit_helpers` (statements.rs — Go checker.go:29105)
//! was the permissive no-op stub; the REAL body now runs Go's gates
//! (importHelpers + IsEffectiveExternalModule + non-ambient) and the
//! memoized `resolveHelpersModule` "tslib" resolution
//! (resolveExternalModule with the 2354 message, checker.go:29195) plus
//! the per-helper-name TS2343/TS2807 ladder. The missing call sites were
//! added Go-exactly: checkDecorators' firstDecorator block
//! (checker.go:6234-6250), checkClassExpressionExternalHelpers
//! (10289-10331, with getFirstTransformableStaticClassElement +
//! IsNamedEvaluationSource), checkYieldExpression's
//! AsyncDelegatorIncludes (11212), the private-field Get/Set/In sites
//! (11509/11512/13329).
//!
//! All pins run WITHOUT libs (the corpus twin seam — the per-unit
//! checker), matching the go-bench driver's construction; the pin
//! Program's importHelpers seam mirrors the twin file loader's synthetic
//! "tslib" import (the loader gate and the checker's
//! IsEffectiveExternalModule gate coincide).

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::options_generated::{CompilerOptions, ModuleKind, ScriptTarget};
use tsc_core::scriptkind::ScriptKind;
use tsc_core::tristate::Tristate;
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
            implied_node_format: tsc_core::compileroptions::ResolutionModeNone,
        }
    }
    fn file_exists(&self, path: &str) -> bool {
        path == self.name.as_string()
    }
    // The twin's emit-module-format answer (Go GetEmitModuleFormatOfFile —
    // the CommonJS import-site gate reads it).
    fn get_emit_module_format_of_file(&self, _file: tsc_ast::NodeId) -> ModuleKind {
        self.options.module
    }
    // PORT (FCS tranche 32): the twin file loader's importHelpers answer —
    // the synthetic "tslib" import exists for every importHelpers unit
    // (the loader's external-module gate coincides with the checker's).
    fn get_import_helpers_import_specifier(&self, _file: tsc_ast::NodeId) -> Option<String> {
        if self.options.import_helpers == Tristate::TSTrue {
            Some("tslib".to_string())
        } else {
            None
        }
    }
}

fn check_with(src: &str, options: CompilerOptions) -> (Checker, tsc_ast::NodeId) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let program = TestProgram {
        options,
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

/// importHelpers options at the given target / module kind.
fn options(target: ScriptTarget, module: ModuleKind) -> CompilerOptions {
    CompilerOptions {
        target,
        module,
        import_helpers: Tristate::TSTrue,
        ..CompilerOptions::default()
    }
}

/// The (code, args) list of the checker's WHOLE semantic collection (the
/// corpus seam reads `semantic_diagnostics()`).
fn diags(c: &mut Checker, file: tsc_ast::NodeId) -> Vec<(i32, Vec<String>)> {
    let _ = file;
    c.semantic_diagnostics()
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

// ── Pin 1: the esDecorators keystone — the classExpression.1 record's
// shape (export const C = @dec class {}; target es2022): the
// checkDecorators ESDecorateAndRunInitializers request at firstDecorator
// fires the memoized 2354 exactly once.

#[test]
fn fcs32_decorator_class_expression_2354_pin() {
    let src = "declare var dec: any;\nexport const C = @dec class {};\n";
    let (mut c, file) = check_with(src, options(ScriptTarget::ES2022, ModuleKind::ES2015));
    let ds = diags(&mut c, file);
    assert_eq!(
        codes(&ds, 2354),
        vec![vec!["tslib".to_string()]],
        "exactly one memoized 2354 (tslib) for the decorated class expression"
    );
}

// ── Pin 2: the legacy-decorator arm (Decorate at firstDecorator). ──

#[test]
fn fcs32_legacy_decorator_2354_pin() {
    let src = "declare var dec: any;\n@dec\nexport class C {}\n";
    let mut o = options(ScriptTarget::ES5, ModuleKind::CommonJS);
    o.experimental_decorators = Tristate::TSTrue;
    let (mut c, file) = check_with(src, o);
    let ds = diags(&mut c, file);
    assert_eq!(
        codes(&ds, 2354),
        vec![vec!["tslib".to_string()]],
        "legacy __decorate reports the memoized 2354"
    );
}

// ── Pin 3: the import-star site (the nodeModulesImportHelpersCollisions
// shape — import * as fs from "fs" under CommonJS emit). ──

#[test]
fn fcs32_import_star_2354_pin() {
    let src = "import * as fs from \"fs\";\nexport {};\n";
    let (mut c, file) = check_with(src, options(ScriptTarget::ES2022, ModuleKind::CommonJS));
    let ds = diags(&mut c, file);
    assert_eq!(codes(&ds, 2354), vec![vec!["tslib".to_string()]]);
}

// ── Pin 4: the import-default site (import d from "fs" under CJS). ──

#[test]
fn fcs32_import_default_2354_pin() {
    let src = "import d from \"fs\";\nexport {};\n";
    let (mut c, file) = check_with(src, options(ScriptTarget::ES2022, ModuleKind::CommonJS));
    let ds = diags(&mut c, file);
    assert_eq!(codes(&ds, 2354), vec![vec!["tslib".to_string()]]);
}

// ── Pin 5: the using-declaration site (usingDeclarationsWithImportHelpers
// — the AddDisposableResource/DisposeResources request). ──

#[test]
fn fcs32_using_declaration_2354_pin() {
    let src = "export {};\nusing y = 1;\n";
    let (mut c, file) = check_with(src, options(ScriptTarget::ES2015, ModuleKind::ES2015));
    let ds = diags(&mut c, file);
    assert_eq!(codes(&ds, 2354), vec![vec!["tslib".to_string()]]);
}

// ── Pin 6: the private-field Set/Get sites (privateNameEmitHelpers —
// `this.#c = 42` requests Set then Get; the memoized resolution reports
// the 2354 exactly ONCE for the first request). ──

#[test]
fn fcs32_private_field_access_2354_pin() {
    let src = "export class C {\n    #a = 1;\n    m() { this.#a = 42; }\n}\n";
    let (mut c, file) = check_with(src, options(ScriptTarget::ES2015, ModuleKind::ES2015));
    let ds = diags(&mut c, file);
    assert_eq!(
        codes(&ds, 2354),
        vec![vec!["tslib".to_string()]],
        "the Set-then-Get pair reports one memoized 2354"
    );
}

// ── Pin 7: the for-await site (the ForAwaitOfIncludes request — the
// pre-existing stub call site now REAL). ──

#[test]
fn fcs32_for_await_2354_pin() {
    let src = "export async function f() {\n    for await (const x of []) {}\n}\n";
    let (mut c, file) = check_with(src, options(ScriptTarget::ES2015, ModuleKind::ES2015));
    let ds = diags(&mut c, file);
    assert_eq!(codes(&ds, 2354), vec![vec!["tslib".to_string()]]);
}

// ── Pin 8: the anonymous class expression in a named-evaluation position
// with a transformable static private member (checkClassExpressionExternal
// Helpers → getFirstTransformableStaticClassElement → SetFunctionName). ──

#[test]
fn fcs32_class_expression_set_function_name_2354_pin() {
    let src = "export const C = class { static #a = 1; };\n";
    let (mut c, file) = check_with(src, options(ScriptTarget::ES2015, ModuleKind::ES2015));
    let ds = diags(&mut c, file);
    assert_eq!(codes(&ds, 2354), vec![vec!["tslib".to_string()]]);
}

// ── Pin 9 (negative): importHelpers off — the checker gate returns
// before any resolution (Go checker.go:29106). ──

#[test]
fn fcs32_no_import_helpers_negative_pin() {
    let src = "declare var dec: any;\nexport const C = @dec class {};\n";
    let (mut c, file) = check_with(
        src,
        CompilerOptions {
            target: ScriptTarget::ES2022,
            module: ModuleKind::ES2015,
            ..CompilerOptions::default()
        },
    );
    let ds = diags(&mut c, file);
    assert_eq!(codes(&ds, 2354).len(), 0, "no importHelpers — no 2354");
}

// ── Pin 10 (negative): a non-module file — IsEffectiveExternalModule
// fails (Go checker.go:29111), so no resolution runs even with
// importHelpers on. ──

#[test]
fn fcs32_non_module_negative_pin() {
    let src = "declare var dec: any;\nconst C = 1;\n";
    let (mut c, file) = check_with(src, options(ScriptTarget::ES2022, ModuleKind::CommonJS));
    let ds = diags(&mut c, file);
    assert_eq!(codes(&ds, 2354).len(), 0, "non-module — no 2354");
}

// ── Pin 11 (negative): an ambient (declaration) location — Go's
// `location.Flags&NodeFlagsAmbient != 0` gate (checker.go:29112). ──

#[test]
fn fcs32_ambient_negative_pin() {
    let src = "declare module \"m\" {\n    export const C: number;\n}\n";
    let (mut c, file) = check_with(src, options(ScriptTarget::ES2022, ModuleKind::CommonJS));
    let ds = diags(&mut c, file);
    assert_eq!(codes(&ds, 2354).len(), 0, "ambient context — no 2354");
}

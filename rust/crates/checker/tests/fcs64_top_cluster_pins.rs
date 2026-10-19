//! FCS64 pins (full-corpus check sweep, micro-tranche 63 — the never-
//! returning-call flow family, worked directly by the orchestrator): the
//! tranche's root —
//!
//!  * `get_type_at_flow_call`'s never-return arm (flow.rs) was nested
//!    INSIDE the `if let Some(predicate)` block, so Go's arm
//!    (flow.go:308-310 — a SIBLING of the asserts-predicate arm, inside
//!    the signature check) never fired for predicate-less never calls:
//!    `function fail(): never` made `if (x === undefined) fail(...)`'s
//!    consequent still walk its antecedent (the true-narrowed
//!    `undefined`) instead of marking the flow unreachable, so `x` at
//!    the join stayed `string | undefined` and TS18048 "x is possibly
//!    undefined" fired rust-extra (neverReturningFunctions1 49 vs go 45
//!    — CONVERGED). Un-nested the arm to Go's structure.
//!
//! ("FCS tranche 63" PORT notes in-tree; pins named fcs64 by census epoch.)

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::compileroptions::ResolutionModeNone;
use tsc_core::options_generated::{CompilerOptions, ModuleKind, ScriptTarget};
use tsc_core::scriptkind::ScriptKind;
use tsc_core::tristate::Tristate;
use tsc_parser::parse_source_file;
use tsc_tspath::RootedFilePath;

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
    fn get_emit_module_format_of_file(&self, _file: tsc_ast::NodeId) -> ModuleKind {
        ModuleKind::CommonJS
    }
    fn get_implied_node_format_for_emit(&self, _file: tsc_ast::NodeId) -> ModuleKind {
        ModuleKind::CommonJS
    }
    fn source_file_implied_node_format(&self, _file: tsc_ast::NodeId) -> ModuleKind {
        ModuleKind::CommonJS
    }
}

fn check_with(src: &str, options: CompilerOptions) -> (Checker, tsc_ast::NodeId) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let program = TestProgram {
        options,
        name: RootedFilePath::from("/x/fcs64_unit.ts".to_string()),
        source_file: sf,
        bind,
        binder_symbol_count,
        checker_symbols: Vec::new(),
    };
    let mut checker = new_checker(Box::new(program));
    checker.check_source_file(file, true);
    (checker, file)
}

fn check_strict(src: &str) -> (Checker, tsc_ast::NodeId) {
    check_with(
        src,
        CompilerOptions {
            target: ScriptTarget::ESNext,
            strict: Tristate::TSTrue,
            ..CompilerOptions::default()
        },
    )
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

/// The neverReturningFunctions1 f01 shape: the consequent
/// `if (x === undefined) fail("undefined argument")` ends in a
/// predicate-less never-returning call, so the branch join drops the
/// consequent path entirely and `x` at the join narrows to `string` —
/// NO TS18048 "x is possibly undefined".
#[test]
fn fcs64_never_call_narrows_consequent_unreachable_pin() {
    let (mut c, file) = check_strict(
        "declare function fail(message?: string): never;\nfunction f01(x: string | undefined) {\n    if (x === undefined) fail(\"undefined argument\");\n    x.length;\n}\n",
    );
    let ds = diags(&mut c, file);
    assert!(
        codes(&ds, 18048).is_empty(),
        "the never-returning consequent is unreachable, so x narrows to string: {ds:?}"
    );
}

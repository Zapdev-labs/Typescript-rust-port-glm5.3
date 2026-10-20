//! FCS65 pins (full-corpus check sweep, micro-tranche 64 — the
//! conditional-relation true-type target family, worked directly by the
//! orchestrator): the tranche's root —
//!
//!  * `structured_type_related_to_worker`'s assignable-relation
//!    conditional-source arm (relater.rs) transcribed Go relater.go:3785
//!    as `is_related_to(instantiate(trueType(source), mapper), target)`,
//!    passing the whole conditional `target` instead of
//!    `getTrueTypeFromConditionalType(target)`. Structurally identical
//!    deferred conditionals (`type T90<T> = T extends 0 ? 0 : () => 0;`
//!    vs `type T91<T> = ...`) therefore related the instantiated true
//!    type `0` against the conditional itself, failed, and fell through
//!    to the default/distributive-constraint paths — TS2322 rust-extra
//!    at every `const f40 = <U>(a: T90<U>): T91<U> => a` shape
//!    (conditionalTypes1 70 vs go 64 — CONVERGED to exactly 64; also
//!    conditionalTypes2 44 -> 42 and unionAndIntersectionInference3
//!    25 -> 24).
//!
//! ("FCS tranche 64" PORT notes in-tree; pins named fcs65 by census epoch.)

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
        name: RootedFilePath::from("/x/fcs65_unit.ts".to_string()),
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

/// The conditionalTypes1 #21823 shape: two structurally identical
/// deferred conditional types are mutually assignable (the true-type
/// relation compares the target's TRUE TYPE against the instantiated
/// source true type) — NO TS2322 on either arrow.
#[test]
fn fcs65_identical_deferred_conditionals_assignable_pin() {
    let (mut c, file) = check_strict(
        "type T90<T> = T extends 0 ? 0 : () => 0;\ntype T91<T> = T extends 0 ? 0 : () => 0;\nconst f40 = <U>(a: T90<U>): T91<U> => a;\nconst f41 = <U>(a: T91<U>): T90<U> => a;\n",
    );
    let ds = diags(&mut c, file);
    assert!(
        codes(&ds, 2322).is_empty(),
        "structurally identical deferred conditionals are mutually assignable: {ds:?}"
    );
}

/// The #21756 shape: two structurally identical deferred conditionals
/// with literal-widened true/false branches (Foo/Bar) are mutually
/// assignable — NO TS2322 on `convert`.
#[test]
fn fcs65_identical_foo_bar_conditionals_assignable_pin() {
    let (mut c, file) = check_strict(
        "type Foo<T> = T extends string ? boolean : number;\ntype Bar<T> = T extends string ? boolean : number;\nconst convert = <U>(value: Foo<U>): Bar<U> => value;\n",
    );
    let ds = diags(&mut c, file);
    assert!(
        codes(&ds, 2322).is_empty(),
        "Foo<U> is assignable to Bar<U>: {ds:?}"
    );
}

/// The #21823 f45 shape still errors in Go too: `T95<U>` (boolean |
/// number branches) is NOT assignable to `T94<U>` (true | 42) because the
/// true-type relation compares `boolean | number` against `true` —
/// TS2322 EXPECTED (guards against an over-permissive fix).
#[test]
fn fcs45_widened_conditional_still_errors_pin() {
    let (mut c, file) = check_strict(
        "type T94<T> = T extends string ? true : 42;\ntype T95<T> = T extends string ? boolean : number;\nconst f45 = <U>(value: T95<U>): T94<U> => value;\n",
    );
    let ds = diags(&mut c, file);
    assert_eq!(
        codes(&ds, 2322).len(),
        1,
        "T95<U> -> T94<U> still errors exactly once (widened branches): {ds:?}"
    );
}

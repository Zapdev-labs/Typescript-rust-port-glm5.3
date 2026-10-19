//! FCS62 pins (full-corpus check sweep, micro-tranche 61 — the keyof/
//! unknown family, worked directly by the orchestrator): the tranche's
//! root —
//!
//!  * `is_unknown_like_union_type` (relater.rs) was a permissive-false
//!    free-helper stub, so isSimpleTypeRelatedTo's tail arm ("Anything is
//!    assignable to a union containing undefined, null, and {}" — Go
//!    relater.go:272) never fired: `T` / `T[keyof T]` / `T[K]` (and bare
//!    `unknown`, via the type-variable constraint arm's `constraint ==
//!    nil -> unknownType` fallback) all failed the assignability to
//!    `{} | null | undefined` — the canonical `unknown` decomposition —
//!    and TS2322/TS2345 fired rust-extra across keyofAndIndexedAccess
//!    (f92/fn/Unbounded: 5 extras), unknownControlFlow and unknownType1.
//!    Implemented Go's checker.go:28337 body (strictNullChecks + union;
//!    cached on the union's objectFlags via IsUnknownLikeUnion{,
//!    Computed}; types[0] undefined, types[1] null, and some constituent
//!    the empty anonymous object type).
//!
//! ("FCS tranche 61" PORT notes in-tree; pins named fcs62 by census epoch.)

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
        name: RootedFilePath::from("/x/fcs62_unit.ts".to_string()),
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

// ── CLUSTER A — the unknown-decomposition union family ──

/// The keyofAndIndexedAccess f92 shape: `T`, `T[keyof T]`, and `T[K]`
/// all assign to `{} | null | undefined` (the type-variable arm falls
/// back to the constraint `unknown`, and the union is the unknown
/// decomposition): NO TS2322.
#[test]
fn fcs62_type_variable_assigns_to_unknown_like_union_pin() {
    let (mut c, file) = check_strict(
        "function f92<T, K extends keyof T>(x: T, y: T[keyof T], z: T[K]) {\n    let a: {} | null | undefined;\n    a = x;\n    a = y;\n    a = z;\n}\n",
    );
    let ds = diags(&mut c, file);
    assert!(
        codes(&ds, 2322).is_empty(),
        "type variables assign to the unknown-decomposition union: {ds:?}"
    );
}

/// The keyofAndIndexedAccess fn/take shape (#21368): the call argument
/// `o[k]` (type `T[K]`) checks against the explicit type argument
/// `{} | null | undefined`: NO TS2345.
#[test]
fn fcs62_take_unknown_like_union_argument_pin() {
    let (mut c, file) = check_strict(
        "interface I { foo: string; }\ndeclare function take<T>(p: T): void;\nfunction fn<T extends I, K extends keyof T>(o: T, k: K) {\n    take<{} | null | undefined>(o[k]);\n}\n",
    );
    let ds = diags(&mut c, file);
    assert!(
        codes(&ds, 2345).is_empty(),
        "the T[K] argument checks against the unknown-like union type argument: {ds:?}"
    );
}

/// The keyofAndIndexedAccess Unbounded shape (#23133): the parameter
/// `x: T[keyof T]` assigns to the local `{} | undefined | null`: NO
/// TS2322.
#[test]
fn fcs62_unbounded_keyof_value_assigns_pin() {
    let (mut c, file) = check_strict(
        "class Unbounded<T> {\n    foo(x: T[keyof T]) {\n        let y: {} | undefined | null = x;\n    }\n}\n",
    );
    let ds = diags(&mut c, file);
    assert!(
        codes(&ds, 2322).is_empty(),
        "T[keyof T] assigns to the unknown-decomposition union: {ds:?}"
    );
}

/// The negative controls (Go errors on these too): the union must
/// contain ALL THREE of undefined, null, and the empty object type —
/// `{} | null` and `{} | undefined` are NOT unknown-like, and neither
/// `unknown` itself nor a bare `T` assigns to the plain `{}`.
#[test]
fn fcs62_unknown_like_union_negative_controls_pin() {
    let (mut c, file) = check_strict(
        "declare let u: unknown;\nlet a3: {} | null = u;\nlet a4: {} | undefined = u;\nfunction g2<T>(x: T) { let b: {}; b = x; }\n",
    );
    let ds = diags(&mut c, file);
    assert_eq!(
        codes(&ds, 2322).len(),
        3,
        "unknown and T do NOT assign to the partial unions or the bare {{}}: {ds:?}"
    );
}

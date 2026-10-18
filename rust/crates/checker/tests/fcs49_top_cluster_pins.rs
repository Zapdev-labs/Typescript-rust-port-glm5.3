//! FCS49 pins (full-corpus check sweep, micro-tranche 49): the top coherent
//! clusters of the fresh t49 census —
//!
//!  * the invocation-error CHAIN-HEAD family (TS2349/TS2351 heads vs the
//!    TS2757/TS2761 chained inner message: invocationErrorDetails now
//!    builds Go's full chain — head "This expression is not
//!    callable/constructable" (or the get-accessor head) with the
//!    signature-mismatch diagnostic (and the union-arm variants
//!    2755/2756/2758, 2759/2760/2762) as its message chain; the caller
//!    arm of resolveCallExpressionWithPotentialDependency passes the
//!    callee expression, not the call node, so the span matches Go's);
//!  * the dynamic-import TS7036 double-fire (checkImportCallExpression's
//!    specifier check now uses the SILENT isTypeAssignableTo — Go
//!    checker.go:8487 — with the single explicit error below carrying the
//!    message; the former port fed the error node + message into
//!    check_type_assignable_to, firing the message through the
//!    elaboration reporter AND the explicit error);
//!  * the parser-recovered async-generator member gate
//!    (checkFunctionOrMethodDeclaration's generator arm gates on
//!    ast.NodeIsPresent(body) — Go checker.go:3483-3486 — so the
//!    synthesized zero-length body of a recovered member no longer
//!    forces the return-type check that resolves
//!    AsyncIterableIteratorChecked and fires the location-less TS2318).
//!
//! ("FCS tranche 49" PORT notes in-tree.)

use tsc_ast::{NodeStore, SourceFile, SourceFileParseOptions, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::options_generated::{CompilerOptions, ModuleKind, ScriptTarget};
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
        ModuleKind::CommonJS
    }
    fn get_implied_node_format_for_emit(&self, _file: tsc_ast::NodeId) -> ModuleKind {
        ModuleKind::CommonJS
    }
    fn source_file_implied_node_format(&self, _file: tsc_ast::NodeId) -> ModuleKind {
        ModuleKind::CommonJS
    }
}

fn check_unit(src: &str, name: &str, options: CompilerOptions) -> (Checker, tsc_ast::NodeId) {
    let file_name = RootedFilePath::from(name);
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
    };
    let mut checker = new_checker(Box::new(program));
    checker.check_source_file(file, true);
    (checker, file)
}

fn diags(c: &mut Checker) -> Vec<tsc_ast::Diagnostic> {
    c.semantic_diagnostics()
}

fn options_es2015() -> CompilerOptions {
    CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::CommonJS,
        ..CompilerOptions::default()
    }
}

fn options_es2018() -> CompilerOptions {
    CompilerOptions {
        target: ScriptTarget::ES2018,
        module: ModuleKind::CommonJS,
        ..CompilerOptions::default()
    }
}

// ── ROOT A — the invocation-error chain-head family ──

/// `x()` where x: never fires the HEAD TS2349 (empty args) with the
/// chained inner TS2757 ("Type 'never' has no call signatures") as its
/// message chain — never a bare top-level TS2757 (Go:
/// invocationErrorDetails, checker.go:10165-10226; the corpus record is
/// neverTypeErrors1.ts — DUMP 2349 at the `x()` span in both twins).
#[test]
fn fcs49_invocation_error_call_head_chain_pin() {
    let (mut c, _) = check_unit(
        "function f1() {\n    let x: never;\n    x();\n}\n",
        "/x/fcs49_call_head.ts",
        options_es2015(),
    );
    let ds = diags(&mut c);
    let heads: Vec<&tsc_ast::Diagnostic> = ds.iter().filter(|d| d.code() == 2349).collect();
    assert_eq!(
        heads.len(),
        1,
        "expected exactly 1 TS2349 head, got: {:?}",
        ds.iter().map(|d| d.code()).collect::<Vec<_>>()
    );
    assert!(
        heads[0].message_args().is_empty(),
        "the head carries no args, got: {:?}",
        heads[0].message_args()
    );
    let chain = heads[0].message_chain();
    assert_eq!(chain.len(), 1, "expected one chain member");
    assert_eq!(chain[0].code(), 2757, "the inner chain member is 2757");
    assert_eq!(
        chain[0].message_args(),
        &["never".to_string()],
        "inner args are [apparent type], got: {:?}",
        chain[0].message_args()
    );
    assert!(
        ds.iter().all(|d| d.code() != 2757),
        "no bare top-level TS2757 may fire, got: {:?}",
        ds.iter().map(|d| d.code()).collect::<Vec<_>>()
    );
}

/// `new `abc`` fires the HEAD TS2351 with the chained inner TS2761
/// ("Type '<the template type>' has no construct signatures") — the
/// resolveNewExpression arm (Go checker.go:8858; the corpus record is
/// templateStringInNewExpression.ts — DUMP 2351 at the template span in
/// both twins).
#[test]
fn fcs49_invocation_error_new_head_chain_pin() {
    let (mut c, _) = check_unit(
        "new `abc`;\n",
        "/x/fcs49_new_head.ts",
        options_es2015(),
    );
    let ds = diags(&mut c);
    let heads: Vec<&tsc_ast::Diagnostic> = ds.iter().filter(|d| d.code() == 2351).collect();
    assert_eq!(
        heads.len(),
        1,
        "expected exactly 1 TS2351 head, got: {:?}",
        ds.iter().map(|d| d.code()).collect::<Vec<_>>()
    );
    let chain = heads[0].message_chain();
    assert_eq!(chain.len(), 1, "expected one chain member");
    assert_eq!(chain[0].code(), 2761, "the inner chain member is 2761");
    assert_eq!(
        chain[0].message_args().len(),
        1,
        "the inner member carries the apparent type arg, got: {:?}",
        chain[0].message_args()
    );
    assert!(ds.iter().all(|d| d.code() != 2761));
}

/// A union apparent type with NO callable constituent fires the head
/// TS2349 chained with the union arm's TS2755 ("No constituent of type
/// 'string | number' is callable") — the former port had no union arm at
/// all (Go checker.go:10177-10197).
#[test]
fn fcs49_invocation_error_union_no_constituent_pin() {
    let (mut c, _) = check_unit(
        "declare const u: string | number;\nu();\n",
        "/x/fcs49_union_head.ts",
        options_es2015(),
    );
    let ds = diags(&mut c);
    let heads: Vec<&tsc_ast::Diagnostic> = ds.iter().filter(|d| d.code() == 2349).collect();
    assert_eq!(heads.len(), 1, "expected exactly 1 TS2349 head");
    let chain = heads[0].message_chain();
    assert_eq!(chain.len(), 1, "expected one chain member");
    assert_eq!(chain[0].code(), 2755, "the union arm's inner member is 2755");
    assert_eq!(
        chain[0].message_args(),
        &["string | number".to_string()],
        "union arm args are the apparent type, got: {:?}",
        chain[0].message_args()
    );
}

/// Calling a never-typed get accessor with no arguments swaps the HEAD
/// to TS6234 ("This expression is not callable because it is a get
/// accessor…") — Go swaps the head message (checker.go:10217-10224), the
/// former port swapped the inner message.
#[test]
fn fcs49_invocation_error_get_accessor_head_pin() {
    let (mut c, _) = check_unit(
        concat!(
            "class A {\n",
            "    get x(): never {\n",
            "        throw 1;\n",
            "    }\n",
            "}\n",
            "const a = new A();\n",
            "a.x();\n",
        ),
        "/x/fcs49_accessor_head.ts",
        options_es2015(),
    );
    let ds = diags(&mut c);
    let heads: Vec<&tsc_ast::Diagnostic> = ds.iter().filter(|d| d.code() == 6234).collect();
    assert_eq!(
        heads.len(),
        1,
        "expected exactly 1 TS6234 head, got: {:?}",
        ds.iter().map(|d| d.code()).collect::<Vec<_>>()
    );
    let chain = heads[0].message_chain();
    assert_eq!(chain.len(), 1, "expected one chain member");
    assert_eq!(chain[0].code(), 2757, "the inner chain member is 2757");
    assert_eq!(
        chain[0].message_args(),
        &["never".to_string()],
        "inner args are [apparent type], got: {:?}",
        chain[0].message_args()
    );
    assert!(ds.iter().all(|d| d.code() != 2349 && d.code() != 2757));
}

// ── ROOT C — the dynamic-import TS7036 double-fire ──

/// `import(<non-string>)` fires TS7036 exactly ONCE, with the
/// specifier's type string as the single arg — and no collateral
/// TS2322/TS2345 from the assignability elaboration reporter (the
/// silent-isTypeAssignableTo port, Go checker.go:8487).
#[test]
fn fcs49_import_specifier_7036_single_fire_pin() {
    let (mut c, _) = check_unit(
        "const s: number = 1;\nimport(s);\n",
        "/x/fcs49_import7036.ts",
        options_es2015(),
    );
    let ds = diags(&mut c);
    let fired: Vec<&tsc_ast::Diagnostic> = ds.iter().filter(|d| d.code() == 7036).collect();
    assert_eq!(
        fired.len(),
        1,
        "expected exactly 1 TS7036, got: {:?}",
        ds.iter().map(|d| d.code()).collect::<Vec<_>>()
    );
    assert_eq!(
        fired[0].message_args(),
        &["number".to_string()],
        "7036 args are the specifier type, got: {:?}",
        fired[0].message_args()
    );
    assert!(
        ds.iter()
            .all(|d| d.code() != 2322 && d.code() != 2345 && d.code() != 2769),
        "no assignability-elaboration collateral may fire, got: {:?}",
        ds.iter().map(|d| d.code()).collect::<Vec<_>>()
    );
}

// ── ROOT B1 — the parser-recovered async-generator member gate ──

/// The parser recovery for `async * get x() { … }` (TS1005 '(' expected)
/// leaves the recovered member with a synthesized zero-length body; Go
/// gates the forced return-type check on ast.NodeIsPresent(body) — so NO
/// AsyncIterableIteratorChecked resolution, no location-less TS2318
/// (Go checker.go:3483-3486, the parser.asyncGenerators classMethods
/// es2018 family).
#[test]
fn fcs49_recovered_async_generator_member_no_2318_pin() {
    let (mut c, _) = check_unit(
        concat!(
            "class C23 {\n",
            "    async * get x() {\n",
            "        return 1;\n",
            "    }\n",
            "}\n",
        ),
        "/x/fcs49_recovered_gen.ts",
        options_es2018(),
    );
    let ds = diags(&mut c);
    let async_iterable_2318 = ds
        .iter()
        .filter(|d| {
            d.code() == 2318
                && d.message_args()
                    .first()
                    .is_some_and(|a| a == "AsyncIterableIterator")
        })
        .count();
    assert_eq!(
        async_iterable_2318, 0,
        "the recovered member must not resolve AsyncIterableIteratorChecked, got: {:?}",
        ds.iter()
            .filter(|d| d.code() == 2318)
            .map(|d| d.message_args().to_vec())
            .collect::<Vec<_>>()
    );
}

/// A REAL async generator method (present body) still forces the
/// return-type check — the AsyncIterableIteratorChecked resolution fires
/// the location-less TS2318 once (the memoized resolver), exactly as Go's
/// generator arm does for valid members.
#[test]
fn fcs49_valid_async_generator_still_resolves_pin() {
    let (mut c, _) = check_unit(
        "class D {\n    async * f() {}\n}\n",
        "/x/fcs49_valid_gen.ts",
        options_es2018(),
    );
    let ds = diags(&mut c);
    let async_iterable_2318 = ds
        .iter()
        .filter(|d| {
            d.code() == 2318
                && d.message_args()
                    .first()
                    .is_some_and(|a| a == "AsyncIterableIterator")
        })
        .count();
    assert_eq!(
        async_iterable_2318, 1,
        "the valid member resolves AsyncIterableIteratorChecked exactly once, got: {:?}",
        ds.iter()
            .filter(|d| d.code() == 2318)
            .map(|d| d.message_args().to_vec())
            .collect::<Vec<_>>()
    );
}

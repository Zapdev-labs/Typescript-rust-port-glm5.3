//! FCS57 pins (full-corpus check sweep, micro-tranche 57): the tranche's
//! three clusters —
//!
//!  * the decorator arity-head family (rust extra TS1278 x10 / missing
//!    TS1238/1240/1241): Go threads the `headMessage` from
//!    `getDiagnosticHeadMessageForDecoratorResolution` (checker.go:9004)
//!    through `resolveCall` -> `reportCallResolutionErrors` ->
//!    `getArgumentArityError`/`getTypeArgumentArityError` and chains it
//!    over every composing arm via `ast.NewDiagnosticChain` — the
//!    "Unable to resolve signature of {class,property,method} decorator
//!    when called as an expression" head becomes the PRIMARY code
//!    (1238/1240/1241) with the "The runtime will invoke the decorator
//!    with N arguments ..." arity error as the chain. The port's
//!    `get_argument_arity_error` had no head parameter at all, so the
//!    bare 1278 arity error surfaced alone (decoratorOnClass8/Property6/
//!    Property7/Method8 + esDecorators-arguments).
//!
//!  * the privateNameNestedClasses family (rust extra TS18013 x6 /
//!    missing TS18014 x6 over the same 6 positions): Go
//!    `checkPrivateIdentifierPropertyAccess`'s shadowed branch
//!    (checker.go:11754-11758) reports TS18014 ("The property '#x' cannot
//!    be accessed on type 'X' within this class because it is shadowed by
//!    another private identifier with the same spelling", args diagName +
//!    TypeToString(leftType)) with the two related-info adds (the
//!    shadowing declaration / the declaration you probably intended to
//!    use); the port emitted the generic TS18013 with NO arguments.
//!
//!  * the parserArrowFunctionExpression TS8010 family (missing x6 over
//!    parserArrowFunctionExpression10/13/14/15/16/17): the parser's
//!    `type_of` helper (Go `node.Type()`, ast/ast.go:629) has a
//!    FunctionLikeData default arm — `funcLike.Type` — that the port
//!    dropped for every function-like kind, so `checkJSSyntax`'s
//!    fallthrough never saw the RETURN TYPE of an arrow function in a JS
//!    file and the TS8010 "Type annotations can only be used in
//!    TypeScript files" on arrow return types never fired.
//!
//! ("FCS tranche 57" PORT notes in-tree.)

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

fn check_with(
    src: &str,
    kind: ScriptKind,
    file_ext: &str,
    options: CompilerOptions,
) -> (Checker, tsc_ast::NodeId) {
    let mut sf = parse_source_file(Default::default(), src, kind);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let program = TestProgram {
        options,
        name: RootedFilePath::from(format!("/x/fcs57_unit.{file_ext}")),
        source_file: sf,
        bind,
        binder_symbol_count,
        checker_symbols: Vec::new(),
    };
    let mut checker = new_checker(Box::new(program));
    checker.check_source_file(file, true);
    (checker, file)
}

fn check_ts_experimental_decorators(src: &str) -> (Checker, tsc_ast::NodeId) {
    check_with(
        src,
        ScriptKind::TS,
        "ts",
        CompilerOptions {
            target: ScriptTarget::ES2015,
            experimental_decorators: Tristate::TSTrue,
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

// ── CLUSTER A — the decorator arity-head family (1238/1240/1241) ──

/// The decoratorOnClass8 shape: `@dec()` where the decorator factory's
/// signature expects 2 arguments but the runtime invokes a class
/// decorator with 1 — the arity error now carries the TS1238 head
/// ("Unable to resolve signature of class decorator when called as an
/// expression") as the PRIMARY code; the bare TS1278 no longer surfaces
/// alone (Go checker.go:9930 getArgumentArityError headMessage chain).
#[test]
fn fcs57_class_decorator_arity_1238_head_pin() {
    let (mut c, file) = check_ts_experimental_decorators(
        "declare function dec(): (target: Function, paramIndex: number) => void;\n@dec()\nclass C {}\n",
    );
    let ds = diags(&mut c, file);
    let e1238 = codes(&ds, 1238);
    assert_eq!(e1238.len(), 1, "exactly one TS1238 head for the class decorator arity error: {ds:?}");
    assert!(codes(&ds, 1278).is_empty(), "no bare TS1278 primary: {ds:?}");
}

/// The decoratorOnClassProperty6 shape: a property decorator whose
/// signature expects 1 argument while the runtime invokes 2 — the
/// TS1240 property-decorator head is the primary code.
#[test]
fn fcs57_property_decorator_arity_1240_head_pin() {
    let (mut c, file) = check_ts_experimental_decorators(
        "declare function dec(target: Function): void;\nclass C {\n    @dec prop;\n}\n",
    );
    let ds = diags(&mut c, file);
    let e1240 = codes(&ds, 1240);
    assert_eq!(
        e1240.len(),
        1,
        "exactly one TS1240 head for the property decorator arity error: {ds:?}"
    );
    assert!(codes(&ds, 1278).is_empty(), "no bare TS1278 primary: {ds:?}");
}

/// The decoratorOnClassMethod8 shape (the corpus source): a generic
/// decorator `dec<T>(target: T): T` on a method — the runtime invokes a
/// method decorator with 3 arguments against a 1-parameter signature, the
/// arity error carries the TS1241 method-decorator head as the primary
/// code (the bare TS1278 no longer surfaces alone).
#[test]
fn fcs57_method_decorator_arity_1241_head_pin() {
    let (mut c, file) = check_ts_experimental_decorators(
        "declare function dec<T>(target: T): T;\nclass C {\n    @dec method() {}\n}\n",
    );
    let ds = diags(&mut c, file);
    let e1241 = codes(&ds, 1241);
    assert_eq!(
        e1241.len(),
        1,
        "exactly one TS1241 head for the method decorator arity error: {ds:?}"
    );
    assert!(codes(&ds, 1278).is_empty(), "no bare TS1278 primary: {ds:?}");
}

/// The control: a decorator signature that ACCEPTS the runtime's argument
/// count reports no arity error at all (neither head nor tail).
#[test]
fn fcs57_matching_decorator_signature_no_arity_error_pin() {
    let (mut c, file) = check_ts_experimental_decorators(
        "declare function dec(target: Function): void;\n@dec\nclass C {}\n",
    );
    let ds = diags(&mut c, file);
    assert!(
        codes(&ds, 1238).is_empty()
            && codes(&ds, 1240).is_empty()
            && codes(&ds, 1241).is_empty()
            && codes(&ds, 1278).is_empty(),
        "the matching decorator signature reports no arity error, got: {ds:?}"
    );
}

// ── CLUSTER B — the shadowed private-identifier TS18014 family ──

/// The privateNameNestedMethodAccess shape: inside nested class D (whose
/// own `#bar` shadows C's), accessing `c.#bar` on the OUTER type reports
/// TS18014 with args ("#bar", "C") — the shadowing branch of
/// checkPrivateIdentifierPropertyAccess (Go checker.go:11754-11758).
/// Pre-fix the generic TS18013 with no arguments fired instead.
#[test]
fn fcs57_shadowed_private_identifier_18014_pin() {
    let (mut c, file) = check_with(
        "class C {\n    #bar = 1;\n    fn() {\n        class D {\n            #bar = 2;\n            test() {\n                const c = new C();\n                return c.#bar;\n            }\n        }\n        return D;\n    }\n}\n",
        ScriptKind::TS,
        "ts",
        CompilerOptions {
            target: ScriptTarget::ES2022,
            ..CompilerOptions::default()
        },
    );
    let ds = diags(&mut c, file);
    let e18014 = codes(&ds, 18014);
    assert_eq!(
        e18014,
        vec![vec!["#bar".to_string(), "C".to_string()]],
        "exactly one TS18014 with args (#bar, C): {ds:?}"
    );
    assert!(codes(&ds, 18013).is_empty(), "no spurious TS18013: {ds:?}");
}

/// The control: accessing the private identifier OUTSIDE any class
/// reports the generic TS18013 ("not accessible outside class 'C'",
/// args ("#bar", "C")) — the non-shadowed branch is unchanged.
#[test]
fn fcs57_private_identifier_outside_class_18013_pin() {
    let (mut c, file) = check_with(
        "class C {\n    #bar = 1;\n}\nconst c = new C();\nc.#bar;\n",
        ScriptKind::TS,
        "ts",
        CompilerOptions {
            target: ScriptTarget::ES2022,
            ..CompilerOptions::default()
        },
    );
    let ds = diags(&mut c, file);
    let e18013 = codes(&ds, 18013);
    assert_eq!(
        e18013,
        vec![vec!["#bar".to_string(), "C".to_string()]],
        "exactly one TS18013 with args (#bar, C): {ds:?}"
    );
    assert!(codes(&ds, 18014).is_empty(), "no TS18014 outside a class: {ds:?}");
}

// ── CLUSTER C — the JS arrow return-type TS8010 family (parser) ──

/// The parserArrowFunctionExpression15 shape: a JS conditional whose
/// parenthesized-branch arrow carries a return type annotation —
/// checkJSSyntax now sees the arrow's RETURN TYPE (the FunctionLikeData
/// default arm of Go's node.Type(), ast/ast.go:685) and the TS8010
/// "Type annotations can only be used in TypeScript files" fires on the
/// annotation node ("string", loc 17-23). Pre-fix the parameter-only
/// port dropped every function-like kind and the 8010 never fired.
#[test]
fn fcs57_js_arrow_return_type_8010_pin() {
    let sf = parse_source_file(
        Default::default(),
        "false ? (param): string => param : null\n",
        ScriptKind::JS,
    );
    let js: Vec<(i32, i32, i32)> = sf
        .js_diagnostics()
        .iter()
        .map(|d| (d.code(), d.loc().pos(), d.loc().end()))
        .collect();
    assert_eq!(
        js,
        vec![(8010, 17, 23)],
        "exactly one TS8010 on the arrow's return type at 17-23, got: {js:?}"
    );
}

/// The parserArrowFunctionExpression14 shape: param annotations AND the
/// return type annotation each fire their own TS8010 in the JS file
/// (pre-fix the third — the `void` return type — was missing).
#[test]
fn fcs57_js_arrow_param_and_return_8010_pin() {
    let sf = parse_source_file(
        Default::default(),
        "a() ? (b: number, c?: string): void => d() : e;\n",
        ScriptKind::JS,
    );
    let js: Vec<(i32, i32, i32)> = sf
        .js_diagnostics()
        .iter()
        .filter(|d| d.code() == 8010)
        .map(|d| (d.code(), d.loc().pos(), d.loc().end()))
        .collect();
    assert_eq!(
        js,
        vec![(8010, 10, 16), (8010, 22, 28), (8010, 31, 35)],
        "the two param annotations and the void return type all fire TS8010, got: {js:?}"
    );
}

/// The control: the SAME source as a TypeScript file reports NO TS8010
/// (type annotations are legal in .ts files).
#[test]
fn fcs57_ts_arrow_return_type_no_8010_pin() {
    let sf = parse_source_file(
        Default::default(),
        "false ? (param): string => param : null\n",
        ScriptKind::TS,
    );
    assert!(
        sf.js_diagnostics().is_empty(),
        "no JS-file 8010 in a TypeScript file, got: {:?}",
        sf.js_diagnostics()
    );
}

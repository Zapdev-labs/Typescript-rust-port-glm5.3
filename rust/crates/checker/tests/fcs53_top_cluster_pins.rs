//! FCS53 pins (full-corpus check sweep, micro-tranche 53): the tranche's
//! three clusters —
//!
//!  * the generatedContextualTyping TS2322 x10 family: the rust
//!    `forEachReturnStatement` walk descended into nested function-likes
//!    (a plain `children_of` recursion) where Go's ast.ForEachReturnStatement
//!    (ast/utilities.go:1157) traverses ONLY statement container kinds —
//!    so a `var x157: () => () => Base[] = () => { return () => [d1, d2]; }`
//!    aggregated the inner arrow's `[d1, d2]` return into the OUTER
//!    arrow's return union, and the assignability check against
//!    `() => () => Base[]` failed with a spurious TS2322. Fixed Go-exactly
//!    in statements.rs (the statement-kind ladder);
//!  * the declarationsAndAssignments TS2403 x8 family: the rest-element arm
//!    of getBindingElementTypeFromParentType (typeofsymbol.rs) was a
//!    permissive identity stub (create_array_type always) where Go
//!    checker.go:18156-18166 slices the TUPLE — `var [...a3] = v` with
//!    v: [number, number, number] now types a3 as the remaining tuple
//!    (here the whole tuple), so the secondary-variable-declaration
//!    identical-type check passes instead of firing TS2403
//!    "must be of type '[number, number, number]', but here has type
//!    'number[]'";
//!  * the rewriteRelativeImportExtensions TS2307 x6 family: Go
//!    checker.go:8551 redirects CommonJS require calls in JS files to
//!    resolveExternalModuleTypeByLiteral (checker.go:14731) — the module
//!    reference resolves WITH error reporting, so the js.js
//!    `require("./foo.ts")` calls emit TS2307 when the specifier doesn't
//!    resolve. The rust check_call_expression carried a W1-era TODO stub;
//!    the real isCommonJSRequire ladder (checker.go:16060 — the synthetic
//!    require symbol or an ambient function/variable declaration) landed
//!    in resolution.rs.
//!
//! ("FCS tranche 53" PORT notes in-tree.)

use tsc_ast::{NodeStore, SourceFile, SourceFileParseOptions, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
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

fn check_unit(
    src: &str,
    name: &str,
    kind: ScriptKind,
    options: CompilerOptions,
) -> (Checker, tsc_ast::NodeId) {
    let file_name = RootedFilePath::from(name);
    let path_key =
        tsc_tspath::CaseSensitivity::CaseSensitive.path_key(&file_name.as_path());
    let opts = SourceFileParseOptions {
        file_name,
        path_key,
        external_module_indicator_options: Default::default(),
    };
    let mut sf = parse_source_file(opts, src, kind);
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

fn options_es2015_js_strict() -> CompilerOptions {
    let mut options = options_es2015();
    options.allow_js = Tristate::TSTrue;
    options.check_js = Tristate::TSTrue;
    options.strict = Tristate::TSTrue;
    options
}

// ── CLUSTER A — the return-statement walk (nested contextual typing) ──

const GCT_PRELUDE: &str = "class Base { private p; }\nclass Derived1 extends Base { private m; }\nclass Derived2 extends Base { private n; }\ninterface Genric<T> { func(n: T[]); }\nvar b = new Base(), d1 = new Derived1(), d2 = new Derived2();\n";

/// The corpus shape (generatedContextualTyping x157/x160): the outer
/// arrow's return union must aggregate ONLY the outer's own return
/// statement — the nested arrow's `[d1, d2]` return stays inside the
/// inner function, so the initializer's type stays
/// `() => (() => Base[])` and the assignment is clean. Pre-fix the walk
/// leaked the inner return and rust reported a spurious TS2322.
#[test]
fn fcs53_nested_contextual_arrow_pin() {
    let (mut c, _) = check_unit(
        &format!(
            "{GCT_PRELUDE}var x157: () => () => Base[] = () => {{ return () => [d1, d2]; }};\nvar x160: () => {{ (): Base[]; }} = () => {{ return () => [d1, d2]; }};\n"
        ),
        "/x/fcs53_nested_arrow.ts",
        ScriptKind::TS,
        options_es2015(),
    );
    let ds = diags(&mut c);
    assert!(
        !ds.iter().any(|d| d.code() == 2322),
        "the nested-contextual arrow assignments must be clean (no leaked inner returns), got: {:?}",
        ds.iter().map(|d| (d.code(), d.message_args())).collect::<Vec<_>>()
    );
}

/// The inner-plain-function twin (generatedContextualTyping x158/x161/x170/
/// x173) — the walk leaked through nested function EXPRESSIONS too; both
/// the arrow-nested and function-nested forms must stay clean.
#[test]
fn fcs53_nested_contextual_function_expression_pin() {
    let (mut c, _) = check_unit(
        &format!(
            "{GCT_PRELUDE}var x158: () => () => Base[] = () => {{ return function() {{ return [d1, d2] }}; }};\nvar x170: () => () => Base[] = function() {{ return function() {{ return [d1, d2] }}; }};\nvar x168: () => Genric<Base> = () => {{ return {{ func: n => {{ return [d1, d2]; }} }}; }};\n"
        ),
        "/x/fcs53_nested_function.ts",
        ScriptKind::TS,
        options_es2015(),
    );
    let ds = diags(&mut c);
    assert!(
        !ds.iter().any(|d| d.code() == 2322),
        "the nested-contextual function-expression assignments must be clean, got: {:?}",
        ds.iter().map(|d| (d.code(), d.message_args())).collect::<Vec<_>>()
    );
}

// ── CLUSTER C — the tuple-rest slice in binding elements ──

/// The corpus shape (declarationsAndAssignments f20/f21): a rest binding
/// element over a TUPLE parent types as the remaining tuple — the
/// secondary declaration of `a3` is identical to `var a3!:
/// [number, number, number]`, so NO TS2403 fires. Pre-fix the rest arm
/// produced an array of the element type and the identical-type check
/// failed.
#[test]
fn fcs53_tuple_rest_redeclaration_identical_pin() {
    let (mut c, _) = check_unit(
        "function f20(v: [number, number, number]) {\n    var a3!: [number, number, number];\n    var [...a3] = v;\n    var a2!: [number, number];\n    var [x, ...a2] = v;\n    var a0!: [];\n    var [x, y, z, ...a0] = v;\n}\n",
        "/x/fcs53_tuple_rest.ts",
        ScriptKind::TS,
        options_es2015(),
    );
    let ds = diags(&mut c);
    assert!(
        !ds.iter().any(|d| d.code() == 2403),
        "the tuple-rest secondary declarations must be identical to the primary tuple types, got: {:?}",
        ds.iter().map(|d| (d.code(), d.message_args())).collect::<Vec<_>>()
    );
}

/// The discriminating twin: the rest element's type is the sliced TUPLE
/// (not an array) — redeclaring against a DIFFERENT primary type fires
/// TS2403 with the tuple as the NEXT type ("Variable 'a3' must be of type
/// 'string', but here has type '[number, number, number]'"). Pre-fix the
/// message reported 'number[]' here.
#[test]
fn fcs53_tuple_rest_redeclaration_mismatch_pin() {
    let (mut c, _) = check_unit(
        "function g(v: [number, number, number]) {\n    var a3!: string;\n    var [...a3] = v;\n}\n",
        "/x/fcs53_tuple_rest_mismatch.ts",
        ScriptKind::TS,
        options_es2015(),
    );
    let ds = diags(&mut c);
    let redeclared: Vec<&tsc_ast::Diagnostic> =
        ds.iter().filter(|d| d.code() == 2403).collect();
    assert_eq!(
        redeclared.len(),
        1,
        "exactly one 2403 expected, got: {:?}",
        ds.iter().map(|d| (d.code(), d.message_args())).collect::<Vec<_>>()
    );
    assert_eq!(
        redeclared[0].message_args(),
        &vec![
            "a3".to_string(),
            "string".to_string(),
            "[number, number, number]".to_string(),
        ],
        "the 2403 must report the sliced tuple as the next type"
    );
}

// ── CLUSTER B — the CommonJS require-call module resolution in JS ──

/// The corpus shape (rewriteRelativeImportExtensions js.js): a
/// `require("./foo.ts")` call in a JS file resolves its module reference
/// WITH error reporting — the unresolvable specifier fires TS2307
/// ("Cannot find module './foo.ts' or its corresponding type
/// declarations"). Pre-fix the call returned the (any-typed) declared
/// require signature and the module resolution never ran.
#[test]
fn fcs53_js_require_call_module_not_found_pin() {
    let (mut c, _) = check_unit(
        "var path = require(\"./foo.ts\");\n",
        "/x/fcs53_require.js",
        ScriptKind::JS,
        options_es2015_js_strict(),
    );
    let ds = diags(&mut c);
    let not_found: Vec<&tsc_ast::Diagnostic> =
        ds.iter().filter(|d| d.code() == 2307).collect();
    assert_eq!(
        not_found.len(),
        1,
        "the require call must resolve-and-report its module specifier, got: {:?}",
        ds.iter().map(|d| (d.code(), d.message_args())).collect::<Vec<_>>()
    );
    assert_eq!(
        not_found[0].message_args(),
        &vec!["./foo.ts".to_string()],
        "the 2307 must name the require specifier"
    );
}

//! FCS56 pins (full-corpus check sweep, micro-tranche 56): the tranche's
//! three clusters —
//!
//!  * the exhaustiveSwitchStatements1 TS2366 x7 family (single record):
//!    `computeExhaustiveSwitchStatement` (flow.rs, Go flow.go:1949)
//!    answers "not literal" for the declared `1 | 2`-style discriminant
//!    unions because the port's shared `is_literal_type` (relater.rs) is
//!    the literal-flags-only form — Go's isLiteralType (checker.go:25927)
//!    has a UNION arm: a union is a literal type when it carries
//!    TypeFlagsEnumLiteral, or when every constituent is a unit type.
//!    Without the arm the exhaustive literal/enum switches were never
//!    marked exhaustive, the unmatched-value bypass flow stayed reachable,
//!    and every `function f(x: 1 | 2): string { switch (x) { ... } }`
//!    over-emitted TS2366 (x7 in exhaustiveSwitchStatements1; the baseline
//!    has NO 2366 there). The arm is scoped to the exhaustiveness
//!    consumer exactly like the tranche-26 BOOLEAN arm (the shared helper
//!    stays simplified until its remaining consumers are Go-exact).
//!
//!  * the jsdoc TS8030 x11/x7 family (checkJsdocTypeTag6 x4,
//!    jsdocThisType x2, checkJsdocTypeTag5/OnObjectProperty1/2): the
//!    JSDoc @type full-signature checks never ran — Go
//!    checkFunctionOrMethodDeclaration (checker.go:3471-3475) and
//!    checkFunctionExpressionOrObjectLiteralMethod (checker.go:10341 +
//!    10372-10376) both checkSourceElement(FullSignature) and probe
//!    getContextualCallSignature(getTypeFromTypeNode(FullSignature));
//!    a null contextual call signature fires TS8030 ("A JSDoc type tag
//!    on a function must have a signature with the correct number of
//!    arguments"). The port had the M6 W4/S3 permissive seams at both
//!    sites (statements.rs / expressions.rs). The same family's
//!    checkAllCodePathsInNonVoidFunctionReturnOrThrow error-node now
//!    falls back to the JSDoc full signature (Go checker.go:3779-3782;
//!    was the declaration node itself).
//!
//!  * the nodeModulesImportModeDeclarationEmit TS1453 x8 family (x8
//!    records over nodeModulesImportAttributesModeDeclarationEmitErrors +
//!    nodeModulesImportModeDeclarationEmitErrors1, 4 module variants
//!    each): `checkImportAttributes`'s getResolutionModeOverride call
//!    (Go checker.go:5569) passes reportErrors = isTypeOnly, so a bad
//!    resolution-mode value on a TYPE-ONLY import fires TS1453
//!    ("resolution mode should be either require or import") on the
//!    attribute VALUE node; the port had the permissive `false`
//!    stand-in for the override (neither the 1453 error nor the
//!    X_resolution_mode_can_only_be_set_for_type_only_imports check ever
//!    ran).
//!
//! ("FCS tranche 56" PORT notes in-tree.)

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
        name: RootedFilePath::from(format!("/x/fcs56_unit.{file_ext}")),
        source_file: sf,
        bind,
        binder_symbol_count,
        checker_symbols: Vec::new(),
    };
    let mut checker = new_checker(Box::new(program));
    checker.check_source_file(file, true);
    (checker, file)
}

fn check_ts_strict(src: &str) -> (Checker, tsc_ast::NodeId) {
    check_with(
        src,
        ScriptKind::TS,
        "ts",
        CompilerOptions {
            target: ScriptTarget::ES2022,
            strict: Tristate::TSTrue,
            ..CompilerOptions::default()
        },
    )
}

fn check_js_checkjs(src: &str) -> (Checker, tsc_ast::NodeId) {
    check_with(
        src,
        ScriptKind::JS,
        "js",
        CompilerOptions {
            check_js: Tristate::TSTrue,
            allow_js: Tristate::TSTrue,
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

// ── CLUSTER A — the exhaustiveSwitchStatements1 TS2366 family ──

/// The corpus shape (exhaustiveSwitchStatements1 f1/f): an exhaustive
/// literal-union switch (no default) with every case returning leaves the
/// function's end flow node UNREACHABLE, so a return-type annotation that
/// excludes undefined reports NO TS2366. Pre-fix the union arm of Go's
/// isLiteralType (checker.go:25931-25934) was missing and the bypass
/// flow stayed reachable — spurious TS2366 per function (x7 in the one
/// record).
#[test]
fn fcs56_exhaustive_union_switch_no_2366_pin() {
    let (mut c, file) = check_ts_strict(
        "function f(x: 1 | 2): string {\n    switch (x) {\n        case 1: return \"a\";\n        case 2: return \"b\";\n    }\n}\n",
    );
    let ds = diags(&mut c, file);
    assert!(
        codes(&ds, 2366).is_empty(),
        "the exhaustive literal-union switch leaves the end unreachable (no TS2366), got: {ds:?}"
    );
}

/// The enum shape (exhaustiveSwitchStatements1 f/g): the enum union
/// discriminant is literal through the EnumLiteral flag arm; exhaustive
/// enum switches with all cases returning report NO TS2366.
#[test]
fn fcs56_exhaustive_enum_switch_no_2366_pin() {
    let (mut c, file) = check_ts_strict(
        "enum E { A, B }\nfunction f(e: E): number {\n    switch (e) {\n        case E.A: return 0;\n        case E.B: return 1;\n    }\n}\n",
    );
    let ds = diags(&mut c, file);
    assert!(
        codes(&ds, 2366).is_empty(),
        "the exhaustive enum switch leaves the end unreachable (no TS2366), got: {ds:?}"
    );
}

/// The boolean shape (the tranche-26 scoped arm): an exhaustive boolean
/// switch still reports NO TS2366 — the tranche-56 union arm must not
/// regress the boolean arm.
#[test]
fn fcs56_exhaustive_boolean_switch_no_2366_pin() {
    let (mut c, file) = check_ts_strict(
        "function f(b: boolean): string {\n    switch (b) {\n        case true: return \"a\";\n        case false: return \"b\";\n    }\n}\n",
    );
    let ds = diags(&mut c, file);
    assert!(
        codes(&ds, 2366).is_empty(),
        "the exhaustive boolean switch leaves the end unreachable (no TS2366), got: {ds:?}"
    );
}

/// The NON-exhaustive control: a case is missing, the bypass flow is
/// reachable, and TS2366 STILL fires (the union arm must not mark
/// non-exhaustive switches exhaustive).
#[test]
fn fcs56_non_exhaustive_switch_still_2366_pin() {
    let (mut c, file) = check_ts_strict(
        "function f(x: 1 | 2 | 3): string {\n    switch (x) {\n        case 1: return \"a\";\n        case 2: return \"b\";\n    }\n}\n",
    );
    let ds = diags(&mut c, file);
    let e2366 = codes(&ds, 2366);
    assert_eq!(e2366.len(), 1, "exactly one TS2366 for the missing case: {ds:?}");
}

// ── CLUSTER B — the jsdoc TS8030 family ──

/// The checkJsdocTypeTag6 shape: a JSDoc `@type` tag whose type is not
/// callable (`{number}` on a function) cannot contextually call the
/// function, so the FullSignature probe fires exactly one TS8030.
#[test]
fn fcs56_jsdoc_type_tag_not_callable_8030_pin() {
    let (mut c, file) = check_js_checkjs("/** @type {number} */\nfunction f() {\n    return 1\n}\n");
    let ds = diags(&mut c, file);
    let e8030 = codes(&ds, 8030);
    assert_eq!(e8030.len(), 1, "exactly one TS8030 for the non-callable @type tag: {ds:?}");
}

/// The checkJsdocTypeTag6 control: a `@type` tag with the CORRECT
/// parameter count ({(a: number) => number} on a 1-parameter function)
/// reports NO TS8030.
#[test]
fn fcs56_jsdoc_type_tag_matching_signature_no_8030_pin() {
    let (mut c, file) = check_js_checkjs(
        "/** @type {(a: number) => number} */\nfunction add(a) {\n    return a\n}\n",
    );
    let ds = diags(&mut c, file);
    assert!(
        codes(&ds, 8030).is_empty(),
        "the matching @type tag contextually calls (no TS8030), got: {ds:?}"
    );
}

/// The checkJsdocTypeTag6 wrong-COUNT shape: the tag's signature takes one
/// parameter FEWER than the function — the contextual call signature probe
/// rejects it and TS8030 fires.
#[test]
fn fcs56_jsdoc_type_tag_wrong_arg_count_8030_pin() {
    let (mut c, file) = check_js_checkjs(
        "/** @type {(a: number) => number} */\nfunction add1(a, b) {\n    return a + b\n}\n",
    );
    let ds = diags(&mut c, file);
    let e8030 = codes(&ds, 8030);
    assert_eq!(
        e8030.len(),
        1,
        "exactly one TS8030 for the wrong-parameter-count @type tag: {ds:?}"
    );
}

// ── CLUSTER C — the nodeModulesImportModeDeclarationEmit TS1453 family ──

/// The corpus shape (nodeModulesImportModeDeclarationEmitErrors1): a bad
/// resolution-mode value ("foobar") on a TYPE-ONLY import's attributes
/// fires TS1453 on the attribute VALUE node (reportErrors = isTypeOnly,
/// Go checker.go:5569). Pre-fix the override stand-in answered none and
/// the error never fired.
#[test]
fn fcs56_type_only_import_bad_resolution_mode_1453_pin() {
    let (mut c, file) = check_with(
        "import type { A } from \"pkg\" with { \"resolution-mode\": \"foobar\" };\nexport { A };\n",
        ScriptKind::TS,
        "ts",
        CompilerOptions {
            module: ModuleKind::NodeNext,
            ..CompilerOptions::default()
        },
    );
    let ds = diags(&mut c, file);
    let e1453 = codes(&ds, 1453);
    assert_eq!(e1453.len(), 1, "exactly one TS1453 for the bad resolution-mode: {ds:?}");
}

/// The valid-value control: a "require" resolution-mode on a type-only
/// import reports NO TS1453.
#[test]
fn fcs56_type_only_import_valid_resolution_mode_no_1453_pin() {
    let (mut c, file) = check_with(
        "import type { A } from \"pkg\" with { \"resolution-mode\": \"require\" };\nexport { A };\n",
        ScriptKind::TS,
        "ts",
        CompilerOptions {
            module: ModuleKind::NodeNext,
            ..CompilerOptions::default()
        },
    );
    let ds = diags(&mut c, file);
    assert!(
        codes(&ds, 1453).is_empty(),
        "the valid resolution-mode value reports no TS1453, got: {ds:?}"
    );
}

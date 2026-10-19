//! FCS58 pins (full-corpus check sweep, micro-tranche 58): the tranche's
//! families —
//!
//!  * the privateNames family (rust extra TS2739 x8/7 + TS2741 x7/7): two
//!    roots in the unmatched-property reporting path. (1) Go's
//!    `getUnmatchedPropertiesWorker` filters static private-identifier
//!    target props (relater.go:981, the privateNamesAndStaticFields TODO)
//!    — the port's `is_static_private_identifier_property` was the M6 S6
//!    `false` stub, so `class A { static #foo }` listed `#foo` as
//!    "missing" (TS2739). (2) Go's `reportUnmatchedProperty` has the
//!    private-identifier arm (relater.go:4379-4391): when the unmatched
//!    target prop is a private identifier and the SOURCE type is a class
//!    with its own private member of the same description, TS18015
//!    ("Property '{0}' in type '{1}' refers to a different member ...")
//!    is reported instead of falling through to TS2741/TS2739.
//!
//!  * the for-of/using TS2448 family (missing x7/7): Go's
//!    `isImmediatelyUsedInInitializerOfBlockScopedVariable`
//!    (checker.go:2116) checks the use site against the for-in/of
//!    statement's EXPRESSION (`grandparent.Expression()`, checker.go:2127);
//!    the port passed the loop initializer (the declaration side), so
//!    `for (let v of v)` never reported "Block-scoped variable 'v' used
//!    before its declaration" on the iterated RHS.
//!
//!  * the jsx TS2695 family (rust extra x7/4): Go's comma-operator arm in
//!    `checkBinaryLikeExpression` (checker.go:12774-12781) skips the
//!    "Left side of comma operator is unused ..." error when the left
//!    operand's start sits inside a parser diagnostic 2657 ("JSX
//!    expressions must have one parent element") — the error-recovered
//!    sibling-JSX binary expression; the port always reported.
//!
//!  * the constructableDecoratorOnClass01 residual: Go's decorator
//!    no-call-signatures branch passes `node.Expression()` as the error
//!    target (checker.go:8981 `invocationErrorDetails(node.Expression(),
//!    ...)`); the port passed the decorator node itself, so the chained
//!    TS1238 head spanned the `@` too ((19,28) vs Go's (20,28)).
//!
//!  * the privateNamesUnique-4 sub-shape (interface source extending a
//!    class): Go's `isThislessInterface` (checker.go:17743) reads heritage
//!    names via `ast.GetHeritageClauseElementName` (utilities.go:1759 —
//!    the TypeReference arm FIRST, an interface's `extends A1` element is
//!    a TypeReference node, not an ExpressionWithTypeArguments) and
//!    resolves the base via resolveEntityName; the port read only the
//!    ExpressionWithTypeArguments arm (name always None for interfaces =>
//!    thisless wrongly true => no this type / no ObjectFlagsReference), so
//!    `getSingleBaseForNonAugmentingSubtype` (checker.go:28621, the M6 S7
//!    `None` stub, now ported in full) never collapsed `interface A2
//!    extends (class) A1` to A1 for reading comparisons — the mismatch
//!    chain then named A2 and the reportRelationError TS2741-suppression
//!    args matched, surfacing the bare 2741 instead of Go's 2322+chain.
//!
//! ("FCS tranche 58" PORT notes in-tree.)

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::compileroptions::ResolutionModeNone;
use tsc_core::options_generated::{CompilerOptions, JsxEmit, ModuleKind, ScriptTarget};
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
        name: RootedFilePath::from(format!("/x/fcs58_unit.{file_ext}")),
        source_file: sf,
        bind,
        binder_symbol_count,
        checker_symbols: Vec::new(),
    };
    let mut checker = new_checker(Box::new(program));
    checker.check_source_file(file, true);
    (checker, file)
}

fn check_ts(src: &str) -> (Checker, tsc_ast::NodeId) {
    check_with(
        src,
        ScriptKind::TS,
        "ts",
        CompilerOptions {
            target: ScriptTarget::ES2015,
            ..CompilerOptions::default()
        },
    )
}

fn check_tsx_preserve(src: &str) -> (Checker, tsc_ast::NodeId) {
    check_with(
        src,
        ScriptKind::TSX,
        "tsx",
        CompilerOptions {
            target: ScriptTarget::ES2015,
            jsx: JsxEmit::Preserve,
            ..CompilerOptions::default()
        },
    )
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

// ── CLUSTER A — the privateNames family (2739 / 2741 / 18015) ──

/// The privateNamesAndStaticFields shape: comparing a class side that
/// lacks the base's static private fields — Go filters static
/// private-identifier target props in getUnmatchedPropertiesWorker
/// (relater.go:981), so no TS2739 "missing #foo" is reported ("OK for
/// now", per the corpus comment).
#[test]
fn fcs58_static_private_field_not_missing_2739_pin() {
    let (mut c, file) = check_ts(
        "class A {\n    static #foo: number;\n}\nclass B {}\nconst willErrorSomeDay: typeof A = B;\n",
    );
    let ds = diags(&mut c, file);
    assert!(
        codes(&ds, 2739).is_empty(),
        "static private identifier fields are filtered out of getUnmatchedProperties: {ds:?}"
    );
}

/// The privateNamesUnique-1 shape: two classes with distinct instance
/// private fields of the same description — Go's reportUnmatchedProperty
/// private-identifier arm reports TS18015 ("Property '{0}' in type '{1}'
/// refers to a different member that cannot be accessed from within type
/// '{2}'") chained under the TS2322 head instead of falling through to a
/// standalone TS2741.
#[test]
fn fcs58_private_identifier_18015_not_2741_pin() {
    let (mut c, file) = check_ts(
        "class A {\n    #foo: number;\n}\nclass B {\n    #foo: number;\n}\nconst b: A = new B();\n",
    );
    let ds = diags(&mut c, file);
    let e2322 = codes(&ds, 2322);
    assert_eq!(
        e2322.len(),
        1,
        "exactly one TS2322 head for the class-pair assignment: {ds:?}"
    );
    assert_eq!(e2322[0][0], "B");
    assert_eq!(e2322[0][1], "A");
    // The TS18015 refers-to-a-different-member elaboration rides the 2322
    // MESSAGE CHAIN (the baseline's second "!!! error TS2322" line is the
    // chain tail; NewDiagnosticChain, not related information).
    let mut found18015: Vec<Vec<String>> = Vec::new();
    for d in c.get_file_diagnostics(file).iter() {
        for link in std::iter::once(d)
            .chain(d.message_chain().iter())
            .chain(d.related_information().iter())
        {
            if link.code() == 18015 {
                found18015.push(link.message_args().to_vec());
            }
        }
    }
    assert_eq!(
        found18015.len(),
        1,
        "exactly one TS18015 refers-to-a-different-member elaboration: {ds:?}"
    );
    assert_eq!(found18015[0][0], "#foo", "the description is the private identifier text");
    assert_eq!(found18015[0][1], "B", "the source type is the class being assigned");
    assert_eq!(found18015[0][2], "A", "the target type is the annotated class");
    assert!(
        codes(&ds, 2741).is_empty(),
        "no bare TS2741 missing-property fallback: {ds:?}"
    );
}

/// The privateNamesUnique-4 sub-shape: the SOURCE is an interface
/// extending a class (so NOT the 18015 arm — source.symbol is not a
/// class), and getSingleBaseForNonAugmentingSubtype collapses
/// `interface A2 extends A1` to A1 for reading comparisons, so the
/// mismatch chain names A1 and the reportRelationError TS2741-suppression
/// args no longer match — the TS2322 head ("Type 'A2' is not assignable to
/// type 'C'") surfaces with the chained TS2741 elaboration instead of a
/// bare standalone TS2741 with (A2, C).
#[test]
fn fcs58_interface_extends_class_2322_head_pin() {
    let (mut c, file) = check_ts(
        "class A1 { }\ninterface A2 extends A1 { }\ndeclare const a: A2;\nclass C { #something: number }\nconst c: C = a;\n",
    );
    let ds = diags(&mut c, file);
    let e2322 = codes(&ds, 2322);
    assert_eq!(
        e2322.len(),
        1,
        "exactly one TS2322 head for the interface-extends-class assignment: {ds:?}"
    );
    assert_eq!(e2322[0][0], "A2", "the head names the original source type A2");
    assert_eq!(e2322[0][1], "C", "the head names the target type C");
    // The standalone TS2741 with (A2, C) must NOT surface as a primary.
    for args in codes(&ds, 2741) {
        assert_ne!(
            (args.get(1).map(String::as_str), args.get(2).map(String::as_str)),
            (Some("A2"), Some("C")),
            "no bare TS2741 primary naming (A2, C): {ds:?}"
        );
    }
}

/// The control for the collapse: an interface with NO base does not
/// collapse, the mismatch chain names A2 itself, and the suppression
/// args match — the bare TS2741 (#something, A2, C) IS the primary
/// (v2 in the scratch corpus; both drivers agree).
#[test]
fn fcs58_standalone_interface_2741_control_pin() {
    let (mut c, file) = check_ts(
        "interface A2 { }\ndeclare const a: A2;\nclass C { #something: number }\nconst c: C = a;\n",
    );
    let ds = diags(&mut c, file);
    let e2741 = codes(&ds, 2741);
    assert_eq!(
        e2741.len(),
        1,
        "the standalone-interface shape keeps the bare TS2741 primary: {ds:?}"
    );
    assert_eq!(e2741[0][0], "#something");
    assert_eq!(e2741[0][1], "A2");
    assert_eq!(e2741[0][2], "C");
}

// ── CLUSTER B — the for-of/using TS2448 family ──

/// The for-of55 shape: the loop variable referenced in the ITERATED
/// expression of its own for-of — Go's
/// isImmediatelyUsedInInitializerOfBlockScopedVariable checks the use
/// site against `grandparent.Expression()` (checker.go:2127), so the RHS
/// reference is "used before its declaration" (TS2448, arg the loop
/// variable name).
#[test]
fn fcs58_for_of_rhs_use_before_declaration_2448_pin() {
    let (mut c, file) = check_ts("let v = [1];\nfor (let v of v) {\n    v;\n}\n");
    let ds = diags(&mut c, file);
    let e2448 = codes(&ds, 2448);
    assert_eq!(
        e2448.len(),
        1,
        "exactly one TS2448 for the for-of RHS self-reference: {ds:?}"
    );
    assert_eq!(e2448[0][0], "v", "the diagnostic names the loop variable");
}

/// The control: referencing a declared variable in the for-of RHS is
/// fine when it is not the loop variable being declared.
#[test]
fn fcs58_for_of_declared_rhs_no_2448_pin() {
    let (mut c, file) = check_ts("let v = [1];\nfor (let w of v) {\n    v;\n}\n");
    let ds = diags(&mut c, file);
    assert!(
        codes(&ds, 2448).is_empty(),
        "no use-before-declaration when the RHS names a different variable: {ds:?}"
    );
}

// ── CLUSTER C — the jsx TS2695 family ──

/// The tsxErrorRecovery2 shape: two sibling JSX elements with no parent —
/// the parser error-recovers into a comma binary expression and reports
/// TS2657 ("JSX expressions must have one parent element"); Go's comma
/// arm (checker.go:12774-12781) suppresses the TS2695 comma-operator
/// error for a left operand whose start sits inside the 2657.
#[test]
fn fcs58_sibling_jsx_no_2695_pin() {
    let src = "declare namespace JSX { interface Element { } }\n\n<div></div>\n<div></div>\n";
    // The TS2657 lives in the PARSER diagnostics (the checker's list
    // only carries what check_source_file reports, e.g. the TS7026s).
    let sf = parse_source_file(Default::default(), src, ScriptKind::TSX);
    assert!(
        sf.diagnostics().iter().any(|d| d.code() == 2657),
        "the parser reports TS2657 for the sibling elements: {:?}",
        sf.diagnostics().iter().map(|d| d.code()).collect::<Vec<_>>()
    );
    let (mut c, file) = check_tsx_preserve(src);
    let ds = diags(&mut c, file);
    assert!(
        codes(&ds, 2695).is_empty(),
        "the comma-operator error is suppressed inside the 2657 span: {ds:?}"
    );
}

/// The control: a side-effect-free comma operand OUTSIDE any 2657 span
/// still reports TS2695.
#[test]
fn fcs58_plain_comma_still_2695_pin() {
    let (mut c, file) = check_ts("let x = (0, 1);\n");
    let ds = diags(&mut c, file);
    assert_eq!(
        codes(&ds, 2695).len(),
        1,
        "the plain comma operator keeps its TS2695: {ds:?}"
    );
}

// ── CLUSTER D — the constructableDecoratorOnClass01 residual ──

/// The constructableDecoratorOnClass01 shape: a class used as a decorator
/// has construct signatures but no call signatures — the no-signatures
/// branch chains the TS1238 head over invocationErrorDetails(node
/// .Expression()); the head is the primary and its span starts at the
/// expression (after the `@`), not at the decorator node.
#[test]
fn fcs58_class_decorator_no_call_signatures_1238_head_pin() {
    let (mut c, file) = check_ts_experimental_decorators(
        "class CtorDtor {}\n\n@CtorDtor\nclass C {\n\n}\n",
    );
    let ds = diags(&mut c, file);
    let e1238 = codes(&ds, 1238);
    assert_eq!(
        e1238.len(),
        1,
        "exactly one TS1238 head for the class-decorator resolution failure: {ds:?}"
    );
}

//! FCS5 pins (full-corpus check sweep, tranche 5): the phantom
//! Duplicate_identifier_2300 object-literal family —
//! es6/computedProperties/computedPropertyNames10_ES5.ts shape.
//!
//! Root cause: the port's
//! `get_property_name_for_property_name_node` returned the EMPTY string
//! where Go's `GetPropertyNameForPropertyNameNode` returns the
//! `InternalSymbolNameMissing` sentinel ("\u{FE}missing") for non-
//! literal-like computed names; `getEffectivePropertyNameFor
//! PropertyNameNode`'s `name != INTERNAL_SYMBOL_NAME_MISSING` gate then
//! treated "" as a REAL property name, and every non-literal-like
//! computed name collided on the "" key in
//! `checkGrammarObjectLiteralExpression`'s seen map — one phantom
//! TS2300 per computed-name object-literal method (Go emits none).
//!
//! The fix pair: (1) the sentinel in the ast_shims fallback arm; (2)
//! `getEffectivePropertyNameForPropertyNameNode`'s computed-name arm is
//! REAL (Go checker.go:19126's tryGetNameFromType(getTypeOfExpression))
//! so const-literal-typed computed names now resolve to their value.

use tsc_ast::{NodeId, NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::options_generated::CompilerOptions;
use tsc_core::scriptkind::ScriptKind;
use tsc_parser::parse_source_file;

// ── The single-file Program scaffold (the fcs4 idiom) ──

struct TestProgram {
    options: CompilerOptions,
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
}

fn check_file(src: &str) -> (Checker, NodeId) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let program = TestProgram {
        options: CompilerOptions::default(),
        source_file: sf,
        bind,
        binder_symbol_count,
        checker_symbols: Vec::new(),
    };
    let mut checker = new_checker(Box::new(program));
    checker.check_source_file(file, true);
    (checker, file)
}

fn diag_codes(c: &mut Checker, file: NodeId) -> Vec<i32> {
    c.get_file_diagnostics(file)
        .iter()
        .map(|d| d.code())
        .collect()
}

// ── The pins ───────────────────────────────────────────────────────────────

/// computedPropertyNames10_ES5.ts's shape: the computed-name method zoo
/// (non-literal-like names all the way down). Pre-fix the port emitted 8
/// phantom TS2300s (every non-literal-like computed name collided on the
/// "" key of checkGrammarObjectLiteralExpression's seen map); Go emits
/// none — the fix restores the `InternalSymbolNameMissing` sentinel so
/// the effective-name gate skips them exactly like Go's
/// `getEffectivePropertyNameForPropertyNameNode`.
#[test]
fn computed_name_method_zoo_no_phantom_2300_pin() {
    let (mut c, file) = check_file(
        "var s: string;\nvar n: number;\nvar a: any;\nvar v = {\n    [s]() { },\n    [n]() { },\n    [s + s]() { },\n    [s + n]() { },\n    [+s]() { },\n    [\"\"]() { },\n    [0]() { },\n    [a]() { },\n    [<any>true]() { },\n    [`hello bye`]() { },\n    [`hello ${a} bye`]() { }\n}\n",
    );
    let codes = diag_codes(&mut c, file);
    assert!(
        !codes.contains(&2300),
        "phantom Duplicate_identifier_2300 in the computed-name method zoo: {codes:?}"
    );
}

/// The literal-like names still pair: `[""]` (the empty string), `[0]` and
/// the no-substitution template all carry distinct REAL names — a literal
/// duplicate of one of them still reports (Go's seen map keeps the
/// literal-like entries).
#[test]
fn literal_like_computed_name_duplicates_still_report_pin() {
    let (mut c, file) = check_file("var v = {\n    [0]() { },\n    0() { },\n}\n");
    let codes = diag_codes(&mut c, file);
    assert!(
        codes.contains(&2300),
        "literal-like computed-name duplicate must still report TS2300: {codes:?}"
    );
}

/// A non-literal-like computed name (`s: string`) next to the literal
/// `""` property: Go's effective name for `[s]` is MISSING (string type,
/// tryGetNameFromType → false) while `""`'s is the empty-string literal's
/// value — NO collision. Pre-fix BOTH keyed "" and the phantom 2300 fired.
#[test]
fn computed_string_name_vs_empty_string_literal_no_collision_pin() {
    let (mut c, file) = check_file("var s: string;\nvar v = {\n    \"\": 1,\n    [s]: 2,\n}\n");
    let codes = diag_codes(&mut c, file);
    assert!(
        !codes.contains(&2300) && !codes.contains(&1117),
        "phantom duplicate for the computed string-typed name: {codes:?}"
    );
}

/// The REAL tryGetNameFromType path (Go checker.go:19126): a
/// const-literal-typed computed name resolves to its VALUE, so two
/// members with the same const name DO collide — Go reports
/// An_object_literal_cannot_have_multiple_properties_with_the_same_name
/// (TS1117) for property assignments.
#[test]
fn const_literal_computed_name_duplicate_reports_pin() {
    let (mut c, file) = check_file("const x = \"hello\";\nvar v = {\n    [x]: 1,\n    [x]: 2,\n}\n");
    let codes = diag_codes(&mut c, file);
    assert!(
        codes.contains(&1117),
        "const-literal computed-name duplicate must report TS1117: {codes:?}"
    );
}

/// Same REAL path for METHODS: two const-computed-name methods collide on
/// the resolved name → Go's TS2300 (Duplicate_identifier_0) fires.
#[test]
fn const_literal_computed_name_method_duplicate_reports_pin() {
    let (mut c, file) = check_file("const x = \"hello\";\nvar v = {\n    [x]() { },\n    [x]() { },\n}\n");
    let codes = diag_codes(&mut c, file);
    assert!(
        codes.contains(&2300),
        "const-literal computed-name method duplicate must report TS2300: {codes:?}"
    );
}

/// A const NUMERAL computed name resolves too (tryGetNameFromType's number
/// literal arm — the raw payload, no quote wrapping).
#[test]
fn const_number_computed_name_duplicate_reports_pin() {
    let (mut c, file) = check_file("const x = 1;\nvar v = {\n    [x]: 1,\n    [x]: 2,\n}\n");
    let codes = diag_codes(&mut c, file);
    assert!(
        codes.contains(&1117),
        "const-number computed-name duplicate must report TS1117: {codes:?}"
    );
}

//! FCS4 pin (full-corpus check sweep, tranche 4): the
//! X_this_cannot_be_referenced_in_a_computed_property_name (2465) family —
//! es6/computedProperties/computedPropertyNames21_ES5.ts shape. Go's
//! checkFunctionOrMethodDeclaration (checker.go:3450) runs the FULL
//! checkComputedPropertyName on method declarations' computed names (the
//! semantic check that VISITS the name's expression); the port had the
//! grammar-only checkGrammarComputedPropertyName, so `[this.bar()]() {}`
//! inside a class was never semantically checked and the 2465 diagnostic
//! never fired (the Go twin's dump for the case carries it at the `this`).

use tsc_ast::{NodeId, NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::options_generated::CompilerOptions;
use tsc_core::scriptkind::ScriptKind;
use tsc_parser::parse_source_file;

// ── The single-file Program scaffold (the fcs3_panic_families.rs idiom) ──

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

/// computedPropertyNames21_ES5.ts's shape: `this` inside a CLASS method's
/// computed name — Go reports 2465 (checkThisExpression's
/// thisInComputedPropertyName arm, reached because checkFunctionOrMethod
/// Declaration's full checkComputedPropertyName visit drives the `this`
/// through checkThisExpression with the computed property as its this
/// container).
#[test]
fn class_method_computed_name_this_pin() {
    let (mut c, file) = check_file("class C {\n    bar() {\n        return 0;\n    }\n    [this.bar()]() { }\n}\n");
    assert!(diag_codes(&mut c, file).contains(&2465));
}

/// computedPropertyNames26_ES5.ts's shape: `this` in a computed PROPERTY
/// name (the object-binding path also runs through the same
/// checkComputedPropertyName visit).
#[test]
fn class_property_computed_name_this_pin() {
    let (mut c, file) = check_file("class C {\n    [this.bar]() { }\n    bar() { return 0; }\n}\n");
    assert!(diag_codes(&mut c, file).contains(&2465));
}

/// The sibling this-in-computed-name grammar rule (computedPropertyNames18
/// shape): `this` in a computed property name of an object LITERAL method
/// is legal — the 2465 must NOT fire (this is not a CLASS computed name;
/// Go's getThisContainer only makes class grandparent computed names
/// 'this' containers).
#[test]
fn object_literal_computed_name_this_ok_pin() {
    let (mut c, file) = check_file("var o = {\n    [this]() { }\n}\n");
    assert!(!diag_codes(&mut c, file).contains(&2465));
}

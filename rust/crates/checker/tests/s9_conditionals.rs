//! M6 S9 conditional-type slice gate: pins that the REAL getConditionalType
//! (typenodes.rs, checker.go 24834) resolves eager conditionals to their
//! branch types, keeps generic conditionals deferred, instantiates through
//! getConditionalTypeInstantiation (uniontypes.rs, checker.go 23010), and
//! distributes distributive conditionals over union check types
//! (checker.go 23031). The `any` union-of-both-branches arm (checker.go
//! 24895) and the immediately-nested-false root switch (checker.go 24897)
//! are pinned end-to-end through the alias machinery.

use tsc_ast::{NodeId, NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::options_generated::CompilerOptions;
use tsc_core::scriptkind::ScriptKind;
use tsc_parser::parse_source_file;

/// The spike.rs harness shape (the §5.5 program seam).
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

/// Parses + binds + checkers `src`; returns (checker, the binder's top-level
/// name→symbol pairs, file NodeId).
fn check_file_with_symbols(
    src: &str,
) -> (Checker, Vec<(String, SymbolId)>, NodeId) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let symbols: Vec<(String, SymbolId)> = bind
        .symbols
        .iter()
        .enumerate()
        .map(|(i, s)| (s.name.clone(), SymbolId(i as u32)))
        .filter(|(name, _)| !name.is_empty() && !name.starts_with('\u{FE}'))
        .collect();
    let program = TestProgram {
        options: CompilerOptions::default(),
        source_file: sf,
        bind,
        binder_symbol_count,
        checker_symbols: Vec::new(),
    };
    let mut checker = new_checker(Box::new(program));
    checker.check_source_file(file, true);
    (checker, symbols, file)
}

fn alias_type(c: &mut Checker, symbols: &[(String, SymbolId)], name: &str) -> tsc_checker::TypeId {
    let symbol = symbols
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, s)| *s)
        .unwrap_or_else(|| panic!("symbol {name} not found"));
    c.get_declared_type_of_symbol(symbol)
}

/// `string extends number ? "t" : "f"` resolves EAGERLY to the false branch
/// (checker.go 24952's instantiateType(falseType, mapper) break).
#[test]
fn s9_eager_conditional_resolves_to_false_branch() {
    let (mut c, symbols, _) =
        check_file_with_symbols(r#"type A = string extends number ? "t" : "f";"#);
    let a = alias_type(&mut c, &symbols, "A");
    assert!(
        c.type_(a).flags.intersects(tsc_checker::TypeFlags::STRING_LITERAL),
        "A resolves to a string literal, got flags {:?}",
        c.type_(a).flags
    );
    assert!(matches!(
        c.type_(a).as_literal_type().and_then(|d| d.value.as_ref()),
        Some(tsc_checker::LiteralValue::String(v)) if v == "f"
    ));
}

/// `string extends string ? "t" : "f"` — the definitely-true restrictive
/// probe (checker.go 24936) takes the true branch.
#[test]
fn s9_eager_conditional_resolves_to_true_branch() {
    let (mut c, symbols, _) =
        check_file_with_symbols(r#"type B = string extends string ? "t" : "f";"#);
    let b = alias_type(&mut c, &symbols, "B");
    assert!(matches!(
        c.type_(b).as_literal_type().and_then(|d| d.value.as_ref()),
        Some(tsc_checker::LiteralValue::String(v)) if v == "t"
    ));
}

/// `any extends string ? "t" : "f"` — the any probe returns the UNION of
/// both branches (checker.go 24895's extraTypes arm).
#[test]
fn s9_any_check_type_returns_the_union_of_both_branches() {
    let (mut c, symbols, _) =
        check_file_with_symbols(r#"type G = any extends string ? "t" : "f";"#);
    let g = alias_type(&mut c, &symbols, "G");
    assert!(
        c.type_(g).flags.intersects(tsc_checker::TypeFlags::UNION),
        "the any check type yields the union of both branches, got flags {:?}",
        c.type_(g).flags
    );
    assert_eq!(c.type_(g).types().len(), 2);
}

/// `type C<T> = T extends string ? "y" : "n"` with no type arguments stays
/// DEFERRED: the declared type is the conditional type itself.
#[test]
fn s9_generic_conditional_stays_deferred() {
    let (mut c, symbols, _) =
        check_file_with_symbols(r#"type C<T> = T extends string ? "y" : "n";"#);
    let cond = alias_type(&mut c, &symbols, "C");
    assert!(
        c.type_(cond).flags.intersects(tsc_checker::TypeFlags::CONDITIONAL),
        "the generic conditional is deferred, got flags {:?}",
        c.type_(cond).flags
    );
}

/// Instantiating the deferred conditional through the alias machinery
/// (getTypeAliasInstantiation → instantiateType → getConditionalType-
/// Instantiation, checker.go 23010) resolves the branch per type argument.
#[test]
fn s9_conditional_instantiation_resolves_the_branch() {
    let (mut c, symbols, _) = check_file_with_symbols(
        r#"type C<T> = T extends string ? "y" : "n";
type D = C<number>;
type E = C<string>;"#,
    );
    let d = alias_type(&mut c, &symbols, "D");
    assert!(matches!(
        c.type_(d).as_literal_type().and_then(|d| d.value.as_ref()),
        Some(tsc_checker::LiteralValue::String(v)) if v == "n"
    ));
    let e = alias_type(&mut c, &symbols, "E");
    assert!(matches!(
        c.type_(e).as_literal_type().and_then(|d| d.value.as_ref()),
        Some(tsc_checker::LiteralValue::String(v)) if v == "y"
    ));
}

/// A DISTRIBUTIVE conditional over a union check type distributes over the
/// constituents (checker.go 23031's mapType arm): `E<string | number>`
/// resolves to the union `"y" | "n"`.
#[test]
fn s9_distributive_conditional_distributes_over_unions() {
    let (mut c, symbols, _) = check_file_with_symbols(
        r#"type E<T> = T extends string ? "y" : "n";
type F = E<string | number>;"#,
    );
    let f = alias_type(&mut c, &symbols, "F");
    assert!(
        c.type_(f).flags.intersects(tsc_checker::TypeFlags::UNION),
        "the distributed conditional is a union, got flags {:?}",
        c.type_(f).flags
    );
    assert_eq!(c.type_(f).types().len(), 2);
    let values: Vec<&str> = c
        .type_(f)
        .types()
        .iter()
        .filter_map(|&t| {
            c.type_(t)
                .as_literal_type()
                .and_then(|d| d.value.as_ref())
                .and_then(|v| match v {
                    tsc_checker::LiteralValue::String(s) => Some(s.as_str()),
                    _ => None,
                })
        })
        .collect();
    assert!(values.contains(&"y") && values.contains(&"n"), "{values:?}");
}

/// The immediately-nested-false root switch (checker.go 24997-25001): the
/// outer false branch IS a nested conditional whose parent is the outer
/// conditional node — resolution switches roots and continues in-place.
#[test]
fn s9_nested_false_branch_resolves_in_place() {
    let (mut c, symbols, _) = check_file_with_symbols(
        r#"type H = string extends number ? 1 : string extends number ? 2 : 3;"#,
    );
    let h = alias_type(&mut c, &symbols, "H");
    assert!(matches!(
        c.type_(h).as_literal_type().and_then(|d| d.value.as_ref()),
        Some(tsc_checker::LiteralValue::Number(v)) if *v == 3.0
    ));
}

/// A conditional with an `infer T` arm keeps the constraint-inference
/// context alive (checker.go 24859's newInferenceContext arm) and resolves
/// `string extends infer U ? U : never` to the inferred `string`.
#[test]
fn s9_infer_conditional_resolves_the_inferred_type() {
    let (mut c, symbols, _) = check_file_with_symbols(
        r#"type I = string extends infer U ? U : never;"#,
    );
    let i = alias_type(&mut c, &symbols, "I");
    assert!(
        c.type_(i).flags.intersects(tsc_checker::TypeFlags::STRING),
        "the infer arm resolves to the inferred string, got flags {:?}",
        c.type_(i).flags
    );
}

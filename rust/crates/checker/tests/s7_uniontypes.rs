//! M6 S7 gate (union/type-construction engine, uniontypes.rs): pins for the
//! REAL interned union/intersection constructors, the eagerly-seeded union
//! intrinsics, the literal-type base/regular readers, the template-literal
//! constructor, and the array/tuple reference constructors — the contracts
//! Go's checker.go 25180-26955 + 29590-29919 establish. Where a pin needs a
//! fresh literal type it comes from the checked source (the spike-scaffold
//! idiom), so the pins ride the full check pipeline like Go's tests do.

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::types::{LiteralValue, TypeFlags, TypeId};
use tsc_checker::{is_fresh_literal_type, new_checker, Checker, Program};
use tsc_core::options_generated::CompilerOptions;
use tsc_core::scriptkind::ScriptKind;
use tsc_parser::parse_source_file;

// ── The single-file Program scaffold (same shape as spike.rs) ──

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

fn check(src: &str) -> (Checker, Vec<(String, SymbolId)>) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let mut symbols = Vec::new();
    if let Some(locals) = tsc_binder::node_data::locals_of(&sf, sf.as_node_id()) {
        for (name, id) in locals.entries() {
            symbols.push((name.clone(), *id));
        }
    }
    let binder_symbol_count = bind.symbol_count as u32;
    let program = TestProgram {
        options: CompilerOptions::default(),
        source_file: sf,
        bind,
        binder_symbol_count,
        checker_symbols: Vec::new(),
    };
    (new_checker(Box::new(program)), symbols)
}

fn symbol_named(symbols: &[(String, SymbolId)], name: &str) -> SymbolId {
    symbols
        .iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("no local symbol named {name}"))
        .1
}

fn flags(c: &Checker, t: TypeId) -> TypeFlags {
    c.type_(t).flags
}

fn constituents(c: &Checker, t: TypeId) -> Vec<TypeId> {
    c.type_(t).types().to_vec()
}

// ── The union constructor (Go getUnionType/getUnionTypeEx) ──

#[test]
fn s7_union_interning_is_stable_and_order_independent() {
    let (mut c, _) = check("");
    let (s, n) = (c.string_type(), c.number_type());
    let u1 = c.get_union_type(&[s, n]);
    let u2 = c.get_union_type(&[n, s]);
    // Interned: the same constituent set resolves to the same TypeId.
    assert_eq!(u1, u2);
    assert!(flags(&c, u1).intersects(TypeFlags::UNION));
    let set = constituents(&c, u1);
    assert_eq!(set.len(), 2);
    assert!(set.contains(&s) && set.contains(&n));
}

#[test]
fn s7_union_empty_and_single_constituent_contracts() {
    let (mut c, _) = check("");
    let s = c.string_type();
    assert_eq!(c.get_union_type(&[]), c.never_type());
    assert_eq!(c.get_union_type(&[s]), s);
}

#[test]
fn s7_union_absorbs_never_and_collapses_any_and_unknown() {
    let (mut c, _) = check("");
    let (s, any_t, unknown_t, never_t) =
        (c.string_type(), c.any_type(), c.unknown_type(), c.never_type());
    assert_eq!(c.get_union_type(&[never_t, s]), s);
    assert_eq!(c.get_union_type(&[any_t, s]), any_t);
    assert_eq!(c.get_union_type(&[unknown_t, s]), unknown_t);
}

#[test]
fn s7_boolean_type_is_the_seeded_union_of_regular_literals() {
    let (mut c, _) = check("");
    let boolean = c.boolean_type();
    assert!(flags(&c, boolean).intersects(TypeFlags::BOOLEAN));
    assert!(flags(&c, boolean).intersects(TypeFlags::UNION));
    // Go: booleanType = getUnionType([regularFalseType, regularTrueType]) —
    // and the constructor interning makes the reconstruction identical.
    let (f, t) = (c.regular_false_type(), c.regular_true_type());
    assert_eq!(boolean, c.get_union_type(&[f, t]));
    let set = constituents(&c, boolean);
    assert_eq!(set, vec![f, t]);
}

#[test]
fn s7_union_intrinsics_are_the_eagerly_seeded_unions() {
    let (c, _) = check("");
    // new_checker seeds the four union intrinsics Go constructs in
    // initializeChecker (checker.go:1013-1035 region).
    assert!(flags(&c, c.string_or_number_type()).intersects(TypeFlags::UNION));
    assert!(flags(&c, c.string_number_symbol_type()).intersects(TypeFlags::UNION));
    assert!(flags(&c, c.number_or_bigint_type()).intersects(TypeFlags::UNION));
    // numericStringType is the `${number}` template-literal type.
    assert!(flags(&c, c.numeric_string_type()).intersects(TypeFlags::TEMPLATE_LITERAL));
    // templateConstraintType: string | number | symbol | bigint? Go's
    // template-constraint domain union (all unions here).
    assert!(flags(&c, c.template_constraint_type()).intersects(TypeFlags::UNION));
}

// ── The intersection constructor (Go getIntersectionType) ──

#[test]
fn s7_intersection_empty_single_and_dedupe_contracts() {
    let (mut c, _) = check("");
    let s = c.string_type();
    assert_eq!(c.get_intersection_type(&[]), c.unknown_type());
    assert_eq!(c.get_intersection_type(&[s]), s);
    // Duplicate constituents collapse (the insertType discipline).
    assert_eq!(c.get_intersection_type(&[s, s]), s);
}

#[test]
fn s7_intersection_of_disjoint_primitives_reduces_to_never() {
    let (mut c, _) = check("");
    let (s, n) = (c.string_type(), c.number_type());
    let never = c.get_intersection_type(&[s, n]);
    assert!(flags(&c, never).intersects(TypeFlags::NEVER));
}

#[test]
fn s7_intersection_with_any_and_unknown() {
    let (mut c, _) = check("");
    let s = c.string_type();
    // Go's getIntersectionTypeWorker: any collapses the intersection to
    // anyType; unknown is dropped from the constituent set (addTypeToIntersection
    // skips any/unknown types), so `unknown & string` reduces to `string`.
    assert_eq!(c.get_intersection_type(&[c.any_type(), s]), c.any_type());
    assert_eq!(c.get_intersection_type(&[c.unknown_type(), s]), s);
}

// ── Literal-type readers (Go getBaseTypeOfLiteralType / getRegularTypeOfLiteralType) ──

#[test]
fn s7_base_type_of_literal_types() {
    let (mut c, symbols) = check("const s = \"hi\"; const n = 1;\n");
    let s_type = c.get_type_of_symbol(symbol_named(&symbols, "s"));
    assert!(flags(&c, s_type).intersects(TypeFlags::STRING_LITERAL));
    assert_eq!(c.get_base_type_of_literal_type(s_type), c.string_type());
    let n_type = c.get_type_of_symbol(symbol_named(&symbols, "n"));
    assert!(flags(&c, n_type).intersects(TypeFlags::NUMBER_LITERAL));
    assert_eq!(c.get_base_type_of_literal_type(n_type), c.number_type());
}

#[test]
fn s7_regular_type_of_fresh_literal_is_the_non_fresh_form() {
    let (mut c, symbols) = check("const x = \"hi\";\n");
    let x_type = c.get_type_of_symbol(symbol_named(&symbols, "x"));
    assert!(is_fresh_literal_type(&c, x_type));
    let regular = c.get_regular_type_of_literal_type(x_type);
    // The regular form is a distinct interned literal with the same value
    // and without the fresh marker.
    assert_ne!(regular, x_type);
    assert!(!is_fresh_literal_type(&c, regular));
    assert!(flags(&c, regular).intersects(TypeFlags::STRING_LITERAL));
    assert!(matches!(
        c.type_(regular).as_literal_type().and_then(|d| d.value.as_ref()),
        Some(LiteralValue::String(v)) if v == "hi"
    ));
    assert_eq!(c.get_regular_type_of_literal_type(regular), regular);
}

// ── Template-literal construction (Go getTemplateLiteralType) ──

#[test]
fn s7_template_literal_type_interns_by_texts_and_types() {
    let (mut c, _) = check("");
    let texts = vec!["a".to_string(), "b".to_string()];
    let (empty, s) = {
        let v: Vec<TypeId> = Vec::new();
        (v, c.string_type())
    };
    let _ = empty;
    let t1 = c.get_template_literal_type(&texts, &[s]);
    assert!(flags(&c, t1).intersects(TypeFlags::TEMPLATE_LITERAL));
    // Interning: the same shape reconstructs the identical type.
    assert_eq!(t1, c.get_template_literal_type(&texts.clone(), &[s]));
    // A different shape is a distinct type.
    let t2 = c.get_template_literal_type(&texts, &[c.number_type()]);
    assert_ne!(t1, t2);
}

// ── Array/tuple reference construction (Go createArrayType/createTupleType) ──

#[test]
fn s7_create_type_reference_interns_against_the_interface_target() {
    let (mut c, symbols) = check("interface I<T> {}\n");
    let i = symbol_named(&symbols, "I");
    let i_type = c.get_declared_type_of_symbol(i);
    assert!(c.type_(i_type).object_flags.intersects(tsc_checker::types::ObjectFlags::INTERFACE));
    let s = c.string_type();
    let reference = c.create_type_reference(i_type, vec![s]);
    assert!(flags(&c, reference).intersects(TypeFlags::OBJECT));
    assert!(c
        .type_(reference)
        .object_flags
        .intersects(tsc_checker::types::ObjectFlags::REFERENCE));
    let args = c
        .type_(reference)
        .as_type_reference()
        .expect("TypeReference data")
        .resolved_type_arguments
        .clone();
    assert_eq!(args, vec![s]);
    // The instantiations-table interning: same target+args → same type.
    assert_eq!(reference, c.create_type_reference(i_type, vec![s]));
    // A different argument list is a distinct instantiation.
    let other = c.create_type_reference(i_type, vec![c.number_type()]);
    assert_ne!(reference, other);
}

#[test]
fn s7_create_array_type_falls_back_without_globals_in_the_bare_scaffold() {
    // The bare single-file scaffold has no lib files, so globalArrayType is
    // the empty generic type and createTypeFromGenericGlobalType answers
    // emptyObjectType (Go's own arm when the global is the empty generic).
    // The interning contract itself is pinned by the interface test above;
    // this pin records the global-less scaffold answer.
    let (mut c, _) = check("");
    let arr = c.create_array_type(c.string_type());
    assert!(flags(&c, arr).intersects(TypeFlags::OBJECT));
}

// ── getReducedType (identity outside reducible intersections) ──

#[test]
fn s7_get_reduced_type_is_identity_for_non_reducible_types() {
    let (mut c, _) = check("");
    let s = c.string_type();
    assert_eq!(c.get_reduced_type(s), s);
    let u = c.get_union_type(&[s, c.number_type()]);
    // A plain primitive union is not a reducible intersection.
    assert_eq!(c.get_reduced_type(u), u);
}

// ── End-to-end: the widening path rides the real union constructor ──

#[test]
fn s7_boolean_widening_resolves_the_boolean_union_intrinsic() {
    let (mut c, symbols) = check("let t = true;\n");
    let t = symbol_named(&symbols, "t");
    let t_type = c.get_type_of_symbol(t);
    // Go: getWidenedLiteralType's boolean arm → booleanType (the seeded
    // union of the regular boolean literals).
    assert_eq!(t_type, c.boolean_type());
    assert!(flags(&c, t_type).intersects(TypeFlags::BOOLEAN | TypeFlags::UNION));
}

//! M6 S8 gate (structured-members machinery, members.rs): pins for the REAL
//! deferred-members protocol — resolveStructuredTypeMembers over the
//! anonymous/object-type/union/intersection dispatch, the
//! getPropertyOfType/getPropertiesOfType family, getSignaturesOfType +
//! the return-type resolution, inherited members (interface extends,
//! class base), and the apparent-type wrapper mapping. Where a pin needs a
//! checked type it comes from the full check pipeline (the spike-scaffold
//! idiom — same shape as s6_resolution.rs / s7_uniontypes.rs), so the pins
//! ride the real member resolution exactly like Go's own tests do.

use tsc_ast::{NodeId, NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::types::{SignatureKind, TypeFlags};
use tsc_checker::{new_checker, Checker, Program};
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

/// Check `src`, returning the checker (already through check_source_file),
/// the source-file node id, and the source-file locals' (name, symbol)
/// rows (collected before the source file moves into the program seam).
fn check_file(src: &str) -> (Checker, NodeId, Vec<(String, SymbolId)>) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let mut locals_rows = Vec::new();
    if let Some(locals) = tsc_binder::node_data::locals_of(&sf, file) {
        for (name, id) in locals.entries() {
            locals_rows.push((name.clone(), *id));
        }
    }
    let program = TestProgram {
        options: CompilerOptions::default(),
        source_file: sf,
        bind,
        binder_symbol_count,
        checker_symbols: Vec::new(),
    };
    let mut checker = new_checker(Box::new(program));
    checker.check_source_file(file, true);
    (checker, file, locals_rows)
}

fn symbol_named(rows: &[(String, SymbolId)], name: &str) -> SymbolId {
    rows.iter()
        .find(|(n, _)| n == name)
        .map(|(_, s)| *s)
        .unwrap_or_else(|| panic!("no file-local symbol named {name}"))
}

fn diag_codes(c: &mut Checker, file: NodeId) -> Vec<i32> {
    c.get_file_diagnostics(file)
        .iter()
        .map(|d| d.code())
        .collect()
}

fn prop_names(c: &mut Checker, t: tsc_checker::types::TypeId) -> Vec<String> {
    let mut names: Vec<String> = c
        .get_properties_of_type(t)
        .iter()
        .map(|&p| {
            let n = c.get_fully_qualified_name(p);
            // get_fully_qualified_name yields container-qualified names
            // ("þobject.x", "I.a", "B.m"); member tests compare the last segment.
            n.rsplit('.').next().unwrap_or(&n).to_string()
        })
        .collect();
    names.sort();
    names
}

// ── Anonymous (object-literal) member resolution ────────────────────────────

/// `let o = { x: 1, y: "s" };` — Go: clean. resolveAnonymousTypeMembers
/// resolves the type-literal symbol's members; getPropertiesOfType lists
/// x and y; the property types are the widened number/string (the fresh
/// literal widening runs before the member table is read).
#[test]
fn s8_object_literal_members_resolve_with_widened_types() {
    let (mut c, file, rows) = check_file("let o = { x: 1, y: \"s\" };\n");
    assert!(diag_codes(&mut c, file).is_empty());
    let o = c.get_type_of_symbol(symbol_named(&rows, "o"));
    assert_eq!(prop_names(&mut c, o), vec!["x", "y"]);
    let x_type = c.get_type_of_property_of_type(o, "x").expect("x resolves");
    assert!(
        c.type_(x_type).flags.intersects(TypeFlags::NUMBER),
        "the widened property type of x is number"
    );
    let y_type = c.get_type_of_property_of_type(o, "y").expect("y resolves");
    assert!(
        c.type_(y_type).flags.intersects(TypeFlags::STRING),
        "the widened property type of y is string"
    );
}

/// `let o = { x: 1 }; o.y;` — Go: TS2339 (Property 'y' does not exist on
/// type '{ x: number; }'). The real getPropertyOfType drives the
/// property-access error ladder.
#[test]
fn s8_missing_property_on_object_literal_reports_2339() {
    let (mut c, file, _rows) = check_file("let o = { x: 1 };\no.y;\n");
    assert_eq!(diag_codes(&mut c, file), vec![2339]);
}

/// `let o = { x: 1 }; o.x;` — Go: clean (the property exists).
#[test]
fn s8_present_property_access_is_clean() {
    let (mut c, file, _rows) = check_file("let o = { x: 1 };\no.x;\n");
    assert!(diag_codes(&mut c, file).is_empty());
}

// ── Signature resolution ────────────────────────────────────────────────────

/// `function g(a: number): string { return ""; }` — Go: clean. The
/// function type's members carry exactly one call signature whose return
/// type is string (the annotated return).
#[test]
fn s8_function_type_resolves_its_call_signature() {
    let (mut c, file, rows) = check_file("function g(a: number): string { return \"\"; }\n");
    assert!(diag_codes(&mut c, file).is_empty());
    let g_type = c.get_type_of_symbol(symbol_named(&rows, "g"));
    let signatures = c.get_signatures_of_type(g_type, SignatureKind::Call);
    assert_eq!(signatures.len(), 1, "one call signature");
    let return_type = c.get_return_type_of_signature(signatures[0]);
    assert!(
        c.type_(return_type).flags.intersects(TypeFlags::STRING),
        "the annotated string return"
    );
    assert_eq!(
        c.get_signatures_of_type(g_type, SignatureKind::Construct).len(),
        0
    );
}

/// `function f() {}` — Go: clean; the inferred return type of an empty body
/// is void (getReturnTypeFromBody's no-context answer, checker.go:20572).
#[test]
fn s8_empty_function_body_infers_void_return() {
    let (mut c, file, rows) = check_file("function f() {}\n");
    assert!(diag_codes(&mut c, file).is_empty());
    let f_type = c.get_type_of_symbol(symbol_named(&rows, "f"));
    let signatures = c.get_signatures_of_type(f_type, SignatureKind::Call);
    assert_eq!(signatures.len(), 1);
    let return_type = c.get_return_type_of_signature(signatures[0]);
    assert!(
        c.type_(return_type).flags.intersects(TypeFlags::VOID),
        "empty body infers void"
    );
}

// ── Inherited members (interface extends / class base) ─────────────────────

/// `interface I { a: number } interface J extends I { b: string }` — Go:
/// clean. J's members table carries both the declared b and the inherited
/// a (resolveObjectTypeMembers' base-type walk).
#[test]
fn s8_interface_extends_inherits_base_members() {
    let (mut c, file, rows) = check_file(
        "interface I { a: number }\ninterface J extends I { b: string }\n",
    );
    assert!(diag_codes(&mut c, file).is_empty());
    let j_type = c.get_declared_type_of_symbol(symbol_named(&rows, "J"));
    assert_eq!(prop_names(&mut c, j_type), vec!["a", "b"], "J carries I's a plus its own b");
    let bases = c.get_base_types(j_type);
    assert_eq!(bases.len(), 1, "one extends clause");
}

/// `class B { m() {} } class D extends B {}` — Go: clean. D's instance
/// members carry the inherited m, and D's static (constructor) type has
/// exactly one construct signature whose return type is D's instance type.
#[test]
fn s8_derived_class_inherits_base_members_and_construct_signature() {
    let (mut c, file, rows) = check_file("class B { m() {} }\nclass D extends B {}\n");
    assert!(diag_codes(&mut c, file).is_empty());
    let d = symbol_named(&rows, "D");
    let d_instance = c.get_declared_type_of_symbol(d);
    assert_eq!(prop_names(&mut c, d_instance), vec!["m"], "D inherits B's m");
    let d_static = c.get_type_of_symbol(d);
    let construct = c.get_signatures_of_type(d_static, SignatureKind::Construct);
    assert_eq!(construct.len(), 1, "the synthesized default constructor");
    let return_type = c.get_return_type_of_signature(construct[0]);
    assert_eq!(return_type, d_instance, "new D() returns the instance type");
}

// ── Union members ──────────────────────────────────────────────────────────

/// `declare let u: { a: number } | { a: string };` — Go: clean. The union's
/// synthetic property a resolves on both constituents (the
/// createUnionOrIntersectionProperty combiner) and its type is the union
/// number | string.
#[test]
fn s8_union_property_combines_constituents() {
    let (mut c, file, rows) = check_file("declare let u: { a: number } | { a: string };\nu.a;\n");
    assert!(diag_codes(&mut c, file).is_empty());
    let u_type = c.get_type_of_symbol(symbol_named(&rows, "u"));
    assert!(
        c.type_(u_type).flags.intersects(TypeFlags::UNION),
        "the annotation resolves to a real union"
    );
    let a_type = c
        .get_type_of_property_of_type(u_type, "a")
        .expect("union property a resolves");
    assert!(
        c.type_(a_type).flags.intersects(TypeFlags::UNION),
        "the union property type is number | string"
    );
}

/// `declare let u: { a: number } | { b: string }; u.a;` — Go: TS2339 (the
/// property exists on no constituent).
#[test]
fn s8_union_property_missing_on_all_constituents_reports_2339() {
    let (mut c, file, _rows) = check_file("declare let u: { a: number } | { b: string };\nu.a;\n");
    assert_eq!(diag_codes(&mut c, file), vec![2339]);
}

// ── Apparent types ──────────────────────────────────────────────────────────

/// Go's getApparentType: the string-like primitives map to the global
/// String wrapper object type (checker.go:22154).
#[test]
fn s8_apparent_type_of_string_is_the_global_string_wrapper() {
    let (mut c, _file, _rows) = check_file("declare const s: string;\n");
    let t = c.string_type();
    let apparent = c.get_apparent_type(t);
    assert!(
        c.type_(apparent).flags.intersects(TypeFlags::OBJECT),
        "the String wrapper is an object type"
    );
    assert_ne!(apparent, t, "the wrapper differs from the primitive");
}

/// The apparent type of an object type is the type itself (Go's
/// `otherwise the type itself` arm).
#[test]
fn s8_apparent_type_of_object_type_is_identity() {
    let (mut c, _file, rows) = check_file("let o = { x: 1 };\n");
    let o_type = c.get_type_of_symbol(symbol_named(&rows, "o"));
    assert_eq!(c.get_apparent_type(o_type), o_type);
}

// ── Type parameters + generics (the base-constraint arm) ───────────────────

/// `function id<T>(x: T): T { return x; } id(1);` — Go: clean: the generic
/// call resolves through the real signature machinery.
#[test]
fn s8_generic_call_resolves_through_signature_machinery() {
    let (mut c, file, _rows) = check_file("function id<T>(x: T): T { return x; }\nid(1);\n");
    assert!(diag_codes(&mut c, file).is_empty());
}

// ── Enums (the getDeclaredTypeOfEnum union) ─────────────────────────────────

/// `enum E { A, B }` — Go: clean. The enum's declared type is
/// getUnionTypeEx(memberTypes, Literal) with TypeFlagsEnumLiteral set and
/// the enum symbol attached (checker.go:24409); the members A and B resolve
/// through the symbol table, and their types are enum literal (number-like)
/// types.
#[test]
fn s8_enum_declared_type_is_enum_literal_union() {
    let (mut c, file, rows) = check_file("enum E { A, B }\nE.A;\n");
    assert!(diag_codes(&mut c, file).is_empty());
    let e_type = c.get_declared_type_of_symbol(symbol_named(&rows, "E"));
    assert!(
        c.type_(e_type).flags.intersects(TypeFlags::ENUM_LITERAL),
        "the declared type carries TypeFlagsEnumLiteral"
    );
    let constituents = c.type_(e_type).types().to_vec();
    assert_eq!(constituents.len(), 2, "A and B constituents");
    for &m in &constituents {
        assert!(
            c.type_(m).flags.intersects(TypeFlags::ENUM_LITERAL),
            "each enum member's type is an enum literal type"
        );
    }
}

//! M5 checker-spike gate (SPEC §8 M5): the `getTypeOfSymbol` demo.
//!
//! The gate text says "`getTypeOfSymbol` on `let x = 1` infers `1`". Go/TS
//! semantics: `getTypeOfSymbol` *widens* fresh literals for mutable bindings
//! (`let x = 1` → `number`) and preserves them for const/readonly bindings
//! (`const x = 1` → fresh literal `1`) — that is exactly what the ported
//! `getTypeOfSymbol` chain does below (the widening happens in
//! `getWidenedLiteralTypeForInitializer`, driven by the binder-set
//! `NodeFlagsConstant`). The literal-inference proof is therefore asserted on
//! all three of: the const declaration (the fresh literal `1`), the
//! initializer *expression's* cached type for the `let` declaration (the
//! fresh literal `1` before widening), and the widened `let` result
//! (`number`), which together pin every step of the chain the gate names.

use tsc_ast::{Kind, NodeId, NodeStore, SourceFile, Symbol, SymbolFlags, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::types::{LiteralValue, TypeFlags, TypeId};
use tsc_checker::{is_fresh_literal_type, new_checker, Checker, Program};
use tsc_core::options_generated::CompilerOptions;
use tsc_core::tristate::Tristate;
use tsc_core::scriptkind::ScriptKind;
use tsc_parser::parse_source_file;

/// The single-file `Program` impl (binder smoke-test idioms): owns the parsed
/// source file + the binder's symbol arena, and extends it with
/// checker-created symbols through the §5.5 seam.
struct TestProgram {
    options: CompilerOptions,
    source_file: SourceFile,
    bind: BindResult,
    /// Base of the program-level SymbolId space: ids below
    /// `binder_symbol_count` resolve into the binder's arena, ids above it
    /// into the checker-created tail this impl appends.
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
    fn bind_source_files(&mut self) {
        // The test binds eagerly; binding twice is refused by the binder
        // (BindOnce), exactly like Go's program does.
    }
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

/// Parses + binds + checkers `src`; returns the checker plus the name→symbol
/// and name→declaration maps for the top-level variables.
type Check = (Checker, Vec<(String, SymbolId)>, Vec<(String, NodeId)>);

fn check(src: &str) -> Check {
    check_with_options(src, CompilerOptions::default())
}

/// The same, with explicit options (Go's `NewChecker` derives the strict*
/// booleans through `GetStrictOptionValue`).
fn check_with_options(src: &str, options: CompilerOptions) -> Check {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let bind = bind_source_file(&mut sf).expect("bind produced a result");

    // Snapshot the top-level locals before the file moves into the program.
    let mut symbols = Vec::new();
    if let Some(locals) = tsc_binder::node_data::locals_of(&sf, sf.as_node_id()) {
        for (name, id) in locals.entries() {
            symbols.push((name.clone(), *id));
        }
    }
    let mut declarations = Vec::new();
    if let Some(statements) = sf.payload().statements.clone() {
        for statement in statements.nodes.iter().copied() {
            let n = sf.node(statement);
            if n.kind != Kind::VariableStatement {
                continue;
            }
            let list = n.as_variable_statement().expect("VariableStatement data").declaration_list;
            let Some(decls) = sf
                .node(list)
                .as_variable_declaration_list()
                .expect("VariableDeclarationList data")
                .declarations
                .clone()
            else {
                continue;
            };
            for decl in decls.nodes.iter().copied() {
                let name_node = sf.node(decl).name().expect("variable name");
                let name = tsc_ast::node_text(&sf, name_node).to_string();
                declarations.push((name, decl));
            }
        }
    }

    let binder_symbol_count = bind.symbol_count as u32;
    let program = TestProgram {
        options,
        source_file: sf,
        bind,
        binder_symbol_count,
        checker_symbols: Vec::new(),
    };
    let checker = new_checker(Box::new(program));
    (checker, symbols, declarations)
}

fn symbol_named(symbols: &[(String, SymbolId)], name: &str) -> SymbolId {
    symbols
        .iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("no local symbol named {name}"))
        .1
}

fn declaration_named(declarations: &[(String, NodeId)], name: &str) -> NodeId {
    declarations
        .iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("no declaration named {name}"))
        .1
}

fn flags(c: &Checker, t: TypeId) -> TypeFlags {
    c.type_(t).flags
}

#[test]
fn gate_demo_let_x_1_infers_the_literal_1() {
    let (mut c, symbols, declarations) = check("const x = 1;\nlet y = 1;\n");

    let x = symbol_named(&symbols, "x");
    let y = symbol_named(&symbols, "y");
    assert!(c.program_symbol(x).flags.intersects(SymbolFlags::BLOCK_SCOPED_VARIABLE));
    assert!(c.program_symbol(y).flags.intersects(SymbolFlags::BLOCK_SCOPED_VARIABLE));

    // ── The literal-inference proof (SPEC §8 M5) ──
    // `const x = 1` → the FRESH literal type `1` (widening is suppressed by
    // the binder-set NodeFlagsConstant).
    let x_type = c.get_type_of_symbol(x);
    assert!(
        flags(&c, x_type).intersects(TypeFlags::NUMBER_LITERAL),
        "const x = 1 must infer literal 1"
    );
    assert!(is_fresh_literal_type(&c, x_type), "the inferred literal is fresh");
    assert!(matches!(
        c.type_(x_type).as_literal_type().and_then(|d| d.value.as_ref()),
        Some(LiteralValue::Number(n)) if *n == tsc_jsnum::Number(1.0)
    ));

    // `let y = 1` → the initializer *expression* checks as the fresh literal
    // `1` (cached in the expression's TypeNodeLinks)...
    let y_initializer = c
        .program_initializer_of(declaration_named(&declarations, "y"))
        .expect("y initializer");
    let initializer_type = c.check_expression_cached(y_initializer);
    assert!(flags(&c, initializer_type).intersects(TypeFlags::NUMBER_LITERAL));
    assert!(is_fresh_literal_type(&c, initializer_type));
    // ...and getTypeOfSymbol widens it to `number` for the mutable binding —
    // Go/TS semantics, preserved exactly by the ported widening chain.
    let y_type = c.get_type_of_symbol(y);
    assert!(flags(&c, y_type).intersects(TypeFlags::NUMBER));
    assert!(!flags(&c, y_type).intersects(TypeFlags::NUMBER_LITERAL));

    // Resolution caching: a second lookup returns the same TypeId.
    assert_eq!(c.get_type_of_symbol(x), x_type);
    assert_eq!(c.get_type_of_symbol(y), y_type);

    // The literal intern table dedupes: both `1` initializers share one
    // regular literal type (Go interns number literal types per value).
    let x_initializer_type = c.check_expression_cached(
        c.program_initializer_of(declaration_named(&declarations, "x"))
            .expect("x initializer"),
    );
    let y_regular = c
        .type_(initializer_type)
        .as_literal_type()
        .and_then(|d| d.regular_type)
        .expect("regular type");
    let x_regular = c
        .type_(x_initializer_type)
        .as_literal_type()
        .and_then(|d| d.regular_type)
        .expect("regular type");
    assert_eq!(x_regular, y_regular);
}

#[test]
fn gate_demo_string_literal() {
    let (mut c, symbols, _) = check("const s = \"hi\";\n");
    let s = symbol_named(&symbols, "s");
    let s_type = c.get_type_of_symbol(s);
    assert!(flags(&c, s_type).intersects(TypeFlags::STRING_LITERAL));
    assert!(is_fresh_literal_type(&c, s_type));
    assert!(matches!(
        c.type_(s_type).as_literal_type().and_then(|d| d.value.as_ref()),
        Some(LiteralValue::String(v)) if v == "hi"
    ));
}

#[test]
fn gate_demo_boolean_literals_widen_at_the_union_boundary() {
    let (mut c, symbols, declarations) = check("const f = false;\nlet t = true;\n");
    // `const f = false` keeps the fresh literal `false`.
    let f = symbol_named(&symbols, "f");
    let f_type = c.get_type_of_symbol(f);
    assert!(flags(&c, f_type).intersects(TypeFlags::BOOLEAN_LITERAL));
    assert!(matches!(
        c.type_(f_type).as_literal_type().and_then(|d| d.value.as_ref()),
        Some(LiteralValue::Boolean(false))
    ));
    // `let t = true`: the initializer checks as the fresh literal `true`;
    // widening it to `boolean` rides the REAL union constructor (M6 S7,
    // uniontypes.rs): the widened type is booleanType, the eagerly-seeded
    // interned union of [regularFalseType, regularTrueType] whose flags
    // carry TypeFlagsBoolean (Go's getWidenedLiteralType boolean arm).
    let t_initializer = c
        .program_initializer_of(declaration_named(&declarations, "t"))
        .expect("t initializer");
    let initializer_type = c.check_expression_cached(t_initializer);
    assert!(flags(&c, initializer_type).intersects(TypeFlags::BOOLEAN_LITERAL));
    assert!(is_fresh_literal_type(&c, initializer_type));
    let t = symbol_named(&symbols, "t");
    let t_type = c.get_type_of_symbol(t);
    assert_eq!(t_type, c.boolean_type());
    assert!(flags(&c, t_type).intersects(TypeFlags::BOOLEAN));
    assert!(flags(&c, t_type).intersects(TypeFlags::UNION));
    // The interned union construction: booleanType is a singleton union.
    assert_eq!(c.boolean_type(), c.boolean_type());
}

#[test]
fn new_checker_seeds_intrinsic_types_in_go_order() {
    let (c, _symbols, _declarations) = check("");
    // Go's NewChecker seeds these intrinsics with these exact names/flags.
    assert!(flags(&c, c.any_type()).intersects(TypeFlags::ANY));
    assert!(flags(&c, c.unknown_type()).intersects(TypeFlags::UNKNOWN));
    assert!(flags(&c, c.string_type()).intersects(TypeFlags::STRING));
    assert!(flags(&c, c.number_type()).intersects(TypeFlags::NUMBER));
    assert!(flags(&c, c.bigint_type()).intersects(TypeFlags::BIGINT));
    assert!(flags(&c, c.void_type()).intersects(TypeFlags::VOID));
    assert!(flags(&c, c.never_type()).intersects(TypeFlags::NEVER));
    assert!(flags(&c, c.null_type()).intersects(TypeFlags::NULL));
    assert!(flags(&c, c.undefined_type()).intersects(TypeFlags::UNDEFINED));
    assert!(flags(&c, c.es_symbol_type()).intersects(TypeFlags::ESSYMBOL));
    assert!(flags(&c, c.non_primitive_type()).intersects(TypeFlags::NON_PRIMITIVE));
    assert!(flags(&c, c.error_type()).intersects(TypeFlags::ANY));
    assert_eq!(
        c.type_(c.number_type()).as_intrinsic_type().map(|d| d.intrinsic_name.as_str()),
        Some("number")
    );
    assert_eq!(
        c.type_(c.error_type()).as_intrinsic_type().map(|d| d.intrinsic_name.as_str()),
        Some("error")
    );
    // zeroType / emptyStringType are interned literal types.
    assert!(flags(&c, c.zero_type()).intersects(TypeFlags::NUMBER_LITERAL));
    assert!(flags(&c, c.empty_string_type()).intersects(TypeFlags::STRING_LITERAL));
    // The widening pair under DEFAULT options: StrictNullChecks is
    // TSUnknown and GetStrictOptionValue falls back to Strict (!= TSFalse),
    // which is also TSUnknown — so strictNullChecks is TRUE and
    // createWideningType returns the SAME type (Go parity with
    // `&CompilerOptions{}`).
    assert_eq!(c.undefined_type(), c.undefined_widening_type());
    assert_eq!(c.null_type(), c.null_widening_type());
    // Ids are 1-based creation counters (Go: TypeId(TypeCount)) and NewChecker
    // creates them in Go's exact order (any, auto, wildcard, blockedString...).
    assert_eq!(c.any_type().0, 1);
    assert_eq!(c.auto_type().0, 2);
    assert_eq!(c.wildcard_type().0, 3);
    assert_eq!(c.blocked_string_type().0, 4);
    assert_eq!(c.error_type().0, 5);
    // The distinct-widening branch lives in its own test (options with
    // Strict = TSFalse).
}

#[test]
fn widening_pairs_are_distinct_without_strict_null_checks() {
    // Go: createWideningType returns nonWideningType unchanged under
    // strictNullChecks; otherwise it mints a fresh intrinsic type carrying
    // ObjectFlagsContainsWideningType.
    let options = CompilerOptions {
        strict: Tristate::TSFalse,
        ..CompilerOptions::default()
    };
    let (c, _symbols, _declarations) = check_with_options("", options);
    assert_ne!(c.undefined_type(), c.undefined_widening_type());
    assert_ne!(c.null_type(), c.null_widening_type());
    use tsc_checker::types::ObjectFlags;
    assert!(c
        .type_(c.undefined_widening_type())
        .object_flags
        .intersects(ObjectFlags::CONTAINS_WIDENING_TYPE));
}

#[test]
fn checker_created_symbols_share_the_program_id_space() {
    // §5.5 proof: new_symbol allocates from the program-level arena, so the
    // id is distinct from every binder id and resolvable through the seam.
    let (c, symbols, _) = check("let a = 1;\n");
    let binder_max = symbols.iter().map(|(_, s)| s.0).max().unwrap();
    let undefined_symbol = c.undefined_symbol();
    assert!(
        undefined_symbol.0 > binder_max,
        "checker symbols extend the program id space"
    );
    let s = c.program_symbol(undefined_symbol);
    assert_eq!(s.name, "undefined");
    assert!(s.flags.intersects(SymbolFlags::PROPERTY | SymbolFlags::TRANSIENT));
}

#[test]
fn type_resolution_stack_detects_no_cycle_for_simple_declarations() {
    // The push/pop pair around every getTypeOfVariableOrParameterOrProperty
    // resolution must balance (Go's cycle detector).
    let (mut c, symbols, _) = check("const x = 1;\n");
    let x = symbol_named(&symbols, "x");
    let t = c.get_type_of_symbol(x);
    assert!(c.type_resolutions_are_balanced());
    assert!(flags(&c, t).intersects(TypeFlags::NUMBER_LITERAL));
}

// ────────────────────────────────────────────────────────────────────────────
// M6 S2 gate demos — the declared-type family + the enum machinery
// (typeofsymbol.rs)
// ────────────────────────────────────────────────────────────────────────────

#[test]
fn gate_demo_class_declared_type_carries_the_this_type_promotion() {
    use tsc_checker::types::ObjectFlags;
    let (mut c, symbols, _) = check("class C {}\n");
    let class = symbol_named(&symbols, "C");
    assert!(c.program_symbol(class).flags.intersects(SymbolFlags::CLASS));
    // Go: getDeclaredTypeOfClassOrInterface — a class always gets a "this"
    // type (ObjectFlagsReference + the thisType parameter whose constraint
    // is the class itself), and the identity instantiation seeds the
    // instantiations table.
    let declared = c.get_declared_type_of_symbol(class);
    assert!(c.type_(declared).object_flags.intersects(ObjectFlags::CLASS));
    assert!(c.type_(declared).object_flags.intersects(ObjectFlags::REFERENCE));
    let data = c.type_(declared).as_interface_type().expect("InterfaceType data");
    let this_type = data.this_type.expect("classes always carry a thisType");
    assert!(flags(&c, this_type).intersects(TypeFlags::TYPE_PARAMETER));
    assert!(
        c.type_(this_type)
            .as_type_parameter()
            .expect("TypeParameter data")
            .is_this_type
    );
    assert_eq!(
        c.type_(this_type)
            .as_type_parameter()
            .expect("TypeParameter data")
            .constraint,
        Some(declared)
    );
    assert_eq!(data.all_type_parameters, vec![this_type]);
    assert_eq!(data.outer_type_parameter_count, 0);
    assert_eq!(data.base.resolved_type_arguments, vec![this_type]);
    let instantiations = data.base.base.instantiations.as_ref().expect("instantiations");
    assert_eq!(instantiations.len(), 1);
    assert!(instantiations.values().all(|&v| v == declared));
    // Cache: a second resolution returns the same TypeId.
    assert_eq!(c.get_declared_type_of_symbol(class), declared);
}

#[test]
fn gate_demo_thisless_interface_has_no_this_type() {
    use tsc_checker::types::ObjectFlags;
    let (mut c, symbols, _) = check("interface I { m(): void }\n");
    let interface = symbol_named(&symbols, "I");
    assert!(c.program_symbol(interface).flags.intersects(SymbolFlags::INTERFACE));
    // Go: isThislessInterface — a this-free body with no base types means
    // no "this" type, so the interface declared type is NOT generic.
    let declared = c.get_declared_type_of_symbol(interface);
    assert!(c.type_(declared).object_flags.intersects(ObjectFlags::INTERFACE));
    assert!(!c.type_(declared).object_flags.intersects(ObjectFlags::REFERENCE));
    let data = c.type_(declared).as_interface_type().expect("InterfaceType data");
    assert!(data.this_type.is_none());
}

#[test]
fn gate_demo_enum_member_declared_types_route_through_the_s7_union_seam() {
    // The Go port's getDeclaredTypeOfEnumMember (checker.go:24461) resolves
    // each member's declared type to the PARENT ENUM's declared type, and
    // getDeclaredTypeOfEnum is the REAL interned union of the member types
    // (M6 S7, uniontypes.rs — the union carries the ENUM_LITERAL flags of
    // its constituents and stays a singleton for the same member list).
    let (mut c, symbols, _) = check("enum E { A, B = 2, S = \"x\" }\n");
    let e = symbol_named(&symbols, "E");
    assert!(c.program_symbol(e).flags.intersects(SymbolFlags::ENUM));
    let exports = c.program_symbol(e).exports.clone();
    let member_named = |name: &str| -> SymbolId {
        *exports
            .entries()
            .find(|(n, _)| n.as_str() == name)
            .unwrap_or_else(|| panic!("no enum member named {name}"))
            .1
    };
    let (a, b, s) = (member_named("A"), member_named("B"), member_named("S"));
    for member in [a, b, s] {
        assert!(c.program_symbol(member).flags.intersects(SymbolFlags::ENUM_MEMBER));
    }
    let e_type = c.get_declared_type_of_symbol(e);
    assert!(flags(&c, e_type).intersects(TypeFlags::UNION));
    assert!(flags(&c, e_type).intersects(TypeFlags::ENUM_LITERAL));
    // The interned enum union is stable across repeated resolution.
    assert_eq!(e_type, c.get_declared_type_of_symbol(e));
    // Go: getDeclaredTypeOfEnum seeds each member's declared type with the
    // FRESH form of its interned enum-literal type (computeEnumMemberValues
    // ran: A auto-increments to 0, B is the literal 2, S is the string "x");
    // getDeclaredTypeOfEnumMember then just reads that cached link.
    let a_type = c.get_declared_type_of_symbol(a);
    assert!(flags(&c, a_type).intersects(TypeFlags::ENUM_LITERAL | TypeFlags::NUMBER_LITERAL));
    assert!(matches!(
        c.type_(a_type).as_literal_type().and_then(|d| d.value.as_ref()),
        Some(LiteralValue::Number(n)) if *n == tsc_jsnum::Number(0.0)
    ));
    let b_type = c.get_declared_type_of_symbol(b);
    assert!(flags(&c, b_type).intersects(TypeFlags::ENUM_LITERAL | TypeFlags::NUMBER_LITERAL));
    assert!(matches!(
        c.type_(b_type).as_literal_type().and_then(|d| d.value.as_ref()),
        Some(LiteralValue::Number(n)) if *n == tsc_jsnum::Number(2.0)
    ));
    let s_type = c.get_declared_type_of_symbol(s);
    assert!(flags(&c, s_type).intersects(TypeFlags::ENUM_LITERAL | TypeFlags::STRING_LITERAL));
    assert!(matches!(
        c.type_(s_type).as_literal_type().and_then(|d| d.value.as_ref()),
        Some(LiteralValue::String(v)) if v == "x"
    ));
    // Resolution caching: repeated lookups are stable.
    assert_eq!(c.get_declared_type_of_symbol(e), e_type);
    assert_eq!(c.get_declared_type_of_symbol(a), a_type);
    assert!(c.type_resolutions_are_balanced());
}

#[test]
fn gate_demo_type_alias_declared_type_resolves_through_the_type_node_family() {
    // Go: getDeclaredTypeOfTypeAlias pushes DeclaredType on the resolution
    // stack and resolves the type node. M6 S5 landed the real
    // getTypeFromTypeNode family, so `type A = string` now resolves to the
    // string type (the W1 errorType pin is superseded; resolution caching
    // and the stack balance stay asserted).
    let (mut c, symbols, _) = check("type A = string;\n");
    let a = symbol_named(&symbols, "A");
    assert!(c.program_symbol(a).flags.intersects(SymbolFlags::TYPE_ALIAS));
    let declared = c.get_declared_type_of_symbol(a);
    assert!(flags(&c, declared).intersects(TypeFlags::STRING));
    assert_eq!(declared, c.string_type());
    assert!(c.type_resolutions_are_balanced());
    assert_eq!(c.get_declared_type_of_symbol(a), declared);
}

#[test]
fn gate_demo_class_value_type_is_the_anonymous_object_type() {
    // Go: getTypeOfFuncClassEnumModule — a class's VALUE type is the
    // anonymous object type (not the declared type); no base type variable
    // means no intersection construction.
    use tsc_checker::types::ObjectFlags;
    let (mut c, symbols, _) = check("class C {}\n");
    let class = symbol_named(&symbols, "C");
    let value_type = c.get_type_of_symbol(class);
    assert!(flags(&c, value_type).intersects(TypeFlags::OBJECT));
    assert!(c.type_(value_type).object_flags.intersects(ObjectFlags::ANONYMOUS));
    // The declared type (the thisType-carrying class type) is distinct.
    let declared = c.get_declared_type_of_symbol(class);
    assert_ne!(value_type, declared);
    assert!(c.type_resolutions_are_balanced());
}

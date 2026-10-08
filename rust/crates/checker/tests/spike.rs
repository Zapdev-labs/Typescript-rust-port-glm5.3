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
use tsc_core::compileroptions::CompilerOptions;
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
    fn symbol(&self, id: SymbolId) -> &Symbol {
        if (id.0 as u32) < self.binder_symbol_count {
            &self.bind.symbols[id.0 as usize]
        } else {
            &self.checker_symbols[(id.0 as u32 - self.binder_symbol_count) as usize]
        }
    }
    fn symbol_mut(&mut self, id: SymbolId) -> &mut Symbol {
        if (id.0 as u32) < self.binder_symbol_count {
            &mut self.bind.symbols[id.0 as usize]
        } else {
            let index = (id.0 as u32 - self.binder_symbol_count) as usize;
            &mut self.checker_symbols[index]
        }
    }
    fn alloc_symbol(&mut self, symbol: Symbol) -> SymbolId {
        self.checker_symbols.push(symbol);
        SymbolId((self.binder_symbol_count + self.checker_symbols.len() as u32 - 1) as usize)
    }
}

/// Parses + binds + checkers `src`; returns the checker plus the name→symbol
/// and name→declaration maps for the top-level variables.
fn check(src: &str) -> (Checker, Vec<(String, SymbolId)>, Vec<(String, NodeId)>) {
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
        options: CompilerOptions::default(),
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
    // widening it to `boolean` requires the union constructor (M6), so the
    // spike's boundary is exactly the boolean-union gate.
    let t_initializer = c
        .program_initializer_of(declaration_named(&declarations, "t"))
        .expect("t initializer");
    let initializer_type = c.check_expression_cached(t_initializer);
    assert!(flags(&c, initializer_type).intersects(TypeFlags::BOOLEAN_LITERAL));
    assert!(is_fresh_literal_type(&c, initializer_type));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let t = symbol_named(&symbols, "t");
        c.get_type_of_symbol(t)
    }));
    assert!(
        result.is_err(),
        "let t = true must stop at the M6 union boundary, not return a wrong type"
    );
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
    assert!(flags(&c, c.es_symbol_type()).intersects(TypeFlags::ES_SYMBOL));
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
    // The widening pair behaves per Go: without strictNullChecks
    // createWideningType produces a distinct widening type.
    assert_ne!(c.undefined_type(), c.undefined_widening_type());
    assert_ne!(c.null_type(), c.null_widening_type());
    // Ids are 1-based creation counters (Go: TypeId(TypeCount)) and NewChecker
    // creates them in Go's exact order (any, auto, wildcard, blockedString...).
    assert_eq!(c.any_type().0, 1);
    assert_eq!(c.auto_type().0, 2);
    assert_eq!(c.wildcard_type().0, 3);
    assert_eq!(c.blocked_string_type().0, 4);
    assert_eq!(c.error_type().0, 5);
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

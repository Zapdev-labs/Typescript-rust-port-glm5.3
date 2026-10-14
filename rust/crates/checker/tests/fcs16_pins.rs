//! FCS16 pins (full-corpus check sweep, tranche 16): the
//! `getRegularTypeOfObjectLiteral` context-sensitive-member regularization
//! (checker.go:28693 + transformTypeOfMembers checker.go:28710).
//!
//! ROOT: the port's S7 seam kept a symbol-flags guard plus a delegation to
//! `getRegularTypeOfLiteralType`, which returns object literal types
//! UNCHANGED (their flags are `TypeFlags::OBJECT`, never Freshable/Union) —
//! an identity function. Go builds a NEW anonymous type: the FreshLiteral
//! flag stripped, each property type recursively regularized (nested fresh
//! object literal members collapse to their regular forms; skip-context-
//! sensitive-pass `anyFunctionType` member types pass through verbatim),
//! and the resolved call/construct signatures + index infos carried over.
//! The seam surfaced as +45 TS2345-extra records once tranche 15 unmasked
//! the anyFunctionType wildcard (intraExpressionInferences +24,
//! typeArgumentInferenceWithObjectLiteral +7, symbolProperty22, tagged-
//! template inference churn): consumers of the regular type (structured_
//! type_related_to's union-target arm, resolveCallArguments, the
//! const-assertion comparable check, JSX attributes, flow) kept comparing
//! against the FRESH type.

use tsc_ast::{Kind, NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::types::ObjectFlags;
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::compileroptions::ResolutionModeNone;
use tsc_core::options_generated::CompilerOptions;
use tsc_core::scriptkind::ScriptKind;
use tsc_parser::parse_source_file;
use tsc_tspath::RootedFilePath;

// ── The single-file Program scaffold (the fcs5-fcs15 idiom) ──

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
}

/// Check `src` over the single-file scaffold (no libs — the corpus seam),
/// returning the checker plus the source file's first (outermost, pre-order)
/// `ObjectLiteralExpression` node id, collected before the file moves into
/// the program seam.
fn check_with_object_literal(src: &str) -> (Checker, tsc_ast::NodeId) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
    let literal = tsc_ast::utilities::find_child_node(&sf, file, &mut |n| {
        sf.node(n).kind == Kind::ObjectLiteralExpression
    })
    .expect("the source has an object literal");
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let program = TestProgram {
        options: CompilerOptions::default(),
        name: RootedFilePath::from("/x/unit.ts"),
        source_file: sf,
        bind,
        binder_symbol_count,
        checker_symbols: Vec::new(),
    };
    let mut checker = new_checker(Box::new(program));
    checker.check_source_file(file, false);
    (checker, literal)
}

// ── Pin 1: the regular type is a fresh-stripped NEW type ──

const NESTED_LITERAL: &str = "declare function f(x: { a: { b: number } }): void;\n\
f({ a: { b: 1 } });\n";

#[test]
fn regular_type_strips_fresh_literal_flag() {
    // The fresh object literal type (the argument's node type) carries
    // ObjectLiteral|FreshLiteral; the regular type is a DISTINCT type with
    // FreshLiteral cleared, ObjectLiteral kept, the cache returning the
    // same id on repeat, and non-fresh inputs passing through unchanged
    // (Go's isObjectLiteralType && FreshLiteral guard). The former seam
    // returned the input unchanged in every case.
    let (mut c, literal) = check_with_object_literal(NESTED_LITERAL);
    let fresh = c.get_type_of_expression(literal);
    assert!(
        c.type_(fresh)
            .object_flags
            .intersects(ObjectFlags::OBJECT_LITERAL | ObjectFlags::FRESH_LITERAL),
        "the argument literal's node type is the fresh literal: {:?}",
        c.type_(fresh).object_flags
    );
    let regular = c.get_regular_type_of_object_literal(fresh);
    assert_ne!(
        regular, fresh,
        "the regular type is a new type, not the fresh literal itself"
    );
    assert!(
        !c.type_(regular)
            .object_flags
            .intersects(ObjectFlags::FRESH_LITERAL),
        "FreshLiteral is stripped: {:?}",
        c.type_(regular).object_flags
    );
    assert!(
        c.type_(regular)
            .object_flags
            .intersects(ObjectFlags::OBJECT_LITERAL),
        "ObjectLiteral is kept (Go: resolved.objectFlags &^ FreshLiteral): {:?}",
        c.type_(regular).object_flags
    );
    // Go: the CachedTypeKindRegularObjectLiteral cache — same id on repeat.
    assert_eq!(c.get_regular_type_of_object_literal(fresh), regular);
    // Go: the guard — non-fresh object literal types pass through unchanged.
    assert_eq!(c.get_regular_type_of_object_literal(regular), regular);
}

// ── Pin 2: transformTypeOfMembers — the recursive member walk ──

#[test]
fn regular_type_recursively_regularizes_nested_literals() {
    // transformTypeOfMembers runs getRegularTypeOfObjectLiteral over every
    // property type: the nested `{ b: 1 }` member of the regular type is
    // the nested member's REGULAR type (fresh flag dropped), while the
    // fresh literal's own member table keeps the nested fresh literal.
    let (mut c, literal) = check_with_object_literal(NESTED_LITERAL);
    let fresh = c.get_type_of_expression(literal);
    let regular = c.get_regular_type_of_object_literal(fresh);

    let a_regular = c
        .get_type_of_property_of_type(regular, "a")
        .expect("the regular type has member a");
    let a_fresh = c
        .get_type_of_property_of_type(fresh, "a")
        .expect("the fresh type has member a");
    assert_ne!(a_regular, a_fresh, "the nested literal member is rebound");
    assert!(
        !c.type_(a_regular)
            .object_flags
            .intersects(ObjectFlags::FRESH_LITERAL),
        "the nested fresh literal member is recursively regularized: {:?}",
        c.type_(a_regular).object_flags
    );
    assert!(
        c.type_(a_regular)
            .object_flags
            .intersects(ObjectFlags::OBJECT_LITERAL),
        "the nested member keeps ObjectLiteral: {:?}",
        c.type_(a_regular).object_flags
    );
    // The property SYMBOL itself was rebound via createSymbolWithType
    // (Go: updated != original), so both tables keep their own symbol
    // identity while the member TYPES diverge as pinned above.
    assert_eq!(
        c.get_type_of_property_of_type(regular, "a"),
        Some(a_regular),
        "the regular type's member table is stable"
    );
}

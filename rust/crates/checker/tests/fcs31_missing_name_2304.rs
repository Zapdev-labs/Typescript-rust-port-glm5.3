//! FCS31 pins (full-corpus check sweep, tranche 31): the rust-missing
//! TS2304 "Cannot find name '{0}'" family — 39 instances across 22
//! divergent records before the tranche (the largest remaining shape:
//! unresolved type-parameter CONSTRAINT names of class/interface/
//! class-expression declarations plus accessor computed names).
//!
//! ROOTS fixed this tranche ("FCS tranche 31" PORT notes in-tree):
//!
//! 1. `type_parameters_of` (statements.rs — Go `Node.TypeParameterList`,
//!    ast.go:527) had no arms for ClassDeclaration / ClassExpression /
//!    InterfaceDeclaration (+ TypeAliasDeclaration|JSTypeAliasDeclaration,
//!    JSDocTemplateTag), so checkClassLikeDeclaration /
//!    checkInterfaceDeclaration passed an EMPTY list to
//!    checkTypeParameters and checkTypeParameter NEVER ran for their type
//!    parameters — the constraint node was never visited via
//!    checkSourceElement, so `class C<T extends Date>` (no libs) never
//!    reported its 2304 (nor 2368/2344-family constraint checks).
//!
//! 2. checkAccessorDeclaration (Go checker.go:2979) called the GRAMMAR
//!    stub where Go calls the REAL checkComputedPropertyName
//!    (checker.go:27336) for accessor computed names — `get [Symbol()]()`
//!    never reported its TS2304 (the same W3T5 miswiring shape the
//!    binding-pattern sites got earlier).
//!
//! All pins run WITHOUT libs (the corpus twin seam — the per-unit
//! checker), matching the go-bench driver's construction.

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::options_generated::{CompilerOptions, ScriptTarget};
use tsc_core::scriptkind::ScriptKind;
use tsc_core::tristate::Tristate;
use tsc_parser::parse_source_file;
use tsc_tspath::RootedFilePath;

// ── The single-file Program scaffold (the fcs23+ idiom) ──

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
            implied_node_format: tsc_core::compileroptions::ResolutionModeNone,
        }
    }
    fn file_exists(&self, path: &str) -> bool {
        path == self.name.as_string()
    }
}

fn check_with(src: &str, options: CompilerOptions) -> (Checker, tsc_ast::NodeId) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let program = TestProgram {
        options,
        name: RootedFilePath::from("/x/unit.ts"),
        source_file: sf,
        bind,
        binder_symbol_count,
        checker_symbols: Vec::new(),
    };
    let mut checker = new_checker(Box::new(program));
    checker.check_source_file(file, true);
    (checker, file)
}

/// strict options at the given target.
fn options(target: ScriptTarget) -> CompilerOptions {
    CompilerOptions {
        strict: Tristate::TSTrue,
        target,
        ..CompilerOptions::default()
    }
}

/// The (code, args) list of the checker's WHOLE semantic collection (the
/// corpus seam reads `semantic_diagnostics()`).
fn diags(c: &mut Checker, file: tsc_ast::NodeId) -> Vec<(i32, Vec<String>)> {
    let _ = file;
    c.semantic_diagnostics()
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

// ── Pin 1: the keystone — a CLASS type-parameter constraint reports ──
//
// The corpus-proven root-1 shape (typeParameterUsedAsConstraint.ts:
// `class C3<T extends Date, U extends T>`): with no libs, the constraint
// node is visited via checkTypeParameter's checkSourceElement and the
// unresolved `Date` reports TS2304 with the suggested-lib second arg
// (onFailedToResolveSymbol's getSuggestedLibForNonExistentName arm,
// checker.go:1629). `U extends T` resolves (T is in scope) — no extra
// 2304.

const CLASS_CONSTRAINT: &str = "class C3<T extends Date, U extends T> { }\n";

#[test]
fn class_constraint_missing_name_2304_pin() {
    let (mut c, f) = check_with(CLASS_CONSTRAINT, options(ScriptTarget::ES2015));
    let ds = diags(&mut c, f);
    assert_eq!(
        codes(&ds, 2304),
        vec![vec!["Date".to_string(), "esnext".to_string()]],
        "the class constraint reports exactly one 2304: {ds:?}"
    );
}

// ── Pin 2: the INTERFACE + class-expression arms ──
//
// The same constraint shape through interface declarations
// (twoGenericInterfacesWithDifferentConstraints.ts) and class
// expressions — Go's TypeParameterList covers both, the tranche's
// type_parameters_of arms feed both through checkTypeParameters.

const INTERFACE_CONSTRAINT: &str = "interface J<T extends Date> { }\n";
const CLASS_EXPR_CONSTRAINT: &str = "let C = class<T extends Date> { };\n";

#[test]
fn interface_constraint_missing_name_2304_pin() {
    let (mut c, f) = check_with(INTERFACE_CONSTRAINT, options(ScriptTarget::ES2015));
    let ds = diags(&mut c, f);
    assert_eq!(
        codes(&ds, 2304),
        vec![vec!["Date".to_string(), "esnext".to_string()]],
        "the interface constraint reports exactly one 2304: {ds:?}"
    );
}

#[test]
fn class_expression_constraint_missing_name_2304_pin() {
    let (mut c, f) = check_with(CLASS_EXPR_CONSTRAINT, options(ScriptTarget::ES2015));
    let ds = diags(&mut c, f);
    assert_eq!(
        codes(&ds, 2304),
        vec![vec!["Date".to_string(), "esnext".to_string()]],
        "the class expression constraint reports exactly one 2304: {ds:?}"
    );
}

// ── Pin 3: the reserved-name family rides the same visit ──
//
// checkTypeParameter's checkTypeNameIsReserved (checker.go:2664) only
// ran for function-like type parameters before the tranche — the corpus
// twin reports TS2368 for `class C3<any>` / `interface J<string>`.

#[test]
fn class_type_parameter_reserved_name_2368_pin() {
    let (mut c, f) = check_with("class C3<any> { }\n", options(ScriptTarget::ES2015));
    let ds = diags(&mut c, f);
    assert_eq!(
        codes(&ds, 2368),
        vec![vec!["any".to_string()]],
        "the class type parameter name reports exactly one 2368: {ds:?}"
    );
}

// ── Pin 4: the accessor computed name — the root-2 shape ──
//
// symbolProperty7.ts's `get [Symbol()]()` — checkAccessorDeclaration's
// REAL checkComputedPropertyName (checker.go:3001) resolves the
// computed-name expression and reports the TS2304 for the unresolved
// Symbol (the grammar stub never did).

const GETTER_COMPUTED: &str = "class C { get [Symbol()]() { return 1; } }\n";

#[test]
fn accessor_computed_name_missing_name_2304_pin() {
    let (mut c, f) = check_with(GETTER_COMPUTED, options(ScriptTarget::ES2015));
    let ds = diags(&mut c, f);
    assert_eq!(
        codes(&ds, 2304),
        vec![vec!["Symbol".to_string(), "es2015".to_string()]],
        "the getter computed name reports exactly one 2304: {ds:?}"
    );
}

// ── Pin 5: the constraint-check path rides the same visit (2344) ──
//
// With the constraint node now visited, the type-argument constraint
// check runs for class constraints exactly like Go (the corpus twin's
// `interface J<T extends E<string>>` records TS2344 where the type
// argument violates the inner constraint).

const CONSTRAINT_VIOLATION: &str =
    "interface E<N extends number> { }\ninterface J<T extends E<string>> { }\n";

#[test]
fn class_constraint_type_argument_violation_2344_pin() {
    let (mut c, f) = check_with(CONSTRAINT_VIOLATION, options(ScriptTarget::ES2015));
    let ds = diags(&mut c, f);
    assert_eq!(
        codes(&ds, 2344),
        vec![vec!["string".to_string(), "number".to_string()]],
        "the constraint type argument reports exactly one 2344: {ds:?}"
    );
}

// ── Pin 6: the negative — method/property computed names were already ──
//
// correct (they used the real checkComputedPropertyName all along); the
// accessor fix must not double-report them.

const METHOD_COMPUTED: &str = "class C { [Symbol()]: number; }\n";

#[test]
fn property_computed_name_single_report_pin() {
    let (mut c, f) = check_with(METHOD_COMPUTED, options(ScriptTarget::ES2015));
    let ds = diags(&mut c, f);
    assert_eq!(
        codes(&ds, 2304),
        vec![vec!["Symbol".to_string(), "es2015".to_string()]],
        "the property computed name still reports exactly one 2304: {ds:?}"
    );
}

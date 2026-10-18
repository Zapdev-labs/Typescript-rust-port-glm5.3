//! FCS50 pins (full-corpus check sweep, micro-tranche 50): the top coherent
//! clusters of the fresh t50 census —
//!
//!  * the type-predicate validation family (TS2677/TS1229, corpus record
//!    typeGuardFunctionErrors.ts): checkTypePredicate's assignability arm now
//!    builds Go's NewDiagnosticChain — the TS2677 message is the HEAD at the
//!    elaboration's own location with the original elaboration as its
//!    message chain tail (checker.go:3130) — where the port inverted the
//!    pair (the elaboration stayed primary with 2677 as related info, so
//!    the corpus showed bare TS2741/TS2322 extras); the rest-parameter arm
//!    now uses the REAL SignatureFlags probe (signatureHasRestParameter,
//!    checker.go:17425) instead of the permissive-false stub, so
//!    `(...a): a is A` fires TS1229 ("A type predicate cannot reference a
//!    rest parameter") instead of the assignability check;
//!  * the TS2551 did-you-mean spelling-suggestion family (five corpus
//!    records): isPropertyAccessible is Go-exact now (checker.go:11874 —
//!    was the M6 S4 permissive-false stub), so the completion-validity
//!    filter in getSuggestedSymbolForNonexistentProperty keeps public
//!    candidates and the property-access error fires 2551
//!    "Property 'x' does not exist on type 'T'. Did you mean 'y'?" instead
//!    of the plain 2339 (inaccessible candidates stay filtered: a private
//!    near-match still answers the plain 2339);
//!  * the dependent-destructured-variable narrowing family (corpus record
//!    dependentDestructuredVariables.ts): getNarrowedTypeOfSymbol's
//!    binding-element arm (checker.go:14010) now walks the flow graph from
//!    the LOCATION's flow node (getFlowNodeOfNode(location) — was the
//!    pattern's own node, so conditional checks upstream of the reference
//!    never applied) with the real isSomeSymbolAssigned guard, and the
//!    second arm (checker.go:14079 — const-like parameters of a
//!    single-rest-parameter signature of a union of tuple types; the
//!    function node is the pseudo-reference) is ported at all.
//!
//! ("FCS tranche 50" PORT notes in-tree.)

use tsc_ast::{NodeStore, SourceFile, SourceFileParseOptions, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::options_generated::{CompilerOptions, ModuleKind, ScriptTarget};
use tsc_core::scriptkind::ScriptKind;
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
            implied_node_format: tsc_core::compileroptions::ResolutionModeNone,
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

fn check_unit(src: &str, name: &str, options: CompilerOptions) -> (Checker, tsc_ast::NodeId) {
    let file_name = RootedFilePath::from(name);
    let mut sf = parse_source_file(
        SourceFileParseOptions {
            file_name,
            ..Default::default()
        },
        src,
        ScriptKind::TS,
    );
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let program = TestProgram {
        options,
        name: RootedFilePath::from(name),
        source_file: sf,
        bind,
        binder_symbol_count,
        checker_symbols: Vec::new(),
    };
    let mut checker = new_checker(Box::new(program));
    checker.check_source_file(file, true);
    (checker, file)
}

fn diags(c: &mut Checker) -> Vec<tsc_ast::Diagnostic> {
    c.semantic_diagnostics()
}

fn options_es2015() -> CompilerOptions {
    CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::CommonJS,
        ..CompilerOptions::default()
    }
}

// ── ROOT A — the type-predicate validation family ──

/// `(x: string): x is number` fires the HEAD TS2677 ("A type predicate's
/// type must be assignable to its parameter's type", no args) with the
/// TS2322 elaboration ("Type 'number' is not assignable to type
/// 'string'") as its message chain tail — never a bare top-level TS2322
/// (Go: checkTypePredicate, checker.go:3126-3132 — ast.NewDiagnosticChain
/// puts the wrap message at the elaboration's location; the corpus record
/// is typeGuardFunctionErrors.ts — DUMP 2677 with empty args in both
/// twins).
#[test]
fn fcs50_type_predicate_2677_chain_head_pin() {
    let (mut c, _) = check_unit(
        "function g(x: string): x is number {\n    return true;\n}\n",
        "/x/fcs50_predicate_chain.ts",
        options_es2015(),
    );
    let ds = diags(&mut c);
    let heads: Vec<&tsc_ast::Diagnostic> = ds.iter().filter(|d| d.code() == 2677).collect();
    assert_eq!(
        heads.len(),
        1,
        "expected exactly 1 TS2677 head, got: {:?}",
        ds.iter().map(|d| d.code()).collect::<Vec<_>>()
    );
    assert!(
        heads[0].message_args().is_empty(),
        "the head carries no args, got: {:?}",
        heads[0].message_args()
    );
    let chain = heads[0].message_chain();
    assert_eq!(chain.len(), 1, "expected one chain member");
    assert_eq!(chain[0].code(), 2322, "the chain tail is the elaboration");
    assert_eq!(
        chain[0].message_args(),
        &["number".to_string(), "string".to_string()],
        "elaboration args are [predicate type, parameter type], got: {:?}",
        chain[0].message_args()
    );
    assert!(
        ds.iter().all(|d| d.code() != 2322),
        "no bare top-level TS2322 may fire, got: {:?}",
        ds.iter().map(|d| d.code()).collect::<Vec<_>>()
    );
}

/// `(...a): a is A` references the rest parameter: the REAL
/// signatureHasRestParameter probe routes to the TS1229 arm ("A type
/// predicate cannot reference a rest parameter", no args) INSTEAD of the
/// assignability check (no 2677, no elaboration) — the permissive-false
/// stub used to fall through into the assignability arm (Go:
/// checker.go:3121-3125; the corpus record is typeGuardFunctionErrors.ts —
/// DUMP 1229 with empty args in the go twin only, pre-fix).
#[test]
fn fcs50_type_predicate_rest_parameter_1229_pin() {
    let (mut c, _) = check_unit(
        "class A { propA: number; }\nfunction b4(...a): a is A {\n    return true;\n}\n",
        "/x/fcs50_predicate_rest.ts",
        options_es2015(),
    );
    let ds = diags(&mut c);
    let rests: Vec<&tsc_ast::Diagnostic> = ds.iter().filter(|d| d.code() == 1229).collect();
    assert_eq!(
        rests.len(),
        1,
        "expected exactly 1 TS1229, got: {:?}",
        ds.iter().map(|d| d.code()).collect::<Vec<_>>()
    );
    assert!(rests[0].message_args().is_empty());
    assert!(
        ds.iter().all(|d| d.code() != 2677),
        "the rest-parameter arm replaces the assignability check, got: {:?}",
        ds.iter().map(|d| d.code()).collect::<Vec<_>>()
    );
}

// ── ROOT B — the TS2551 spelling-suggestion family ──

/// `a.propB` on `A { propA }` fires TS2551 with the did-you-mean arg
/// (the completion-validity filter keeps the accessible public candidate
/// through the Go-exact isPropertyAccessible; the S4 permissive-false
/// stub filtered everything and answered the plain 2339) — the corpus
/// shape of typeGuardFunctionErrors.ts /
/// typeGuardsWithInstanceOfBySymbolHasInstance.ts.
#[test]
fn fcs50_property_suggestion_2551_pin() {
    let (mut c, _) = check_unit(
        "class A { propA: number; }\ndeclare let a: A;\na.propB;\n",
        "/x/fcs50_suggestion_2551.ts",
        options_es2015(),
    );
    let ds = diags(&mut c);
    let sugg: Vec<&tsc_ast::Diagnostic> = ds.iter().filter(|d| d.code() == 2551).collect();
    assert_eq!(
        sugg.len(),
        1,
        "expected exactly 1 TS2551, got: {:?}",
        ds.iter().map(|d| d.code()).collect::<Vec<_>>()
    );
    assert_eq!(
        sugg[0].message_args(),
        &["propB".to_string(), "A".to_string(), "propA".to_string()],
        "2551 args are [property, container, suggestion], got: {:?}",
        sugg[0].message_args()
    );
    assert!(
        ds.iter().all(|d| d.code() != 2339),
        "the suggestion replaces the plain 2339, got: {:?}",
        ds.iter().map(|d| d.code()).collect::<Vec<_>>()
    );
}

/// A PRIVATE near-match stays filtered by the completion-validity check
/// (isPropertyAccessible → checkPropertyAccessibilityAtLocation answers
/// false at the module scope), so `a.propB` with `private propA` answers
/// the PLAIN 2339 — the accessibility semantics, not a blanket
/// suggestion (Go: checker.go:11850 + getSuggestedSymbolForNonexistentProperty).
#[test]
fn fcs50_property_suggestion_private_stays_plain_2339_pin() {
    let (mut c, _) = check_unit(
        "class A { private propA: number; }\ndeclare let a: A;\na.propB;\n",
        "/x/fcs50_suggestion_private.ts",
        options_es2015(),
    );
    let ds = diags(&mut c);
    let plain: Vec<&tsc_ast::Diagnostic> = ds.iter().filter(|d| d.code() == 2339).collect();
    assert_eq!(plain.len(), 1, "expected exactly 1 plain TS2339");
    assert_eq!(
        plain[0].message_args(),
        &["propB".to_string(), "A".to_string()],
        "plain 2339 args are [property, container], got: {:?}",
        plain[0].message_args()
    );
    assert!(
        ds.iter().all(|d| d.code() != 2551),
        "an inaccessible candidate must not produce a suggestion, got: {:?}",
        ds.iter().map(|d| d.code()).collect::<Vec<_>>()
    );
}

// ── ROOT C — the dependent-destructured-variable narrowing family ──

/// `function f({ kind, payload }: Action)` + `if (kind === 'A')` narrows
/// `payload` to `number` at the guarded reference: the lib-less
/// `payload.toFixed()` fires 2339 with container `number` (not
/// `string | number`) for both guards — the pseudo-reference walk now
/// starts at the LOCATION's flow node (Go: getNarrowedTypeOfSymbol arm 1,
/// checker.go:14010-14042; the corpus record is
/// dependentDestructuredVariables.ts — DUMP 2339 toFixed/number in the go
/// twin only, pre-fix).
#[test]
fn fcs50_dependent_destructured_discriminant_pin() {
    let (mut c, _) = check_unit(
        "type Action =\n    | { kind: 'A', payload: number }\n    | { kind: 'B', payload: string };\nfunction f10({ kind, payload }: Action) {\n    if (kind === 'A') {\n        payload.toFixed();\n    }\n    if (kind === 'B') {\n        payload.toUpperCase();\n    }\n}\n",
        "/x/fcs50_dependent_destructured.ts",
        options_es2015(),
    );
    let ds = diags(&mut c);
    let props: Vec<&tsc_ast::Diagnostic> = ds.iter().filter(|d| d.code() == 2339).collect();
    assert_eq!(
        props.len(),
        2,
        "expected exactly 2 TS2339 (one per guard), got: {:?}",
        ds.iter().map(|d| d.code()).collect::<Vec<_>>()
    );
    let mut containers: Vec<String> = props
        .iter()
        .map(|d| d.message_args().last().cloned().unwrap_or_default())
        .collect();
    containers.sort();
    assert_eq!(
        containers,
        vec!["number".to_string(), "string".to_string()],
        "the narrowed containers are the per-guard discriminant types, got: {:?}",
        containers
    );
}

/// A const-like parameter list contextually typed by a single-rest
/// parameter of a union of tuple types narrows per-discriminant too: the
/// arrow function node is the pseudo-reference and the narrowed parameter
/// type is the tuple-indexed access (Go: getNarrowedTypeOfSymbol arm 2,
/// checker.go:14079-14096 — the arm was absent entirely; the corpus shape
/// is dependentDestructuredVariables.ts's f50 — DUMP 2339 toFixed/number
/// in the go twin only, pre-fix).
#[test]
fn fcs50_rest_tuple_parameters_discriminant_pin() {
    let (mut c, _) = check_unit(
        "type Action = ['A', number] | ['B', string];\nconst f: (...args: Action) => void = (kind, payload) => {\n    if (kind === 'A') {\n        payload.toFixed();\n    }\n};\n",
        "/x/fcs50_rest_tuple_params.ts",
        options_es2015(),
    );
    let ds = diags(&mut c);
    let props: Vec<&tsc_ast::Diagnostic> = ds.iter().filter(|d| d.code() == 2339).collect();
    assert_eq!(
        props.len(),
        1,
        "expected exactly 1 TS2339, got: {:?}",
        ds.iter().map(|d| d.code()).collect::<Vec<_>>()
    );
    assert_eq!(
        props[0].message_args(),
        &["toFixed".to_string(), "number".to_string()],
        "the narrowed parameter type is the tuple-indexed number, got: {:?}",
        props[0].message_args()
    );
}

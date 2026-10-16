//! FCS29 pins (full-corpus check sweep, tranche 29): the rust-missing
//! TS2318 "Cannot find global type 'X'" family — 130 instances across
//! 86 records before the tranche.
//!
//! ROOTS fixed this tranche ("FCS tranche 29" PORT notes in-tree):
//!
//! 1. EXTRACT (25 instances): `getExtractStringType` (checker.go:27242)
//!    read the MEMOIZED `getGlobalExtractSymbol()` — the resolver row is
//!    reportErrors=true (checker.go:1080). The port called
//!    `getGlobalTypeAliasSymbol("Extract", 2, false)` directly, so the
//!    location-less 2318 never fired. Fixed in index.rs (the spine's
//!    `get_global_extract_symbol` accessor).
//!
//! 2. ITERABLE (20): the empty/rest-only array binding pattern's type is
//!    `createIterableType(c.anyType)` for target >= ES2015
//!    (checker.go:18370) — the reportErrors=true `Iterable` row. The port
//!    returned permissive `anyArrayType` (a TODO'd M6 S7 stub). Fixed in
//!    typeofsymbol.rs (`create_iterable_type`, jsx.rs's W5 twin).
//!
//! 3. TYPEDPROPERTYDESCRIPTOR (45) + the CLASS*DECORATORCONTEXT family
//!    (41): the whole decorator call-signature wave (checker.go:30665-
//!    31101) — `getDecoratorCallSignature` /
//!    `getLegacyDecoratorCallSignature` / `getESDecoratorCallSignature` +
//!    the synthetic helpers (newParameter/newProperty, the context/override
//!    types) + the call sites (checkDecorator 6245, resolveDecorator 8974+
//!    8986, getContextualTypeForDecorator 30349, the decorator arm of
//!    getEffectiveCallArguments 30610) + the checkDecorators grammar gate
//!    (checker.go:6208: CanHaveDecorators/HasDecorators/
//!    NodeCanBeDecorated — the permissive walk had run the machinery for
//!    private-named members and body-less overloads that Go rejects with
//!    TS1206 first, 3 newly-extra 2318s). Ported in statements.rs,
//!    resolvecall.rs, contextual.rs, checker.rs.
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

/// legacy (experimental) decorator options at the given target.
fn legacy_options(target: ScriptTarget) -> CompilerOptions {
    CompilerOptions {
        strict: Tristate::TSTrue,
        target,
        experimental_decorators: Tristate::TSTrue,
        ..CompilerOptions::default()
    }
}

/// The (code, args) list of the checker's WHOLE semantic collection (the
/// location-less TS2318s land in the global entries, not the per-file
/// read — the corpus seam reads `semantic_diagnostics()`, so do the pins).
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

// ── Pin 1: Extract — the getGlobalExtractSymbol checked row ──
//
// `for (const k in x)` over a TYPE PARAMETER right-hand side types k as
// `keyof T` (getTypeForVariableLikeDeclaration's ForInStatement arm →
// getExtractStringType, checker.go:27242) — the memoized reportErrors=true
// `Extract` resolver row reports exactly one location-less TS2318
// "Cannot find global type 'Extract'" (the corpus records
// statements/for-inStatements/for-inStatementsAsyncIdentifier.ts +
// keyof/keyofAndForIn.ts shape).

const FOR_IN_TYPE_PARAM: &str = "function f<T>(x: T) {\n    for (const k in x) {\n        const v: string = k;\n    }\n}\n";

#[test]
fn extract_2318_checked_row_pin() {
    let (mut c, f) = check_with(FOR_IN_TYPE_PARAM, options(ScriptTarget::ES2015));
    let ds = diags(&mut c, f);
    assert_eq!(
        codes(&ds, 2318)
            .iter()
            .filter(|args| **args == vec!["Extract".to_string()])
            .count(),
        1,
        "the memoized Extract resolver reports once, location-less: {ds:?}"
    );
}

// ── Pin 2: Iterable — the empty array binding pattern ──
//
// `function f([])` at ES2015+ types the pattern Iterable<any, any,
// undefined> (createIterableType(c.anyType), checker.go:18370 — the
// reportErrors=true `Iterable` row); below ES2015 Go answers anyArrayType
// with NO 2318 (the `// TODO: remove ScriptTargetES2015` gate).

const EMPTY_ARRAY_PARAM: &str = "function f([]) {\n    var x, y, z;\n}\n";

#[test]
fn empty_array_binding_pattern_iterable_2318_pin() {
    let (mut c, f) = check_with(EMPTY_ARRAY_PARAM, options(ScriptTarget::ES2015));
    let ds = diags(&mut c, f);
    assert!(
        codes(&ds, 2318)
            .iter()
            .any(|args| args == &vec!["Iterable".to_string()]),
        "ES2015: the Iterable checked row fires: {ds:?}"
    );
}

#[test]
fn empty_array_binding_pattern_es5_no_2318_pin() {
    let (mut c, f) = check_with(EMPTY_ARRAY_PARAM, options(ScriptTarget::ES5));
    let ds = diags(&mut c, f);
    assert!(
        !codes(&ds, 2318)
            .iter()
            .any(|args| args == &vec!["Iterable".to_string()]),
        "ES5: anyArrayType, no Iterable 2318 (Go's pre-ES2015 arm): {ds:?}"
    );
}

// ── Pin 3: TypedPropertyDescriptor — the legacy decorator signature ──
//
// A legacy decorator on a class METHOD builds the synthetic
// (target, propertyKey, descriptor: TypedPropertyDescriptor<T>) call
// signature (checker.go:30750-30771) — the reportErrors=true
// `TypedPropertyDescriptor` resolver row reports once.

const LEGACY_METHOD_DECORATOR: &str = "declare function dec(target: any, propertyKey: string, descriptor: any): void;\n\nclass C {\n    @dec\n    m(): void {\n        this.m();\n    }\n}\n";

#[test]
fn legacy_method_decorator_typed_property_descriptor_2318_pin() {
    let (mut c, f) = check_with(LEGACY_METHOD_DECORATOR, legacy_options(ScriptTarget::ES2015));
    let ds = diags(&mut c, f);
    assert_eq!(
        codes(&ds, 2318)
            .iter()
            .filter(|args| **args == vec!["TypedPropertyDescriptor".to_string()])
            .count(),
        1,
        "exactly one TypedPropertyDescriptor 2318: {ds:?}"
    );
}

// ── Pin 4: ClassMethodDecoratorContext — the ES decorator signature ──
//
// A native (ES2022) decorator on a class METHOD builds
// ClassMethodDecoratorContext<This, Value> (checker.go:30886-30905 —
// the reportErrors=true context row family).

const ES_METHOD_DECORATOR: &str = "declare function dec(target: unknown, context: unknown): void;\n\nclass C {\n    @dec\n    m(): void {\n        this.m();\n    }\n}\n";

#[test]
fn es_method_decorator_context_2318_pin() {
    let (mut c, f) = check_with(ES_METHOD_DECORATOR, options(ScriptTarget::ES2022));
    let ds = diags(&mut c, f);
    assert_eq!(
        codes(&ds, 2318)
            .iter()
            .filter(|args| **args == vec!["ClassMethodDecoratorContext".to_string()])
            .count(),
        1,
        "exactly one ClassMethodDecoratorContext 2318: {ds:?}"
    );
}

// ── Pin 5: the NodeCanBeDecorated gate — private names ──
//
// Legacy decorators on a PRIVATE class element are rejected by
// NodeCanBeDecorated (ast/utilities.go:4330 — "private names cannot be
// used with decorators yet") before any signature is built: NO
// TypedPropertyDescriptor 2318 (the corpus record
// classes/members/privateNames/privateNamesAndDecorators.ts, 12 errors
// both drivers, the 1206s already matched).

const PRIVATE_NAME_DECORATOR: &str = "declare function dec<T>(target: T): T;\n\nclass A {\n    @dec\n    #foo = 1;\n}\n";

#[test]
fn private_name_decorator_gate_no_2318_pin() {
    let (mut c, f) = check_with(PRIVATE_NAME_DECORATOR, legacy_options(ScriptTarget::ES2015));
    let ds = diags(&mut c, f);
    assert!(
        !codes(&ds, 2318)
            .iter()
            .any(|args| args == &vec!["TypedPropertyDescriptor".to_string()]),
        "private names never reach the signature construction: {ds:?}"
    );
}

// ── Pin 6: the NodeCanBeDecorated gate — body-less overloads ──
//
// Legacy decorators on an overloaded METHOD SIGNATURE (no body) are
// rejected by NodeCanBeDecorated's `node.Body() != nil` arm
// (ast/utilities.go:4348): NO TypedPropertyDescriptor 2318 (the corpus
// record decorators/class/method/decoratorOnClassMethodOverload1.ts,
// 11/12 errors both drivers).

const OVERLOAD_DECORATOR: &str = "declare function dec<T>(target: any, propertyKey: string, descriptor: TypedPropertyDescriptor<T>): TypedPropertyDescriptor<T>;\n\nclass C {\n    @dec\n    method();\n    method() { }\n}\n";

#[test]
fn overload_decorator_gate_no_2318_pin() {
    let (mut c, f) = check_with(OVERLOAD_DECORATOR, legacy_options(ScriptTarget::ES2015));
    let ds = diags(&mut c, f);
    assert!(
        !codes(&ds, 2318)
            .iter()
            .any(|args| args == &vec!["TypedPropertyDescriptor".to_string()]),
        "body-less overloads never reach the signature construction: {ds:?}"
    );
}

// ── Pin 7: isPotentiallyUncalledDecorator — TS1329 ──
//
// A legacy decorator whose every call signature takes zero parameters
// (and no rest) with fewer params than the synthetic argument count
// reports TS1329 "'X' accepts too few arguments to be used as a
// decorator here. Did you mean to call it first and write '@X()'?"
// (checker.go:8974-8980) and resolves as an error call — so
// checkDecorator's any-return gate then skips the signature entirely
// (NO TypedPropertyDescriptor 2318).

const ZERO_ARG_DECORATOR: &str = "declare function dec(): void;\n\nclass C {\n    @dec\n    m(): void {\n        this.m();\n    }\n}\n";

#[test]
fn uncalled_decorator_1329_pin() {
    let (mut c, f) = check_with(ZERO_ARG_DECORATOR, legacy_options(ScriptTarget::ES2015));
    let ds = diags(&mut c, f);
    assert_eq!(codes(&ds, 1329), vec![vec!["dec".to_string()]], "{ds:?}");
    assert!(
        !codes(&ds, 2318)
            .iter()
            .any(|args| args == &vec!["TypedPropertyDescriptor".to_string()]),
        "the error-call path skips the synthetic signature: {ds:?}"
    );
}

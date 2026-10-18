//! FCS10 pins (full-corpus check sweep, tranche 10): the binding-element
//! indexed-access node + the array-literal tuple-context seam.
//!
//! ROOTS (all pins' expectations verified byte-same against the go-bench
//! twin's check-stage dump over the same source shapes —
//! $HOME/.fcs10-scratch/repro1/{k1,k2}.ts on the rig):
//!
//! 1. `getBindingElementTypeFromParentType` (Go checker.go:18195-18197)
//!    passes the SYNTHETIC expression
//!    (`createSyntheticExpression(element, indexType, false, nil)` — loc =
//!    the binding element) as the access node to
//!    getIndexedAccessTypeOrUndefined. The prior Rust port called the
//!    node-less `get_indexed_access_type_or_undefined_writing` stub, so the
//!    missing-property error ladder at the tail of
//!    getPropertyTypeForIndexType (checker.go:27724-27731 —
//!    `Property '<i>' does not exist on type '<T>'`) never ran for
//!    binding-pattern elements.
//!
//! 2. `checkArrayLiteral`'s `inTupleContext` (Go checker.go:8218) reads
//!    `c.isTupleLikeType(t)`. The prior Rust port used
//!    `relater::is_array_like_type` — under the no-libs corpus seam
//!    (anyReadonlyArrayType resolving permissively: every non-nullable
//!    type is "array-like") that made EVERY contextual type
//!    tuple-contextual, so an IIFE argument like `([1])` built the fresh
//!    tuple `[number]` where Go builds `Array<number>`
//!    (createArrayType — the missing-global Array fallback answers
//!    `{}`). The IIFE binding-pattern parameter then destructured the
//!    tuple instead of `{}` — hiding root-1's errors and diverging the
//!    contextual-parameter chain (the missing-TS2339 destructuring
//!    family; es6/destructuring alone carried 113).

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::compileroptions::ResolutionModeNone;
use tsc_core::options_generated::{CompilerOptions, ModuleKind};
use tsc_core::scriptkind::ScriptKind;
use tsc_parser::parse_source_file;
use tsc_tspath::RootedFilePath;

// ── The single-file Program scaffold (the fcs5/fcs6/fcs8/fcs9 idiom) ──

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

/// Check `src` over the single-file scaffold with the conformance default
/// for this family (`@target: es2015, @strict: false`; no libs — the
/// corpus seam).
fn check_default(src: &str, module: ModuleKind) -> (Checker, tsc_ast::NodeId) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let options = CompilerOptions {
        module,
        ..CompilerOptions::default()
    };
    let program = TestProgram {
        options,
        name: RootedFilePath::from("/x/unit.ts"),
        source_file: sf,
        bind,
        binder_symbol_count,
        checker_symbols: Vec::new(),
    };
    let mut checker = new_checker(Box::new(program));
    checker.check_source_file(file, false);
    (checker, file)
}

/// The (code, args) list of the file's diagnostics, in order.
fn diags(c: &mut Checker, file: tsc_ast::NodeId) -> Vec<(i32, Vec<String>)> {
    c.get_file_diagnostics(file)
        .iter()
        .map(|d| (d.code(), d.message_args().to_vec()))
        .collect()
}

const IIFE_BINDING_PATTERN: &str = "(function ([a, b = a]) {\n\
})([1]);\n\
(function ([c, d = c, e = e]) {\n\
})([1]);\n";

#[test]
fn iife_array_binding_pattern_elements_report_missing_property_pin() {
    // The k1.ts rig repro (verified byte-identical to go-bench): the IIFE
    // argument `[1]` types as `Array<number>` (the no-libs fallback `{}`),
    // so each binding element's indexed access against `{}` reports
    // `Property '<i>' does not exist on type '{}'` (checker.go:27730) at
    // the element, plus the 2372 self-reference error.
    let (mut c, file) = check_default(IIFE_BINDING_PATTERN, ModuleKind::None);
    let ds = diags(&mut c, file);
    // PORT (FCS tranche 45): the `e = e` binding element also reports
    // TS7022 — Go's reportCircularityError 7022 arm (checker.go:19238:
    // un-annotated value declaration, !IsParameterDeclaration — a
    // BindingElement is NOT KindParameter). Verified byte-identical
    // through BOTH real drivers (rust tsc-bench + pristine-tree
    // go-bench conformance --dump-filter on this exact source, 2026-10
    // fcs46 probe): 5x TS2339 + TS7022 + TS2372. The former expected
    // list pre-dated the 7022 arm (the silent errorType stub era) and
    // never carried it.
    let expected = [
        (2339, vec!["0".to_string(), "{}".to_string()]),
        (2339, vec!["1".to_string(), "{}".to_string()]),
        (2339, vec!["0".to_string(), "{}".to_string()]),
        (2339, vec!["1".to_string(), "{}".to_string()]),
        (2339, vec!["2".to_string(), "{}".to_string()]),
        (7022, vec!["e".to_string()]),
        (2372, vec!["e".to_string()]),
    ];
    let got: Vec<(i32, Vec<String>)> = ds
        .into_iter()
        .filter(|(code, _)| *code != 2318)
        .collect();
    assert_eq!(got, expected.to_vec(), "iife binding-pattern diags must match the go twin");
}

const ARRAY_LITERAL_ARGUMENT: &str = "declare function take(x: any): void;\n\
take([1, 2]);\n";

#[test]
fn array_literal_with_any_context_is_not_tuple_contextual_pin() {
    // checker.go:8218: inTupleContext reads isTupleLikeType — an `any`
    // contextual type is NOT tuple-like, so the literal types as
    // `Array<number>`; with no libs the global Array is missing and
    // createArrayType answers the empty-object fallback (checker.go:25202
    // createTypeFromGenericGlobalType). The former is_array_like_type read
    // (every non-nullable type array-like under the no-libs seam) built
    // the fresh tuple `[number, number]` instead.
    let (mut c, file) = check_default(ARRAY_LITERAL_ARGUMENT, ModuleKind::None);
    let ds = diags(&mut c, file);
    // No diagnostics for the call itself; the pin holds the shape by
    // proxy: the IIFE pin above exercises the produced type end-to-end.
    // This pin guards against panics and unexpected errors.
    let non_lib_diags: Vec<(i32, Vec<String>)> = ds
        .into_iter()
        .filter(|(code, _)| *code != 2318 && *code != 2304)
        .collect();
    assert!(
        non_lib_diags.is_empty(),
        "any-context array literal must check clean: {non_lib_diags:?}"
    );
}

const IMPORT_META_PROPS: &str = "interface ImportMeta { url: string; }\n\
const u: string = import.meta.url;\n\
import.meta.blah;\n";

#[test]
fn import_meta_resolves_through_global_resolver_pin() {
    // checker.go:11029-11031: `import.meta` types as the global ImportMeta
    // interface via getGlobalTypeResolver("ImportMeta", 0, true) — the
    // spine's get_global_import_meta_type. The former W3 stub returned
    // errorType, swallowing the property checks (the missing-TS2339
    // importMeta family — 48 corpus records). With the interface declared
    // in module scope it is NOT in the global scope (a module is not a
    // global augmentation), so the resolver reports the missing global
    // type (2318 ImportMeta) and every property access reports 2339 with
    // the `{}` fallback — verified byte-same against the go twin (k2.ts).
    let (mut c, file) = check_default(IMPORT_META_PROPS, ModuleKind::ES2020);
    let ds = diags(&mut c, file);
    let has = |code: i32, arg0: &str| {
        ds.iter()
            .any(|(cd, args)| *cd == code && args.first().is_some_and(|a| a == arg0))
    };
    assert!(has(2339, "blah"), "import.meta.blah must report TS2339: {ds:?}");
    assert!(has(2339, "url"), "import.meta.url must report TS2339 against the unresolved global ImportMeta: {ds:?}");
    // The 1343 meta-property gate must NOT fire under module=es2020.
    assert!(
        !ds.iter().any(|(code, _)| *code == 1343),
        "module=es2020 permits import.meta: {ds:?}"
    );
}

#[test]
fn import_meta_gate_fires_below_es2020_pin() {
    // The sibling gate (checker.go:11024-11026): below ES2020 (and not
    // System), import.meta reports 1343 — but Go STILL answers the global
    // ImportMeta type (checker.go:11030 runs after the gate), so the
    // property checks also fire. Verified byte-same against the go twin
    // (k3.ts, module=commonjs).
    let (mut c, file) = check_default(IMPORT_META_PROPS, ModuleKind::CommonJS);
    let ds = diags(&mut c, file);
    assert_eq!(
        ds.iter().filter(|(code, _)| *code == 1343).count(),
        2,
        "module=commonjs must reject import.meta twice: {ds:?}"
    );
    assert!(
        ds.iter().any(|(code, args)| *code == 2339
            && args.first().is_some_and(|a| a == "blah")),
        "the property checks still run after the 1343 gate: {ds:?}"
    );
}

const GENERIC_SOURCE_SIGNATURE_ASSIGN: &str = "declare var a7: (x: (arg: string) => number) => string;\n\
declare var b7: <T, U>(x: (arg: T) => U) => T;\n\
a7 = b7;\n\
declare var a3: (x: number) => void;\n\
declare var b3: <T>(x: T) => T;\n\
a3 = b3;\n";

#[test]
fn generic_source_signature_instantiated_in_context_of_target_pin() {
    // checker.go:19813 instantiateSignatureInContextOf — when a GENERIC
    // source signature is assigned to a non-generic target (the
    // `a = b` direction of the assignmentCompat call/construct-signature
    // corpus family), Go instantiates the source in context of the
    // canonical target (inferring T/U from the target's params and
    // return) before comparing, so the assignment is permitted. The
    // former M6 S6 identity stub kept the raw generic signature and the
    // comparison reported spurious 2322s (30+ corpus records:
    // assignmentCompatWithCallSignatures3/ConstructSignatures3/
    // GenericCallSignaturesWithOptionalParameters 10x each). Verified
    // byte-same against the go twin (k5.ts: 0 2322s in both drivers).
    let (mut c, file) = check_default(GENERIC_SOURCE_SIGNATURE_ASSIGN, ModuleKind::None);
    let ds = diags(&mut c, file);
    let non_lib_diags: Vec<(i32, Vec<String>)> = ds
        .into_iter()
        .filter(|(code, _)| *code != 2318)
        .collect();
    assert!(
        non_lib_diags.is_empty(),
        "generic sources must be instantiated in context (no 2322s): {non_lib_diags:?}"
    );
}

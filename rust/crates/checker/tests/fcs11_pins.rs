//! FCS11 pins (full-corpus check sweep, tranche 11): the optional-chain
//! removeOptionalTypeMarker seam, the template-literal pattern validators,
//! the 2322 optional-parameter elaboration path, and the declare-global
//! augmentation merge.
//!
//! ROOTS (all pins' expectations verified byte-same against the go-bench
//! twin's check-stage dump over the same source shapes —
//! $HOME/.fcs11-scratch/repro{1,2,3}/ on the rig):
//!
//! 1. `removeOptionalTypeMarker` (Go checker.go:29605) was the M6 S7
//!    identity stub. getOptionalExpressionType (checker.go:29593) takes the
//!    IsOptionalChain arm for a chain node (`o3.b?.()` under `.c`), and the
//!    call's return type carries the `optionalType` intrinsic instance
//!    because resolveTypedCallExpression classifies it IS_INNER_CALL_CHAIN
//!    and getReturnTypeOfSignature's inner arm is addOptionalTypeMarker.
//!    The identity stub left `| undefined` in the type checkNonNullType
//!    consumed, reporting spurious TS2532 on every `o3.b?.()` corpus shape
//!    (61 extra records: callChain 12, thisTypeInObjectLiterals2 12,
//!    logicalAssignment5 8, optionalMethods 6, ...).
//!
//! 2. `is_valid_type_for_template_literal_placeholder`'s number/bigint arms
//!    (Go relater.go:2504-2507) resolve `isValidNumberString` /
//!    `isValidBigIntString`. The Rust port had LOCAL identity-stub twins in
//!    relater.rs shadowing the real jsnum validators in utilities.rs, so
//!    every numeric-string literal failed `${number}` / `${bigint}`
//!    template-pattern checks (the TS2345 templateLiteralTypesPatterns
//!    family, 21 extras + 6 bigint).
//!
//! 3. `symbolValueDeclarationIsContextSensitive` (Go relater.go:1296) is
//!    `IsExpression(ValueDeclaration) && !isContextSensitive(...)` — the
//!    port returned the RAW isContextSensitive result (inverted, no
//!    IsExpression gate), so getTypeNamesForErrorDisplay always took the
//!    no-enclosing typeToString arm for arrow/function-expression symbols.
//!    Go threads the expression's ValueDeclaration as the nodebuilder
//!    enclosing, which enables serializeTypeForDeclaration's
//!    annotation-reuse arm (nodebuilderimpl.go:2253-2360): the optional
//!    parameter renders its written annotation (`x?: number`, not
//!    `x?: number | undefined` — the 26-string-only diff lines of the
//!    assignmentCompatWithCallSignaturesWithRestParameters family). The
//!    port now approximates the pseudochecker equivalence with checker-type
//!    identity against the annotation type (± addOptionalityEx's
//!    `getOptionalType(annType, isProperty)`) and renders the annotation's
//!    source text.
//!
//! 4. `mergeModuleAugmentation`'s global arm (Go checker.go:1449 —
//!    `c.mergeSymbolTable(c.globals, moduleAugmentation.Symbol.Exports,
//!    false, nil)`) was a TODO stub, so `declare global { interface
//!    ImportMeta {...} }` never merged into the global symbol table and
//!    getGlobalType("ImportMeta") reported 2318 while `import.meta.baz`
//!    2339'd against `{}` (the 3 broken importMetaNarrowing records).
//!    mergeSymbolTable/mergeSymbol (checker.go:14380/14417) are ported real
//!    (the related-information chains of reportMergeSymbolError are the
//!    W4 seam).

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::compileroptions::ResolutionModeNone;
use tsc_core::options_generated::{CompilerOptions, ModuleKind};
use tsc_core::scriptkind::ScriptKind;
use tsc_core::tristate::Tristate;
use tsc_parser::parse_source_file;
use tsc_tspath::RootedFilePath;

// ── The single-file Program scaffold (the fcs5-fcs10 idiom) ──

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

/// Check `src` over the single-file scaffold with the corpus defaults for
/// this family (`strict: true` mirroring the conformance harness default;
/// no libs — the corpus seam).
fn check_default(src: &str, module: ModuleKind, strict: bool) -> (Checker, tsc_ast::NodeId) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let options = CompilerOptions {
        module,
        strict_null_checks: if strict { Tristate::TSTrue } else { Tristate::TSUnknown },
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

// ── Pin 1: removeOptionalTypeMarker (the TS2532 optional-chain family) ──

const OPTIONAL_CHAIN_CALLS: &str = "declare const o3: { b: ((...args: any[]) => { c: string }) | undefined };\n\
o3.b?.().c;\n\
o3.b?.(1).c;\n\
o3.b?.()[\"c\"];\n\
o3[\"b\"]?.(1).c;\n";

#[test]
fn optional_chain_call_target_not_possibly_undefined_pin() {
    // checker.go:29593-29611: for `o3.b?.().c` the CALL node is the
    // optional-chain root (it carries the `?.`); as the left of `.c` it
    // takes getOptionalExpressionType's IsOptionalChain arm, whose return
    // type carries the optionalType marker (IS_INNER_CALL_CHAIN's
    // addOptionalTypeMarker). removeOptionalTypeMarker must strip it
    // (strictNullChecks) before checkNonNullType — the former identity
    // stub reported TS2532 on the call node for all 12 corpus shapes.
    // Verified byte-same against the go twin (repro1/k1.ts: 0 2532s).
    let (mut c, file) = check_default(OPTIONAL_CHAIN_CALLS, ModuleKind::None, true);
    let ds = diags(&mut c, file);
    assert!(
        !ds.iter().any(|(code, _)| *code == 2532),
        "optional-call targets must not report TS2532: {ds:?}"
    );
}

// ── Pin 2: the template-literal pattern validators ──

const TEMPLATE_LITERAL_PATTERNS: &str = "declare function numbers(x: `${number}`): void;\n\
numbers(\"1\");\n\
numbers(\"-1.1\");\n\
numbers(\"abc\");\n\
declare function bools(x: `${boolean}`): void;\n\
bools(\"true\");\n\
bools(\"other\");\n\
type RequiresLeadingSlash = `/${string}`;\n\
const a: RequiresLeadingSlash = \"/bin\";\n\
const b: RequiresLeadingSlash = \"no slash\";\n";

#[test]
fn numeric_string_literal_matches_number_pattern_pin() {
    // Go relater.go:2504: `target.flags&TypeFlagsNumber != 0 &&
    // isValidNumberString(value, false)` — the real jsnum validator
    // (utilities.rs), not the former relater-local identity stub. "1" and
    // "-1.1" match `${number}`; "abc" does not; `${boolean}` keeps the
    // boolean-literal name arm. Verified byte-same against the go twin
    // (repro1/k2.ts).
    let (mut c, file) = check_default(TEMPLATE_LITERAL_PATTERNS, ModuleKind::None, true);
    let ds = diags(&mut c, file);
    let non_lib: Vec<(i32, Vec<String>)> = ds
        .into_iter()
        .filter(|(code, _)| *code != 2318)
        .collect();
    let expected = [
        (2345, vec!["\"abc\"".to_string(), "`${number}`".to_string()]),
        (2345, vec!["\"other\"".to_string(), "\"false\" | \"true\"".to_string()]),
        (2322, vec!["\"no slash\"".to_string(), "`/${string}`".to_string()]),
    ];
    assert_eq!(
        non_lib, expected.to_vec(),
        "numeric/bigint strings must pass `${{number}}`; only the true mismatches remain"
    );
}

// ── Pin 3: the 2322 optional-parameter elaboration ──

const OPTIONAL_PARAM_ASSIGN: &str = "var a: (...args: number[]) => number;\n\
a = (x?: number) => 1;\n";

#[test]
fn assignment_error_renders_optional_param_annotation_pin() {
    // Go relater.go:1296 + nodebuilderimpl.go:2253-2360: the arrow's
    // ValueDeclaration (an expression, not context-sensitive) threads as
    // the enclosing for typeToString; serializeTypeForDeclaration's
    // annotation-reuse arm then renders the optional parameter's written
    // annotation — `(x?: number)`, NOT the addOptionalityEx'd symbol type
    // `(x?: number | undefined)`. Verified byte-same against the go twin
    // (repro2/k8.ts).
    let (mut c, file) = check_default(OPTIONAL_PARAM_ASSIGN, ModuleKind::None, true);
    let ds = diags(&mut c, file);
    let e2322: Vec<(i32, Vec<String>)> = ds
        .into_iter()
        .filter(|(code, _)| *code == 2322)
        .collect();
    assert_eq!(e2322.len(), 1, "exactly one 2322: {e2322:?}");
    assert_eq!(
        e2322[0].1,
        vec!["(x?: number) => number".to_string(), "(...args: {}) => number".to_string()],
        "the 2322 args render the written annotation for the optional param"
    );
}

// ── Pin 4: the declare-global augmentation merge ──

const DECLARE_GLOBAL_IMPORT_META: &str = "declare global {\n\
    interface ImportMeta {\n\
        foo?: () => void;\n\
    }\n\
}\n\
export {};\n\
import.meta.foo?.();\n\
import.meta.baz;\n";

#[test]
fn declare_global_augmentation_merges_into_global_scope_pin() {
    // Go checker.go:1449: the `declare global` module augmentation's
    // exports merge into c.globals before the global types finalize, so
    // getGlobalImportMetaType resolves the augmented interface: NO 2318
    // for ImportMeta, `import.meta.foo` checks against the declared
    // optional method, and `import.meta.baz` 2339s against `ImportMeta`
    // (not the `{}` fallback). Verified byte-same against the go twin
    // (repro3/k1.ts).
    let (mut c, file) = check_default(DECLARE_GLOBAL_IMPORT_META, ModuleKind::ES2020, true);
    let ds = diags(&mut c, file);
    assert!(
        !ds.iter().any(|(code, args)| *code == 2318 && args.first().is_some_and(|a| a == "ImportMeta")),
        "the augmented global ImportMeta must resolve: {ds:?}"
    );
    assert!(
        ds.iter().any(|(code, args)| *code == 2339
            && args.first().is_some_and(|a| a == "baz")
            && args.get(1).is_some_and(|a| a == "ImportMeta")),
        "import.meta.baz must 2339 against the merged global ImportMeta: {ds:?}"
    );
    assert!(
        !ds.iter().any(|(code, args)| *code == 2339
            && args.first().is_some_and(|a| a == "foo")),
        "import.meta.foo resolves through the augmentation: {ds:?}"
    );
}

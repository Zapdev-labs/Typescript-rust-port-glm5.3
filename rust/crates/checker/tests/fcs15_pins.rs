//! FCS15 pins (full-corpus check sweep, tranche 15): the TS2345
//! overload-resolution family — the `anyFunctionType` wildcard
//! (relater.go:4478/4481) and `hasCorrectArity`'s too-few-args filter
//! (checker.go:9389).
//!
//! ROOTS (expectations verified byte-same against the go-bench twin's
//! check-stage dump — the rig probe battery, one_k1/k4/k5/k6 repros):
//!
//! 1. `anyFunctionType` helper returned None (an M6-S8 stub), dead-coding
//!    the wildcard arms in `signaturesRelatedTo`: a context-sensitive
//!    function expression's FIRST applicability pass (CheckMode
//!    SkipContextSensitive) types it as anyFunctionType, which Go relates
//!    to EVERY function type; with the stub, the empty anonymous object
//!    failed the walk, so multi-overload calls with function-typed first
//!    parameters resolved against a LATER overload (bogus TS2345
//!    "number -> FuncType", parenthesizedContexualTyping2 a-d) or failed
//!    silently on a single candidate.
//!
//! 2. `hasCorrectArity`'s too-few-args ladder checked the RAW parameter
//!    flags; Go filters with `acceptsVoid` — a non-void parameter filters
//!    to never and fails the candidate. The raw probe accepted every
//!    under-supplied call with non-never params, keeping arity-mismatched
//!    overload candidates in the race.

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::compileroptions::ResolutionModeNone;
use tsc_core::options_generated::CompilerOptions;
use tsc_core::scriptkind::ScriptKind;
use tsc_parser::parse_source_file;
use tsc_tspath::RootedFilePath;

// ── The single-file Program scaffold (the fcs5-fcs14 idiom) ──

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

/// Check `src` over the single-file scaffold (no libs — the corpus seam).
fn check(src: &str) -> (Checker, tsc_ast::NodeId) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
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
    (checker, file)
}

/// The (code, args) list of the file's diagnostics, in order.
fn diags(c: &mut Checker, file: tsc_ast::NodeId) -> Vec<(i32, Vec<String>)> {
    c.get_file_diagnostics(file)
        .iter()
        .map(|d| (d.code(), d.message_args().to_vec()))
        .collect()
}

// ── Pin 1: the anyFunctionType wildcard (relater.go:4478/4481) ──

const WILDCARD_OVERLOAD_CALL: &str = "type FuncType = (x: <T>(p: T) => T) => typeof x;\n\
declare function fun<T>(f: FuncType, x: T): T;\n\
declare function fun<T>(f: FuncType, g: FuncType, x: T): T;\n\
var a = fun(x => { return x; }, 10);\n\
var b = fun(x => { return x; }, x => { return x; }, 10);\n";

#[test]
fn anyfunctiontype_wildcard_first_pass_overload_pin() {
    // The rig k1 repro (PROBES-IDENTICAL after the fix): the call
    // `fun(arrow, 10)` matches the FIRST overload — the arrow's
    // SkipContextSensitive first-pass type is anyFunctionType, which
    // signaturesRelatedTo relates to every function type. With the None
    // stub the walk failed, the call fell through to the second overload
    // and reported a bogus TS2345 "10 -> FuncType" on the numeric
    // argument.
    let (mut c, file) = check(WILDCARD_OVERLOAD_CALL);
    let ds = diags(&mut c, file);
    assert!(
        !ds.iter().any(|(code, _)| *code == 2345 || *code == 2554),
        "the two-argument call matches overload 1 cleanly (wildcard pass): {ds:?}"
    );
}

// ── Pin 2: hasCorrectArity's too-few-args filter (checker.go:9389) ──

const TOO_FEW_ARGS_ARITY: &str = "declare function f(a: number, b: number): void;\n\
declare function f(a: number, b: number, c: number): void;\n\
f(1);\n\
f(1, 2);\n\
f(1, 2, 3);\n";

#[test]
fn has_correct_arity_too_few_args_pin() {
    // A NON-void parameter filters to never (candidate fails the arity
    // ladder); the former raw-flags probe accepted the under-supplied call.
    // `f(1)` fails both candidates on arity -> TS2554 "Expected 2
    // arguments, but got 1"; `f(1, 2)` matches overload 1; `f(1, 2, 3)`
    // matches overload 2 — no TS2345s anywhere.
    let (mut c, file) = check(TOO_FEW_ARGS_ARITY);
    let ds = diags(&mut c, file);
    let arity_errors: Vec<_> = ds
        .iter()
        .filter(|(code, _)| *code == 2554 || *code == 2555)
        .collect();
    assert_eq!(
        arity_errors.len(),
        1,
        "exactly one arity error (the 1-argument call): {ds:?}"
    );
    assert!(
        arity_errors
            .first()
            .is_some_and(|(_, args)| args.first().is_some_and(|a| a == "2-3")),
        "the arity error counts the 2-3 required parameters: {ds:?}"
    );
    assert!(
        !ds.iter().any(|(code, _)| *code == 2345),
        "arity rejection produces no bogus argument errors: {ds:?}"
    );
}

// ── Pin 3: tagged templates are calls with real arguments (checker.go:30636) ──

const TAGGED_TEMPLATE_ARGS: &str = "declare function tag(strs: TemplateStringsArray, x: number): void;\n\
tag `a ${10} b`;\n\
declare function tag2(strs: TemplateStringsArray, ...rest: number[]): void;\n\
tag2 `no subs`;\n\
tag2 `${10}${20}`;\n";

#[test]
fn tagged_template_effective_call_arguments_pin() {
    // The template IS the call's argument list: a synthetic first argument
    // typed TemplateStringsArray plus one argument per substitution span.
    // The former empty stub held every tagged-template call at ZERO
    // arguments — dormant while hasCorrectArity's too-few-args ladder
    // accepted everything; the arity fix surfaced TS2554/2555 on every
    // tagged-template call in the corpus until this arm went Go-exact.
    let (mut c, file) = check(TAGGED_TEMPLATE_ARGS);
    let ds = diags(&mut c, file);
    assert!(
        ds.iter()
            .all(|(code, _)| *code != 2554 && *code != 2555 && *code != 2345),
        "the tagged template calls carry their template/spans as arguments: {ds:?}"
    );
}

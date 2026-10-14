//! FCS20 pins (full-corpus check sweep, tranche 20): the variadic-tuple S8
//! seam — contextual tuple-tail parameters of `[...T, F]`-style rest
//! tuples (the 9 rust-extra TS7006/7019s across contextualTypeTupleEnd 3,
//! variadicTuples2 3, variadicTuples1 2, restTupleElements1 1; the
//! tranche-19 inference fix flipped their within-record direction).
//!
//! ROOT (expectations verified byte-same against the go-bench twin's
//! check-stage dump — the rig probe battery p1_tail/p2_tailrest/p3_ctxend
//! + the 4 full repros, all DIAG-IDENTICAL after the fix):
//!
//! Two Go-exact port gaps, one family:
//! 1. `getContextualTypeForElementExpression`'s tuple arm (checker.go:
//!    30526-30556) was the M6 S9 TODO stub — it answered the fixed
//!    constituent at `index` and ignored `length`/`lastSpreadIndex`,
//!    dropping the ending-fixed arm that contextually types the tuple
//!    TAIL parameters (the arrow at `...args: [...T, F]`'s tail position)
//!    and the variadic slice union for the middle positions.
//! 2. `getStartElementCount`/`getEndElementCount` (utilities.go) were M6
//!    S7 TODO stubs returning 0. `inferFromObjectTypes`'s `endLength`
//!    consumed the stub, so the "middle of target is exactly one variadic
//!    element" arm never fired and T inferred as the empty tuple `{}`
//!    instead of the source slice (e.g. `[string, number]`).

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::compileroptions::ResolutionModeNone;
use tsc_core::options_generated::CompilerOptions;
use tsc_core::scriptkind::ScriptKind;
use tsc_parser::parse_source_file;
use tsc_tspath::RootedFilePath;

// ── The single-file Program scaffold (the fcs5-fcs19 idiom) ──

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

// ── Pin 1: the variadic tuple tail contextually types the arrow parameters ──

const VARIADIC_TUPLE_TAIL_ARROW: &str = "declare function call<T extends unknown[], R>(...args: [...T, (...args: T) => R]): [T, R];\n\
const r1 = call('hello', 32, (a, b) => 42);\n";

#[test]
fn variadic_tuple_tail_arrow_contextually_typed_pin() {
    // The `(...args: [...T, (...args: T) => R])` rest tuple: the tail
    // function element contextually types the arrow — `a: string`,
    // `b: number` from the inferred `T = [string, number]` — so there is
    // NO TS7006 on the parameters and the call RESOLVES (no TS2345, no
    // TS2347). The port's stub tuple arm dropped the ending-fixed arm (the
    // arrow got NO contextual type -> two TS7006s) and the stub
    // getEndElementCount broke the middle-variadic inference (T inferred
    // `{}' -> the whole call failed with a TS2345). (Go twin errors=10 for
    // this file — identical after the fix.)
    let (mut c, file) = check(VARIADIC_TUPLE_TAIL_ARROW);
    let ds = diags(&mut c, file);
    let count_7006 = ds.iter().filter(|(code, _)| *code == 7006).count();
    let count_7019 = ds.iter().filter(|(code, _)| *code == 7019).count();
    let count_2345 = ds.iter().filter(|(code, _)| *code == 2345).count();
    let count_2347 = ds.iter().filter(|(code, _)| *code == 2347).count();
    assert_eq!(count_7006, 0, "the tail arrow's parameters are contextually typed: {ds:?}");
    assert_eq!(count_7019, 0, "no rest-parameter TS7019: {ds:?}");
    assert_eq!(count_2345, 0, "the call resolves against the instantiated rest tuple: {ds:?}");
    assert_eq!(count_2347, 0, "no TS2347: {ds:?}");
}

// ── Pin 2: the tuple tail contextually types the REST parameter ──

const VARIADIC_TUPLE_TAIL_REST_PARAM: &str = "declare function pipe<T extends readonly unknown[]>(...args: [...T, (...values: T) => void]): void;\n\
pipe(\"foo\", 123, true, (...x) => { const q = x; });\n";

#[test]
fn variadic_tuple_tail_rest_param_contextually_typed_pin() {
    // The `(...x)` rest parameter of the tail arrow receives the sliced
    // contextual tuple (`x: [string, number, boolean]` via
    // getRestTypeAtPosition on the contextual signature) — NO TS7019. The
    // stub tuple arm left it `{}`-typed with a TS7019. (Go twin errors=10
    // for this file — identical after the fix.)
    let (mut c, file) = check(VARIADIC_TUPLE_TAIL_REST_PARAM);
    let ds = diags(&mut c, file);
    let count_7019 = ds.iter().filter(|(code, _)| *code == 7019).count();
    let count_2345 = ds.iter().filter(|(code, _)| *code == 2345).count();
    assert_eq!(count_7019, 0, "the rest parameter is contextually typed from the tuple tail: {ds:?}");
    assert_eq!(count_2345, 0, "the call resolves: {ds:?}");
}

// ── Pin 3: the mixed rest/fixed tuple tail (contextualTypeTupleEnd #52846) ──

const MIXED_REST_FIXED_TAIL: &str = "declare function test(...args: [...((arg: number) => void)[], (arg: string) => void]): void;\n\
test(a => a, b => b, c => c);\n";

#[test]
fn mixed_rest_fixed_tail_contextual_typing_pin() {
    // `...args: [...((arg: number) => void)[], (arg: string) => void]`:
    // the trailing FIXED element contextually types the LAST arrow
    // (`c: string`) — the ending-fixed arm of the tuple geometry — and
    // the arrows at the variadic positions get the sliced union
    // contextual signature (`(arg: number) => void`). NO TS7006 on `c`
    // (the stub's `unknown` at tail position produced one), and the call
    // resolves. (Go twin errors=10 for this file — identical after the
    // fix.)
    let (mut c, file) = check(MIXED_REST_FIXED_TAIL);
    let ds = diags(&mut c, file);
    let count_7006 = ds.iter().filter(|(code, _)| *code == 7006).count();
    let count_2345 = ds.iter().filter(|(code, _)| *code == 2345).count();
    assert_eq!(count_7006, 0, "all three arrows are contextually typed: {ds:?}");
    assert_eq!(count_2345, 0, "the call resolves: {ds:?}");
}

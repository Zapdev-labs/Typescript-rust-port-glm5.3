//! FCS18 pins (full-corpus check sweep, tranche 18): the contextual
//! rest-tuple signature instantiation seam — the rest-tuple family of
//! restTuplesFromContextualTypes (Rust 42 vs Go 18 errors, unchanged through
//! tranches 15-17).
//!
//! ROOT (expectations verified byte-same against the go-bench twin's
//! check-stage dump — the rig probe battery, p1_f3 repro + the full
//! restTuplesFromContextualTypes repro, DIAG-IDENTICAL after the fix):
//!
//! Three Go-exact port gaps, one family:
//! 1. `getEffectiveCallArguments` (checker.go:30615) never expanded
//!    spread-of-TUPLE arguments into synthetic per-element arguments
//!    (createSyntheticExpression per element; rest elements -> arrays,
//!    variadic elements carry isSpread) — the arity ladder saw one
//!    argument against fixed-arity candidates and every such call fell
//!    to `getArgumentArityError`'s TS2556 spread fallback.
//! 2. `tryGetTypeAtPosition` (relater.go:1803) restated Go's ORed gate
//!    `!isTuple || Variable || index < fixedLength` as
//!    `!Variable || index < fixedLength` — for variadic rest tuples the
//!    indexed access must run for EVERY index; positions at/past
//!    fixedLength dropped, leaving contextual parameters implicitly any.
//! 3. `sliceTupleType` (relater.go:1912) was an identity seam —
//!    `getEffectiveRestType` handed out the UNSLICED rest tuple, so
//!    contextual rest parameters doubled their fixed elements.

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::compileroptions::ResolutionModeNone;
use tsc_core::options_generated::CompilerOptions;
use tsc_core::scriptkind::ScriptKind;
use tsc_parser::parse_source_file;
use tsc_tspath::RootedFilePath;

// ── The single-file Program scaffold (the fcs5-fcs17 idiom) ──

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

// ── Pin 1: spread-of-tuple calls expand into synthetic element args ──

const SPREAD_TUPLE_CALLS: &str = "declare const t1: [number, boolean, string];\n\
(function (a, b, c) {})(...t1);\n\
(function (a, b, c, ...x) {})(...t1);\n\
declare const t2: [number, boolean, ...string[]];\n\
(function (a, b, c) {})(...t2);\n\
declare const t3: [boolean, ...string[]];\n\
(function (a, b, c) {})(1, ...t3);\n\
(function (a, b, c, ...x) {})(1, ...t3);\n";

#[test]
fn effective_call_arguments_spread_tuple_expansion_pin() {
    // A spread argument with a TUPLE type becomes one synthetic argument
    // per tuple element (variadic elements keep isSpread so they still
    // satisfy a rest tail), so the arity ladder counts the ELEMENTS and
    // none of these calls may report TS2556 ("a spread argument must
    // either have a tuple type or be passed to a rest parameter") or
    // TS2554/2555 arity errors. The unexpanded pass-through fired TS2556
    // on every spread-of-tuple call above.
    let (mut c, file) = check(SPREAD_TUPLE_CALLS);
    let ds = diags(&mut c, file);
    assert!(
        !ds.iter().any(|(code, _)| *code == 2556 || *code == 2554 || *code == 2555),
        "spread-of-tuple calls resolve against the expanded element arity: {ds:?}"
    );
}

// ── Pin 2: contextual parameters typed from a variadic rest tuple ──

const CONTEXTUAL_REST_TUPLE_PARAMS: &str = "declare const t3: [boolean, ...string[]];\n\
declare function f3(cb: (x: number, ...args: typeof t3) => void): void;\n\
f3((a, b, c) => { c.charCodeAt(0); });\n";

#[test]
fn contextual_rest_tuple_parameter_typing_pin() {
    // tryGetTypeAtPosition's gate is Go's disjunction — for a rest TUPLE
    // with a variadic element the indexed access runs for EVERY index, so
    // `c` (position 2, at the variadic tail `...string[]`) is contextually
    // typed `string`, NOT implicitly any. The inverted `within_bounds`
    // restatement dropped positions at/past fixedLength, leaving `c` any
    // (TS7006 "parameter c implicitly has an any type") and breaking the
    // member access.
    // ... (the no-lib scaffold additionally reports the go-parity TS2345
    // and a no-member-on-string TS2339 for charCodeAt — the twin's
    // artifacts, not this seam).
    let (mut c, file) = check(CONTEXTUAL_REST_TUPLE_PARAMS);
    let ds = diags(&mut c, file);
    assert!(
        !ds.iter().any(|(code, _)| *code == 7006),
        "the variadic-tail contextual parameter is typed string: {ds:?}"
    );
}

// ── Pin 3: the contextual rest parameter's SLICED tuple ──

const CONTEXTUAL_REST_PARAM_SLICE: &str = "declare const t3: [boolean, ...string[]];\n\
declare function f3(cb: (x: number, ...args: typeof t3) => void): void;\n\
f3((...x) => {});\n\
f3((a, ...x) => {});\n\
f3((a, b, ...x) => {});\n";

#[test]
fn contextual_rest_parameter_sliced_tuple_pin() {
    // The arrow's own rest parameter gets getRestTypeAtPosition(context,
    // length) — the fixed elements come from the context's positions and
    // the LAST element carries getEffectiveRestType's SLICED rest tuple
    // (sliceTupleType drops the fixed prefix; the identity seam returned
    // the full tuple, so the variadic element spread [boolean, ...string[]]
    // and DOUBLED the fixed boolean — "(x_0: number, x_1: boolean, x_2:
    // boolean, ...x_3)" — failing the applicability relation with a bogus
    // TS2345 on the first three calls).
    let (mut c, file) = check(CONTEXTUAL_REST_PARAM_SLICE);
    let ds = diags(&mut c, file);
    assert!(
        !ds.iter().any(|(code, _)| *code == 2345),
        "the contextually typed rest parameters relate cleanly: {ds:?}"
    );
}

// ── Pin 4: the go-parity 2345 on the beyond-arity rest arrow ──

const CONTEXTUAL_REST_PARAM_BEYOND: &str = "declare const t3: [boolean, ...string[]];\n\
declare function f3(cb: (x: number, ...args: typeof t3) => void): void;\n\
f3((a, b, c, ...x) => {});\n";

#[test]
fn contextual_rest_parameter_beyond_arity_pin() {
    // The fourth parameter `c` (position 2) is fine (string from the
    // variadic tail), but the arrow's OWN rest `...x` lands past the
    // context's arity — getRestTypeAtPosition(context, 3) with
    // parameterCount 3 takes the indexed-access arm (string[][]) and the
    // relation fails exactly like the go twin: ONE TS2345, message target
    // "(x: number, ...args: typeof t3)"-shaped. (Go twin errors=1 for this
    // file.)
    let (mut c, file) = check(CONTEXTUAL_REST_PARAM_BEYOND);
    let ds = diags(&mut c, file);
    let count_2345 = ds.iter().filter(|(code, _)| *code == 2345).count();
    assert_eq!(count_2345, 1, "exactly the go twin's TS2345: {ds:?}");
}

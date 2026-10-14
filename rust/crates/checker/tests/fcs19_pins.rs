//! FCS19 pins (full-corpus check sweep, tranche 19): the
//! parenthesizedContexualTyping2 sub-shape B family — failed-overload
//! contextual typing of rest-position arrows + the arrow/object-literal
//! assignment-error elaboration (Rust 45 vs Go 50 errors, noted since
//! fcs14).
//!
//! ROOT (expectations verified byte-same against the go-bench twin's
//! check-stage dump — the rig probe battery, p1_kline/p2_obj/p4 probes +
//! the full parenthesizedContexualTyping2 repro, DIAG-IDENTICAL after the
//! fix):
//!
//! Two Go-exact port gaps, one family:
//! 1. `reportCallResolutionErrors` was missing Go's
//!    `addImplementationSuccessElaboration` (checker.go:9917) — the re-run
//!    of chooseOverload on the overload IMPLEMENTATION signature (+
//!    TS2793 related info on success) — and `isSignatureApplicable`'s rest
//!    arm was the S8 seam (comparing the rest type to itself instead of
//!    calling `getSpreadArgumentType`, checker.go:9536). Go's elaboration
//!    pass eagerly re-checks every rest-position argument with the
//!    implementation's anyType rest type as contextual type; that push is
//!    what the deferred parameter typing of those arrows sees, so their
//!    parameters resolve to implicit any (TS7006) and the untyped call
//!    reports TS2347. The port contextually typed them from the failed
//!    call's longest candidate instead (spurious TS2345s).
//! 2. `elaborateArrowFunction` (relater.go:639) and
//!    `elaborateObjectLiteral` (relater.go:496) were S4 stubs, and
//!    `elaborateError`'s parenthesized/as/jsx and `=`/`,` binary
//!    recursion arms were seams. Go elaborates the failed assignability
//!    into per-return-type TS2322s on the arrow body (comma/ternary
//!    bodies included), the TS6502 expected-type related info, and the
//!    async Promise hint (`createPromiseType` — the global Promise
//!    lookup the go twin reports on lib-less corpora).

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::compileroptions::ResolutionModeNone;
use tsc_core::options_generated::CompilerOptions;
use tsc_core::scriptkind::ScriptKind;
use tsc_parser::parse_source_file;
use tsc_tspath::RootedFilePath;

// ── The single-file Program scaffold (the fcs5-fcs18 idiom) ──

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

// ── Pin 1: the failed-overload rest-position arrow is NOT contextually typed ──

const FAILED_OVERLOAD_REST_ARROW: &str = "type FuncType = (x: <T>(p: T) => T) => typeof x;\n\
function fun<T>(f: FuncType, g: FuncType, x: T): T;\n\
function fun<T>(...rest: any[]): T { return undefined; }\n\
var k = fun((x => undefined), x => { x<number>(undefined); return x; }, 10);\n";

#[test]
fn failed_overload_rest_arrow_not_contextually_typed_pin() {
    // The call fails on the first argument, so candidate 2's second
    // parameter never contextually types the second arrow. Go's
    // addImplementationSuccessElaboration then re-runs chooseOverload on
    // the implementation signature `(...rest: any[])`, and the rest arm's
    // getSpreadArgumentType pass checks the arrow with an anyType
    // contextual type — the parameter resolves to implicit any (ONE
    // TS7006 "parameter x implicitly has an any type") and the type-
    // argument call reports TS2347. The port used to contextually type x
    // as `<T>(p: T) => T` from the failed call's longest candidate and
    // emit a spurious TS2345 ("argument of type 'undefined' is not
    // assignable to parameter of type 'number'") instead. The first
    // argument's failure is ELABORATED onto the arrow's return expression
    // (a TS2322 "type 'undefined' is not assignable to type '<T>(p: T)
    // => T'"), NOT a whole-argument TS2345. (Go twin errors=15 for this
    // file.)
    let (mut c, file) = check(FAILED_OVERLOAD_REST_ARROW);
    let ds = diags(&mut c, file);
    let count_7006 = ds.iter().filter(|(code, _)| *code == 7006).count();
    let count_2347 = ds.iter().filter(|(code, _)| *code == 2347).count();
    let count_2345 = ds.iter().filter(|(code, _)| *code == 2345).count();
    assert_eq!(count_7006, 1, "exactly the go twin's TS7006 on the rest-position arrow: {ds:?}");
    assert_eq!(count_2347, 1, "exactly the go twin's TS2347 on the untyped type-argument call: {ds:?}");
    assert_eq!(count_2345, 0, "no whole-argument TS2345 — the failure is elaborated onto the return: {ds:?}");
    assert!(
        ds.iter().any(|(code, args)| {
            *code == 2322 && args.iter().any(|a| a.contains("<T>(p: T) => T"))
        }),
        "the first argument's failure is elaborated to the return-expression TS2322: {ds:?}"
    );
}

// ── Pin 2: the ternary-bodied first argument keeps the head 2345, the
// second arrow still gets the anyType rest pass ──

const TERNARY_AND_REST_ARROW: &str = "type FuncType = (x: <T>(p: T) => T) => typeof x;\n\
function fun<T>(f: FuncType, g: FuncType, x: T): T;\n\
function fun<T>(...rest: any[]): T { return undefined; }\n\
var k = fun((Math.random() < 0.5 ? (x => { x<number>(undefined); return x; }) : (x => undefined)), x => { x<number>(undefined); return x; }, 10);\n";

#[test]
fn ternary_first_arg_head_and_rest_arrow_any_pin() {
    // The parenthesizedContexualTyping2 `var k` shape. The ternary first
    // argument is checked WITH its contextual type (the true branch's
    // arrow IS contextually typed — its `x<number>(undefined)` reports
    // TS2345 undefined→number), but the whole ternary fails FuncType with
    // the UNELABORATED head TS2345 (conditional expressions have no
    // declaration to elaborate onto). The SECOND arrow is past the
    // failing argument, so only the implementation-elaboration anyType
    // pass reaches it: TS7006 + TS2347, never a contextual TS2345.
    // (Go twin errors=16 for this file.)
    let (mut c, file) = check(TERNARY_AND_REST_ARROW);
    let ds = diags(&mut c, file);
    let count_7006 = ds.iter().filter(|(code, _)| *code == 7006).count();
    let count_2347 = ds.iter().filter(|(code, _)| *code == 2347).count();
    let count_2345_head = ds
        .iter()
        .filter(|(code, args)| {
            *code == 2345
                && args.iter().any(|a| a.contains("FuncType"))
                && args.iter().any(|a| a.contains("=> undefined"))
        })
        .count();
    let count_2345_undef_number = ds
        .iter()
        .filter(|(code, args)| *code == 2345 && args.len() == 2 && args[0] == "undefined" && args[1] == "number")
        .count();
    assert_eq!(count_7006, 1, "exactly the go twin's TS7006 on the second arrow: {ds:?}");
    assert_eq!(count_2347, 1, "exactly the go twin's TS2347 on the second arrow: {ds:?}");
    assert_eq!(count_2345_head, 1, "the ternary keeps its unelaborated head TS2345: {ds:?}");
    assert_eq!(
        count_2345_undef_number, 1,
        "the ternary's true branch reports its one contextual TS2345: {ds:?}"
    );
    assert!(
        !ds.iter().any(|(code, args)| *code == 7006 && args.first().is_some_and(|a| a != "x")),
        "no spurious TS7006 beyond the go twin's: {ds:?}"
    );
}

// ── Pin 3: the comma-bodied arrow's return is elaborated per-property ──

const OBJ_LITERAL_COMMA_ARROW: &str = "type ObjType = { x: (p: number) => string; y: (p: string) => number };\n\
var obj1: ObjType = { x: x => (x, undefined), y: y => (y, undefined) };\n";

#[test]
fn object_literal_comma_arrow_return_elaboration_pin() {
    // The assignment failure is elaborated per property through
    // elaborateObjectLiteral → elaborateElement → elaborateError →
    // elaborateArrowFunction: each comma-bodied arrow's return expression
    // reports its own TS2322 (undefined→string for x, undefined→number
    // for y), with the comma-operator recursion reaching the RIGHT operand
    // and the TS6502 expected-type related info attached upstream. The
    // port's stub reported ONE whole-literal TS2322 (the object literal's
    // type vs ObjType) and nothing per property. (Go twin errors=15 for
    // this file.)
    let (mut c, file) = check(OBJ_LITERAL_COMMA_ARROW);
    let ds = diags(&mut c, file);
    let count_2322_undef_string = ds
        .iter()
        .filter(|(code, args)| *code == 2322 && args.len() == 2 && args[0] == "undefined" && args[1] == "string")
        .count();
    let count_2322_undef_number = ds
        .iter()
        .filter(|(code, args)| *code == 2322 && args.len() == 2 && args[0] == "undefined" && args[1] == "number")
        .count();
    let count_2322_whole_literal = ds
        .iter()
        .filter(|(code, args)| *code == 2322 && args.iter().any(|a| a.contains("ObjType")))
        .count();
    let count_2695 = ds.iter().filter(|(code, _)| *code == 2695).count();
    assert_eq!(count_2322_undef_string, 1, "the x property's return elaborates its TS2322: {ds:?}");
    assert_eq!(count_2322_undef_number, 1, "the y property's return elaborates its TS2322: {ds:?}");
    assert_eq!(count_2322_whole_literal, 0, "no whole-literal TS2322 — the elaboration replaces it: {ds:?}");
    assert_eq!(count_2695, 2, "both comma operators report TS2695: {ds:?}");
}

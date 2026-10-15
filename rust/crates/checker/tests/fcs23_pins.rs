//! FCS23 pins (full-corpus check sweep, tranche 23): the declared
//! type-predicate resolution seam — the assertionTypePredicates1 missing-2339
//! cluster (controlFlow/assertionTypePredicates1.ts, 11 missing 2339
//! instances + 7027 x5 + 7053 x2 + 2775 x3 and 13 rust-extra
//! 18046/18048/2339 narrowing shapes, all on ONE record).
//!
//! ROOT (expectations verified byte-same against the go-bench twin's
//! check-stage dump — the rig probe assertionTypePredicates1 both-drivers,
//! REPRO-IDENTICAL after the fix):
//!
//! Go's `getTypePredicateOfSignature` (relater.go:2048) resolves a
//! DECLARED predicate through the signature declaration's return-type
//! annotation: a TypePredicateNode goes to
//! `createTypePredicateFromTypePredicateNode` (relater.go:2117), which reads
//! the asserts modifier, the parameter name, and the parameter's index in
//! the signature's parameter list (`core.FindIndex`, -1 when the name
//! matches no parameter). The port's declaration arm was a TODO stub, so
//! EVERY declared `asserts value` / `value is T` / `this is T` /
//! `asserts this is T` signature resolved to noTypePredicate — which kept
//! `getEffectsSignature`, `getTypeAtFlowCall`, `narrowTypeByCallExpression`,
//! the FlowCall reachability arm, and checkCallExpression's assertion-call
//! validation (checker.go:8560-8566, the TS2775/TS2776 arm) all dead.
//!
//! FIX: the declaration arm now runs the Go ladder (the un-annotated
//! function-like body-inference arm — Go's `getTypePredicateFromBody`,
//! checker.go:20922 — stays a future tranche and keeps Go's pre-body
//! noTypePredicate marker); `create_type_predicate_from_type_predicate_node`
//! is ported; the checkCallExpression TS2775/TS2776 validation is ported
//! (its TS2782 related-info seam — `"'X' needs an explicit type
//! annotation"` threaded through `getTypeOfDottedName` — is unported and
//! noted at the call site).
//!
//! The unreachable-code spans additionally needed Go's
//! `scanner.GetTokenPosOfNode` (scanner.go:2526) — the port's stub returned
//! the raw node pos, so an ExpressionStatement's leading line break + indent
//! leaked into the TS7027 span (5 records' spans started 9 bytes early).
//!
//! The pins run WITHOUT libs (the corpus twin seam): `length` on `string`
//! reports 2339 there, which is what makes the narrowing observable.

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::compileroptions::ResolutionModeNone;
use tsc_core::options_generated::CompilerOptions;
use tsc_core::scriptkind::ScriptKind;
use tsc_core::tristate::Tristate;
use tsc_parser::parse_source_file;
use tsc_tspath::RootedFilePath;

// ── The single-file Program scaffold (the fcs22 idiom, TS flavor) ──

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

/// Check `src` (a TypeScript unit, no libs — the corpus seam) with
/// noImplicitAny (the conformance harness configuration the twin drivers
/// run).
fn check_ts(src: &str) -> (Checker, tsc_ast::NodeId) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let options = CompilerOptions {
        no_implicit_any: Tristate::TSTrue,
        // The corpus family's config: controlFlow/assertionTypePredicates1.ts
        // runs `@allowUnreachableCode: false` — the TS7027 pin needs the
        // unreachable check to report errors.
        allow_unreachable_code: Tristate::TSFalse,
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
    checker.check_source_file(file, true);
    (checker, file)
}

/// The (code, args) list of the file's diagnostics, in order.
fn diags(c: &mut Checker, file: tsc_ast::NodeId) -> Vec<(i32, Vec<String>)> {
    c.get_file_diagnostics(file)
        .iter()
        .map(|d| (d.code(), d.message_args().to_vec()))
        .collect()
}

// ── Pin 1: the f02 shape — a declared `asserts value` call in an
// expression statement narrows the argument reference by truthiness, so the
// later `x.length` reads `string` (2339 without libs) instead of reporting
// TS18048 "x is possibly undefined" ──

const ASSERTS_VALUE_NARROWING: &str = "declare function assert(value: unknown): asserts value;\n\
function f(x: string | undefined) {\n\
    assert(x);\n\
    x.length;\n\
}\n";

#[test]
fn asserts_value_call_narrows_reference_by_truthiness_pin() {
    // Pre-fix: the declared predicate never resolved, the FlowCall node had
    // no effect, and `x` stayed `string | undefined` — TS18048 on the
    // `x.length` reference. Post-fix (createTypePredicateFromTypePredicate
    // Node → getEffectsSignature → getTypeAtFlowCall → narrowTypeByAssertion
    // → narrowTypeByTruthiness): `x` narrows to `string`, and `x.length`
    // reports 2339 ('length' on 'string' — the no-libs corpus seam).
    // Verified byte-same against the go-bench twin (rig probe f23p1).
    let (mut c, file) = check_ts(ASSERTS_VALUE_NARROWING);
    let ds = diags(&mut c, file);
    let e2339: Vec<&Vec<String>> = ds
        .iter()
        .filter(|(code, _)| *code == 2339)
        .map(|(_, args)| args)
        .collect();
    assert_eq!(e2339.len(), 1, "one narrowed x.length: {ds:?}");
    assert_eq!(e2339[0][0], "length", "{ds:?}");
    assert_eq!(e2339[0][1], "string", "{ds:?}");
    assert!(
        !ds.iter().any(|(code, _)| *code == 18048),
        "no possibly-undefined after the assert: {ds:?}"
    );
    assert!(
        !ds.iter().any(|(code, _)| *code == 2775),
        "an explicitly annotated declared function is a valid assertion target: {ds:?}"
    );
}

// ── Pin 2: the Test/f11 shape — `this is T` in an if condition AND
// `asserts this is T` in an expression statement both narrow the receiver,
// so `item.z` (z declared on the subclass only) resolves ──

const PREDICATE_RECEIVER_NARROWING: &str = "class Test {\n\
    isTest2(): this is Test2 { return this instanceof Test2; }\n\
    assertIsTest2(): asserts this is Test2 {\n\
        if (this instanceof Test2) return;\n\
        throw undefined;\n\
    }\n\
}\n\
class Test2 extends Test { z = 0; }\n\
function f11(item: Test) {\n\
    if (item.isTest2()) { item.z; }\n\
    item.assertIsTest2();\n\
    item.z;\n\
}\n";

#[test]
fn this_and_asserts_this_predicates_narrow_receiver_pin() {
    // Pre-fix: `item` stayed `Test` at both `item.z` reads — 2339
    // ('z' on 'Test') twice. Post-fix: the if-condition narrows through
    // narrowTypeByCallExpression and the expression-statement call through
    // getTypeAtFlowCall, both to `Test2`, where `z` exists. Verified
    // byte-same against the go-bench twin (rig probe f23p2).
    let (mut c, file) = check_ts(PREDICATE_RECEIVER_NARROWING);
    let ds = diags(&mut c, file);
    assert!(
        !ds.iter()
            .any(|(code, args)| *code == 2339 && args.first() == Some(&"z".to_string())),
        "no z-on-Test: both item.z reads narrow to Test2: {ds:?}"
    );
}

// ── Pin 3: the f01 shape — `assert(false)` in an expression statement
// makes the following statement unreachable (TS7027), and the diagnostic
// spans exactly the statement (Go's GetTokenPosOfNode skips the leading
// line break + indent out of the ExpressionStatement's raw pos) ──

const ASSERTS_FALSE_UNREACHABLE: &str = "declare function assert(value: unknown): asserts value;\n\
function f(x: unknown) {\n\
    assert(false);\n\
    x;\n\
}\n";

#[test]
fn asserts_false_makes_following_statement_unreachable_pin() {
    // Pre-fix: the FlowCall reachability arm never saw the assertsIdentifier
    // predicate (no TS7027 at all). Post-fix: isReachableFlowNodeWorker's
    // FlowCall arm matches assertsIdentifier with parameterIndex 0 whose
    // argument is `false` → unreachable → 7027; and the span is
    // [getTokenPosOfNode(stmt), stmt.End()) — the raw-pos stub leaked the
    // leading "\n    " (9 bytes) into the start. Verified against the
    // go-bench twin's full-corpus record (controlFlow/assertionTypePredicates
    // 1.ts: the five 7027 spans are byte-identical post-fix, each covering
    // exactly the `x;` statement; the default-options mini probe stays
    // silent because 7027 needs @allowUnreachableCode: false, which this
    // scaffold sets).
    let (mut c, file) = check_ts(ASSERTS_FALSE_UNREACHABLE);
    let stmt_pos = ASSERTS_FALSE_UNREACHABLE
        .rfind("x;")
        .expect("the x; statement is in the source")
        as i32;
    let seventy_twenty_sevens: Vec<tsc_ast::Diagnostic> = c
        .get_file_diagnostics(file)
        .iter()
        .filter(|d| d.code() == 7027)
        .cloned()
        .collect();
    assert_eq!(seventy_twenty_sevens.len(), 1, "one unreachable report");
    let d = &seventy_twenty_sevens[0];
    assert_eq!(d.pos(), stmt_pos, "the span starts at the x; token, not the raw pos");
    assert_eq!(d.end(), stmt_pos + 2, "the span covers exactly the x; statement");
}

// ── Pin 4: the f20 shape — an UN-annotated assert call target in an
// expression statement reports TS2775, and the predicate must NOT narrow
// (no over-apply): the later `x.length` still reads `unknown` (TS18046),
// never `string` ──

const UNANNOTATED_ASSERT_TARGET: &str = "function f20(x: unknown) {\n\
    const assert = (value: unknown): asserts value => {};\n\
    assert(typeof x === \"string\");\n\
    x.length;\n\
}\n";

#[test]
fn unannotated_assert_target_reports_2775_and_does_not_narrow_pin() {
    // The 2775 validation (checker.go:8560-8566) fires when the call target
    // is dotted but getEffectsSignature cannot resolve it through
    // getTypeOfDottedName — the arrow const has no explicit type annotation.
    // The effects signature stays nil, so the FlowCall node has NO narrowing
    // effect: `x.length` still reports 18046 (x reads unknown), pinning the
    // no-over-apply guard. Verified byte-same against the go-bench twin
    // (rig probe f23p4: 2775 + 18046, nothing else).
    let (mut c, file) = check_ts(UNANNOTATED_ASSERT_TARGET);
    let ds = diags(&mut c, file);
    assert!(
        ds.iter().any(|(code, _)| *code == 2775),
        "the un-annotated call target reports 2775: {ds:?}"
    );
    assert!(
        ds.iter().any(|(code, _)| *code == 18046),
        "no narrowing without an explicit annotation: x stays unknown: {ds:?}"
    );
    assert!(
        !ds.iter().any(|(code, _)| *code == 2339),
        "no 2339: x was never narrowed to string: {ds:?}"
    );
}

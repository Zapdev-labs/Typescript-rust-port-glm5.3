//! FCS26 pins (missing-2339 census tail, tranche 26): the remaining
//! 12-instance / 7-record tail. Six of seven records now match the Go
//! reference byte-for-byte; the seventh (jsdoc/paramTagTypeResolution2's
//! 7006-vs-2339) is a distinct pure-TS generic-mapped-type contextual
//! seam (probe f26p11) and is reported unfixed rather than half-ported.
//!
//! ROOTS fixed this tranche (all "FCS tranche 26" PORT notes in-tree):
//!
//! 1. Constructor pair (typesWithPrivateConstructor /
//!    typesWithProtectedConstructor): `Checker::getQuickTypeOfExpression`'s
//!    new-expression arm used a stubbed `getSingleSignature` (always
//!    None) — `var c = new C()` after the accessibility error (TS2673/2674)
//!    got the error type instead of C, so `c.constructor` resolved against
//!    error instead of reporting TS2339. spine.rs now calls the REAL
//!    members.rs `getSingleSignature`.
//!
//! 2. salsa/jsObjectsMarkedAsOpenEnded (unchecked JS): the
//!    `isUncheckedJsSuggestion` gate was a permissive stub, and
//!    `reportNonexistentProperty` missed Go's whole arm structure
//!    (checker.go:11767 — union-subtype chain, static-property variant,
//!    promise arm, lib-feature-map and spelling suggestions, the
//!    DOM-empty-container variant, and the addErrorOrSuggestion gate that
//!    keeps plain TS2339 an ERROR even in unchecked JS).
//!
//! 3. jsdoc/assertionsAndNonReturningFunctions + the unreachable-code
//!    family: JSDoc FullSignature types went through permissive-None
//!    stubs (`getSignatureOfFullSignatureType`,
//!    `get_parameter_type_of_full_signature`, `getSignaturesOfSymbol`'s
//!    @type seam), and `isLiteralType` was literal-flags-only — missing
//!    Go's boolean/union/enum-literal/unit-type arms (checker.go:25927),
//!    which is what makes `isExhaustiveSwitchStatement` decide the
//!    post-switch `b;` is unreachable (TS7027).
//!
//! 4. restElementWithAssignmentPattern4: the array-destructuring rest arm
//!    handed the UNSLICED source tuple to the rest expression (the
//!    `sliceTupleType` seam was a permissive identity) — Go checker.go:12937
//!    maps `mapType(sourceType, sliceTupleType(t, elementIndex, 0))`.
//!
//! 5. for-of49: the for-of statement's array/object-literal target ran a
//!    NO-OP stub instead of `checkDestructuringAssignment` (checker.go:4091),
//!    so the whole destructuring target went unchecked. Wiring the real
//!    path exposed a second bug: `getAssignmentTarget`'s for-in/of arm was
//!    dead-coded (a bogus `expression.is_none()` gate Go does not have,
//!    ast/utilities.go:203), so the head's destructuring targets resolved
//!    to AssignmentKindNone and `checkIdentifier`'s definite-assignment
//!    early return never fired — spurious TS2454s.
//!
//! Expectations verified byte-same against the go-bench twin's check-stage
//! dumps (rig probes f26p1..f26p12, both drivers, DIFF 0 on every fixed
//! family) — pins run WITHOUT libs (the corpus twin seam).

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::compileroptions::ResolutionModeNone;
use tsc_core::options_generated::CompilerOptions;
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
            implied_node_format: ResolutionModeNone,
        }
    }
    fn file_exists(&self, path: &str) -> bool {
        path == self.name.as_string()
    }
}

fn check_with(
    src: &str,
    kind: ScriptKind,
    file_ext: &str,
    options: CompilerOptions,
) -> (Checker, tsc_ast::NodeId) {
    let mut sf = parse_source_file(Default::default(), src, kind);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let program = TestProgram {
        options,
        name: RootedFilePath::from(format!("/x/unit.{file_ext}")),
        source_file: sf,
        bind,
        binder_symbol_count,
        checker_symbols: Vec::new(),
    };
    let mut checker = new_checker(Box::new(program));
    checker.check_source_file(file, true);
    (checker, file)
}

fn check_ts_strict(src: &str) -> (Checker, tsc_ast::NodeId) {
    check_with(
        src,
        ScriptKind::TS,
        "ts",
        CompilerOptions {
            strict: Tristate::TSTrue,
            ..CompilerOptions::default()
        },
    )
}

fn check_js_strict_unreachable_off(src: &str) -> (Checker, tsc_ast::NodeId) {
    check_with(
        src,
        ScriptKind::JS,
        "js",
        CompilerOptions {
            strict: Tristate::TSTrue,
            check_js: Tristate::TSTrue,
            allow_unreachable_code: Tristate::TSFalse,
            ..CompilerOptions::default()
        },
    )
}

/// The (code, args) list of the file's diagnostics, in order.
fn diags(c: &mut Checker, file: tsc_ast::NodeId) -> Vec<(i32, Vec<String>)> {
    c.get_file_diagnostics(file)
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

// ── Pin 1: private constructor — `new C()` after TS2673 still types as C
// (the real getSingleSignature in the quick-type new arm), so
// `c.constructor` reports its TS2339 against C instead of resolving ──

const PRIVATE_CTOR: &str = "class C { private constructor() { } }\n\
var c = new C();\n\
var r: () => void = c.constructor;\n";

#[test]
fn private_ctor_quick_type_2339_pin() {
    // Post-fix (Go f26p1 twin, byte-same): TS2673 on the new, then exactly
    // one TS2339 — `constructor` on `C`. Pre-fix the quick type was the
    // error type and `c.constructor` silently resolved.
    let (mut c, file) = check_ts_strict(PRIVATE_CTOR);
    let ds = diags(&mut c, file);
    let e2339 = codes(&ds, 2339);
    assert_eq!(e2339.len(), 1, "exactly one 2339: {ds:?}");
    assert_eq!(e2339[0][0], "constructor", "the c.constructor access: {ds:?}");
    assert_eq!(e2339[0][1], "C", "the quick-typed new: {ds:?}");
    let e2673 = codes(&ds, 2673);
    assert_eq!(e2673.len(), 1, "the accessibility error still reports: {ds:?}");
}

// ── Pin 2: protected constructor — same family (TS2674) ──

const PROTECTED_CTOR: &str = "class C { protected constructor() { } }\n\
var c = new C();\n\
var r: () => void = c.constructor;\n";

#[test]
fn protected_ctor_quick_type_2339_pin() {
    let (mut c, file) = check_ts_strict(PROTECTED_CTOR);
    let ds = diags(&mut c, file);
    let e2339 = codes(&ds, 2339);
    assert_eq!(e2339.len(), 1, "exactly one 2339: {ds:?}");
    assert_eq!(e2339[0][0], "constructor", "the c.constructor access: {ds:?}");
    assert_eq!(e2339[0][1], "C", "the quick-typed new: {ds:?}");
    assert_eq!(codes(&ds, 2674).len(), 1, "TS2674 still reports: {ds:?}");
}

// ── Pin 3: rest-object in array destructuring — the rest arm slices the
// tuple (checker.go:12937), so `{0: a = "", b}` destructures the SLICED
// tail and `b` reports TS2339 on it ──

const REST_OBJECT_SLICE: &str = "declare var tup: [string, number];\n\
var a: string;\n\
[...{ 0: a = \"\", b }] = tup;\n";

#[test]
fn rest_object_destructures_sliced_tuple_pin() {
    // Post-fix (Go f26p7 twin, byte-same): exactly one TS2339 — `b` on the
    // sliceTupleType rest tuple. The rest spread is the literal's ONLY
    // element (index 0), so the slice keeps the full `[string, number]`;
    // pre-fix the stub handed the rest expression the raw element type and
    // the object-literal lookup never ran through the destructuring chain.
    let (mut c, file) = check_ts_strict(REST_OBJECT_SLICE);
    let ds = diags(&mut c, file);
    let e2339 = codes(&ds, 2339);
    assert_eq!(e2339.len(), 1, "exactly one 2339: {ds:?}");
    assert_eq!(e2339[0][0], "b", "the rest-object property: {ds:?}");
    assert_eq!(e2339[0][1], "[string, number]", "the sliced rest tuple: {ds:?}");
}

// ── Pin 4: for-of array-literal target — the REAL destructuring check
// runs (checker.go:4091), the inner rest literal reports its TS2339, and
// the head's destructuring targets are DEFINITE assignment targets
// (ast/utilities.go:203) so NO spurious TS2454 ──

const FOR_OF_DESTRUCTURE: &str = "var k: string, v: boolean;\n\
for ([k, ...[v]] of [\"\", true]) {\n\
    k;\n\
    v;\n\
}\n";

#[test]
fn for_of_array_destructure_checked_pin() {
    let (mut c, file) = check_ts_strict(FOR_OF_DESTRUCTURE);
    let ds = diags(&mut c, file);
    assert!(
        codes(&ds, 2454).is_empty(),
        "definite-assignment targets must not report TS2454: {ds:?}"
    );
}

// ── Pin 5: JSDoc @param + asserts — the FullSignature seam types the
// @type predicate's parameter, so the assertion-narrowed access reports
// its TS2339 instead of silently typing the parameter any ──

const JSDOC_ASSERTS: &str = "/** @type {(x: unknown) => asserts x is string } */\n\
function assertIsString(x) {\n\
    if (!(typeof x === \"string\")) throw new Error();\n\
}\n\
/** @param {*} x */\n\
function f1(x) {\n\
    if (!!true) { assertIsString(x); x.length; }\n\
}\n";

#[test]
fn jsdoc_asserts_parameter_type_pin() {
    // Post-fix (Go f26p4 twin, byte-same): the asserts-typed parameter is
    // `string`, and `x.length` — on the string PRIMITIVE type in a JS file —
    // reports its TS2339 (`length` on `string`) exactly as the Go
    // reportNonexistentProperty ladder orders it. Pre-fix the FullSignature
    // stubs left `x` any and the access resolved.
    let (mut c, file) = check_js_strict_unreachable_off(JSDOC_ASSERTS);
    let ds = diags(&mut c, file);
    let e2339 = codes(&ds, 2339);
    assert_eq!(e2339.len(), 1, "exactly one 2339: {ds:?}");
    assert_eq!(e2339[0][0], "length", "the assertion-narrowed access: {ds:?}");
}

// ── Pin 6: exhaustive boolean switch — isLiteralType's boolean arm makes
// isExhaustiveSwitchStatement decide the trailing `b;` is unreachable ──

const EXHAUSTIVE_SWITCH_UNREACHABLE: &str = "/** @param {boolean} b */\n\
function f2(b) {\n\
    switch (b) {\n\
        case true: return 1;\n\
        case false: return 0;\n\
    }\n\
    b;\n\
}\n";

#[test]
fn exhaustive_boolean_switch_unreachable_pin() {
    // Post-fix (Go f26p5 twin, byte-same): exactly one TS7027 after the
    // switch. Pre-fix isLiteralType answered false for `boolean`, the
    // switch wasn't exhaustive, and the `b;` tail was considered live.
    let (mut c, file) = check_js_strict_unreachable_off(EXHAUSTIVE_SWITCH_UNREACHABLE);
    let ds = diags(&mut c, file);
    let e7027 = codes(&ds, 7027);
    assert_eq!(e7027.len(), 1, "exactly one unreachable-code report: {ds:?}");
}

// ── Pin 7: the unchecked-JS open-ended-object family (salsa core) — a
// strict:false JS unit where `this.member = {}`-style expando writes
// leave `{}`-typed members; the follow-up `.a` writes report TS2339 as
// ERRORs (not suggestions) through the real isUncheckedJsSuggestion /
// reportNonexistentProperty ladder ──

const OPEN_ENDED_OBJECT: &str = "var obj = {\n\
    property: {}\n\
};\n\
obj.property.a = 0;\n";

#[test]
fn unchecked_js_open_ended_object_2339_pin() {
    // Post-fix (Go f26p3full twin a.js arm, byte-same): the literal-written
    // `property` stays `{}`-typed (not any), so `obj.property.a` reports
    // exactly one TS2339 (`a` on `{}`). Pre-fix the permissive unchecked-JS
    // gates widened it away.
    let (mut c, file) = check_with(
        OPEN_ENDED_OBJECT,
        ScriptKind::JS,
        "js",
        CompilerOptions {
            check_js: Tristate::TSTrue,
            ..CompilerOptions::default()
        },
    );
    let ds = diags(&mut c, file);
    let e2339 = codes(&ds, 2339);
    assert_eq!(e2339.len(), 1, "exactly one 2339: {ds:?}");
    assert_eq!(e2339[0][0], "a", "the open-ended write: {ds:?}");
    assert_eq!(e2339[0][1], "{}", "the literal member type: {ds:?}");
}

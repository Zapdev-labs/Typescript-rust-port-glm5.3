//! FCS37 pins (full-corpus check sweep, tranche 37): the TS2322
//! destructuring cluster — the t33-era census's "31 rust-missing
//! instances across ~13 es6/destructuring records" (binding defaults
//! `string -> undefined`, tuple `boolean -> [[any]]` shapes). ("FCS
//! tranche 37" PORT notes in-tree.)
//!
//! THREE roots, all Go-verified via temporary relater.go probes on the
//! rig (the probes were removed; the Go tree restored and md5-verified):
//!
//! ROOT 1 — the 31 rust-MISSING 2322s (+ the 4 same-record rust-EXTRA
//! head 2322s): the spans are ARGUMENT array-literal ELEMENTS at call
//! sites (`b6(["string", 1, 2])`, the destructuring variable-declaration
//! RHS, parameter-property constructor arguments) — Go emits them from
//! `elaborateArrayLiteral` (relater.go:522): when the array literal's
//! type isn't tuple-like it is re-checked with the failing target as the
//! contextual type (`CheckModeContextual|CheckModeForceTuple`), then
//! every element whose index exists on the target is elaborated through
//! `elaborateElement`, replacing the head error. rust's
//! `elaborate_array_literal` (relater.rs) was the M6 S4/S9 stub
//! returning false. Ported Go-exactly.
//!
//! ROOT 2 — the 2353 position family (exposed by ROOT 1 in
//! destructuringParameterProperties5): the relater's excess-property arm
//! resets the error node to the property's name node when the property
//! is assigned inside the object literal itself. The port's ancestor
//! guard compared the node's raw lazily-assigned `Node.id` cell against
//! the packed `NodeId` — a value-space mismatch that could never match,
//! so the reset silently never fired (the whole-literal span leaked
//! into the 2353 diagnostic). Fixed with the binder's NodeId-callback
//! `find_ancestor` identity comparison (Go: `d == objectLiteralDeclaration`).
//!
//! ROOT 3 — the 2345 argument family (`null` vs `null | undefined`) and
//! the union-target elaboration bail: the relater free fn
//! `get_type_of_parameter` (used by `try_get_type_at_position` — the
//! paramType of every argument applicability check and signature
//! relation) was a bare `get_type_of_symbol` stub. Go (checker.go:17429)
//! wraps it: `addOptionalityEx(getTypeOfSymbol(symbol), false,
//! declaration != nil && (declaration.Initializer() != nil ||
//! isOptionalDeclaration(declaration)))` — parameters with initializers
//! carry `| undefined`. Without it the rendered 2345 target lost the
//! union member, and the `{ z: boolean; }`-vs-`{ z: number; } | undefined`
//! object-literal elaboration succeeded per-property (emitting a 2322)
//! where Go's `getBestMatchingType` finds no assignable constituent,
//! bails, and reports the head 2345 at the argument. Fixed by
//! delegating to the Go-exact typeofsymbol.rs method.
//!
//! After the tranche ALL 13 destructuring records are byte-identical
//! through both drivers (fresh dd dumps); evidence in
//! ~/.fcs37-continuation.md on the rig.
//!
//! All pins run WITHOUT libs (the corpus twin seam — the per-unit
//! checker), matching the go-bench driver's construction; the sources
//! are the corpus cases' shapes.

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::options_generated::{CompilerOptions, ScriptTarget};
use tsc_core::scriptkind::ScriptKind;
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

fn check_with(src: &str) -> (Checker, tsc_ast::NodeId) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        strict: tsc_core::tristate::Tristate::TSTrue,
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

/// The (code, args) list of the checker's WHOLE semantic collection (the
/// corpus seam reads `semantic_diagnostics()`).
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

/// The destructuringParameterDeclaration1ES5.ts b6/b7 shape: parameters
/// whose binding pattern carries a DEFAULT initializer — the parameter
/// type is the initializer's tuple, and the ARGUMENT array literal is
/// elaborated per element (ROOT 1). GO emits exactly 7 element-level
/// 2322s (the 1ES5 census's 7 missing for this record).
const DEFAULT_INITIALIZER_SLICE: &str = r#"
function b6([a, z, y] = [undefined, null, undefined]) { }
function b7([[a], b, [[c, d]]] = [[undefined], undefined, [[undefined, undefined]]]) { }

b6(["string", 1, 2]);
b7([["string"], 1, [[true, false]]]);
"#;

/// The destructuringParameterProperties5.ts shape: the excess-property
/// 2353 must be reported (via the array-literal elaboration) with the
/// property's own name as the error node (ROOT 2) — plus the tuple
/// element elaborations for the second/third elements (ROOT 1).
const PARAMETER_PROPERTIES_SLICE: &str = r#"
type ObjType1 = { x: number; y: string; z: boolean };
type TupleType1 = [ObjType1, number, string];

class C1 {
    constructor(public [{ x1, x2, x3 }, y, z]: TupleType1) { }
}

var a = new C1([{ x1: 10, x2: "", x3: true }, "", false]);
"#;

/// The destructuringParameterDeclaration2.ts b1 shape: a parameter with
/// an initializer is OPTIONAL — `getTypeOfParameter` adds `| undefined`
/// (ROOT 3), so the 2345 renders the target `null | undefined` (the
/// pre-tranche rust rendered bare `null`).
const OPTIONAL_INITIALIZER_SLICE: &str = r#"
function b1(z = null, o = { x: 0, y: undefined }) { }

b1("string", { x: "string", y: true });
"#;

/// The destructuringParameterDeclaration2.ts c1/c3 shape: the object-
/// literal argument against the union-shaped optional parameter — Go's
/// `getBestMatchingType` finds no assignable constituent, the
/// elaboration BAILS, and the head 2345 is reported at the ARGUMENT
/// (no per-property 2322). The pre-tranche rust (missing ROOT 3) had a
/// non-union target, elaborated per property, and emitted 2322s.
const UNION_TARGET_BAIL_SLICE: &str = r#"
function c1({z} = { z: 10 }) { }
function c3({b}: { b: number|string } = { b: "hello" }) { }

c1({ z: true });
c3({ b: true });
"#;

/// Pin 1 (ROOT 1 — the per-element array-literal elaboration): the b6/b7
/// calls report exactly GO's 7 element-level 2322s — the literal types
/// widened through `checkExpressionForMutableLocationWithContextualType`
/// (string/number, the true/false literals) against the default
/// initializer's tuple element types (undefined/null).
#[test]
fn fcs37_destructuring_default_parameter_elements_pin() {
    let (mut c, file) = check_with(DEFAULT_INITIALIZER_SLICE);
    let ds = diags(&mut c, file);
    let d = codes(&ds, 2322);
    let mut expected = vec![
        vec!["string".to_string(), "undefined".to_string()],
        vec!["number".to_string(), "null".to_string()],
        vec!["number".to_string(), "undefined".to_string()],
        vec!["string".to_string(), "undefined".to_string()],
        vec!["number".to_string(), "undefined".to_string()],
        vec!["true".to_string(), "undefined".to_string()],
        vec!["false".to_string(), "undefined".to_string()],
    ];
    expected.sort();
    let mut got = d.clone();
    got.sort();
    assert_eq!(
        got, expected,
        "the GO 2322 element census for b6/b7; whole unit: {ds:?}"
    );
}

/// Pin 2 (ROOT 2 + ROOT 1 — the excess-property position): the
/// parameter-property call reports the 2353 for `x1` (the elaborated
/// object-literal element against ObjType1) AND the tuple element
/// elaborations for the string/boolean mismatches. The 2353's error node
/// is the property NAME (the corpus dump's 342|344), not the whole
/// literal — the args pin the diagnostic's presence through the
/// elaboration path; the span pin lives in the corpus dump-diff.
#[test]
fn fcs37_excess_property_reports_at_property_pin() {
    let (mut c, file) = check_with(PARAMETER_PROPERTIES_SLICE);
    let ds = diags(&mut c, file);
    let d2353 = codes(&ds, 2353);
    assert!(
        d2353.iter().any(|a| a.first().map(|s| s.as_str()) == Some("x1")
            && a.get(1).map(|s| s.as_str()) == Some("ObjType1")),
        "the 2353 for x1 must be reported through the elaboration; got {d2353:?} (whole unit: {ds:?})"
    );
    let d2322 = codes(&ds, 2322);
    let mut expected = vec![
        vec!["string".to_string(), "number".to_string()],
        vec!["boolean".to_string(), "string".to_string()],
    ];
    expected.sort();
    let mut got = d2322.clone();
    got.sort();
    assert_eq!(got, expected, "the y/z element elaborations; whole unit: {ds:?}");
}

/// Pin 3 (ROOT 3 — the initializer-parameter optionality): the 2345
/// against `z = null` renders the target `null | undefined` — before the
/// tranche the free-fn stub lost the union member and rendered `null`.
#[test]
fn fcs37_parameter_initializer_optionality_renders_pin() {
    let (mut c, file) = check_with(OPTIONAL_INITIALIZER_SLICE);
    let ds = diags(&mut c, file);
    let d = codes(&ds, 2345);
    assert!(
        d.iter().any(|a| a.iter().any(|s| s == "null | undefined")),
        "the 2345 target must include the added undefined: {d:?} (whole unit: {ds:?})"
    );
    assert!(
        !d.iter().any(|a| a.iter().any(|s| s == "null")),
        "the bare `null` target rendering must be gone; got {d:?}"
    );
}

/// Pin 4 (ROOT 3's union-target bail): the object-literal arguments
/// against the optional `{ z: number; } | undefined` /
/// `{ b: string | number; } | undefined` parameters report the HEAD 2345
/// at the argument (source `{ z: boolean; }` / `{ b: boolean; }`) and
/// NO per-property 2322 — the pre-tranche rust elaborated the
/// non-union target and emitted 2322s where GO bails.
#[test]
fn fcs37_union_target_no_property_elaboration_pin() {
    let (mut c, file) = check_with(UNION_TARGET_BAIL_SLICE);
    let ds = diags(&mut c, file);
    let d2322 = codes(&ds, 2322);
    assert!(
        d2322.is_empty(),
        "the elaboration must bail on the union target — no 2322; got {d2322:?} (whole unit: {ds:?})"
    );
    let d2345 = codes(&ds, 2345);
    let sources: Vec<&str> = d2345
        .iter()
        .filter_map(|a| a.first().map(|s| s.as_str()))
        .collect();
    assert!(
        sources.contains(&"{ z: boolean; }") && sources.contains(&"{ b: boolean; }"),
        "both head 2345s must report at the arguments; sources {sources:?} (whole unit: {ds:?})"
    );
}

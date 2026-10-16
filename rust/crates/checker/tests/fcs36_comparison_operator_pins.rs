//! FCS36 pins (full-corpus check sweep, tranche 36): the TS2365 family —
//! "Operator '<' cannot be applied to types ..." (the t26-era census's
//! "48 instances in one record"; the fresh per-code census corrected it
//! to 4 records, RUST-EXTRA 94 / RUST-MISSING 46). ("FCS tranche 36"
//! PORT notes in-tree.)
//!
//! THREE roots, all Go-verified via temporary relater.go probes (the
//! probes were removed; the rig Go tree restored and md5-verified):
//!
//! ROOT B — the 48 count-diff (comparisonOperatorWithNoRelationshipTypeParameter,
//! rust 98 vs GO 50): the relation arms' free fn `get_constraint_of_type`
//! (relater.rs) was a RAW-FIELD shortcut reading `typeParameter.constraint`
//! directly. Go (checker.go:17434 → getConstraintOfTypeParameter →
//! hasNonCircularBaseConstraint + getConstraintFromTypeParameter, the
//! noConstraintType sentinel) answers NIL for an unconstrained type
//! parameter; the raw field holds the eagerly-installed `{}` default, so
//! rust's relational arm fell into the {}-vs-primitive object shortcut
//! for (T, string/void/{a:string}) and emitted rust-EXTRA 2365s where
//! GO's (unknown, X) fast path (isSimpleTypeRelatedTo t&Unknown) said
//! comparable. Fixed by delegating to the Go-exact typeofsymbol.rs family.
//!
//! ROOT A — rendering in all 4 records: `getBaseTypeOfLiteralTypeForComparison`
//! (checker.go:25988) was the M6 S7 TODO stub delegating to
//! getBaseTypeOfLiteralType; Go widens StringLike→string,
//! NumberLiteral|Enum→number, BigIntLiteral→bigint, BooleanLiteral→boolean,
//! Union→map, so the 2365 message renders the enum operand as `number`.
//! Ported Go-exactly.
//!
//! ROOT C — the `+`/`+=` records' `E.a` vs `E` rendering:
//! `getBaseTypeOfEnumLikeType` (checker.go:25980) was the M6 S2 None
//! stub, so getBaseTypeOfLiteralType never reduced an enum-member literal
//! to its parent enum and getBaseTypesIfUnrelated kept the unreduced
//! `E.a` in the message. Ported Go-exactly (the Option form carries Go's
//! non-nil answer as Some in both cases).
//!
//! ROOT D — the battery's single newly-divergent record
//! (types/rest/genericRestParameters2.ts, a rust-EXTRA 2370 on
//! `...tail: infer B`): the Go-exact `getConstraintOfType` (root B)
//! unmasked two latent seams in `getInferredTypeParameterConstraint`
//! (checker.go): the arm walk was seeded one level short (Go's
//! `child = declaration.Parent; parent = child.Parent` skips PAST the
//! InferType node — every arm switched on InferType itself and missed),
//! and the rest-position arm kept the permissive `unknownType` element
//! instead of `createArrayType(unknownType)`. An `infer B` in a rest
//! position answered constraint `unknown`/none instead of `unknown[]`,
//! and the rest-parameter check (checker.go:2743) 2370'd where GO's
//! `unknown[]` stays assignable to anyReadonlyArrayType. Both fixed
//! Go-exactly.
//!
//! After the tranche all four records are byte-identical through both
//! drivers (per-code 2365 census: EXTRA 0 / MISSING 0); evidence in
//! ~/.fcs36-continuation.md on the rig.
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

/// The comparisonOperatorWithNoRelationshipTypeParameter.ts r1a/r1b shape
/// verbatim (t and u vs every operand; the corpus case's `foo<T, U>`
/// preamble).
const TYPE_PARAMETER_SLICE: &str = r#"
enum E { a, b, c }

declare var a: boolean;
declare var b: number;
declare var c: string;
declare var d: void;
declare var e: E;
declare var f: { a: string };

function foo<T, U>(t: T, u: U) {
    var r0 = t < u;

    var r1a1 = t < a;
    var r1a2 = t < b;
    var r1a3 = t < c;
    var r1a4 = t < d;
    var r1a5 = t < e;
    var r1a6 = t < f;

    var r1b1 = a < t;
    var r1b2 = b < t;
    var r1b3 = c < t;
    var r1b4 = d < t;
    var r1b5 = e < t;
    var r1b6 = f < t;
}
"#;

/// The comparisonOperatorWithNoRelationshipPrimitiveType.ts shape: the
/// enum operand widens to `number` for the 2365 message (ROOT A).
const PRIMITIVE_SLICE: &str = r#"
enum E { a, b, c }

declare var a: number;
declare var b: boolean;
declare var e: E;

var r1 = a < e;
var r2 = b < e;
var r3 = e < b;
var r4 = e < a;
"#;

/// The additionOperatorWithInvalidOperands.ts / compoundAddition shapes:
/// the enum-member literal reduces to its parent enum for the message
/// (ROOT C), and the `+=` record's rendering twins it.
const ADDITION_SLICE: &str = r#"
enum E { a, b, c }
class C { foo() { return 1; } }

var r18 = E.a + new C();

declare var s: boolean;
s += E.a;
"#;

/// The types/rest/genericRestParameters2.ts P1 shape: an `infer B` in a
/// rest position (ROOT D — the arm walk + the `unknown[]` inference).
const INFER_REST_SLICE: &str = r#"
type P1<T extends Function> = T extends (head: infer A, ...tail: infer B) => any ? { head: A, tail: B } : any[];

type T10 = P1<(x: number, y: string, ...z: boolean[]) => void>;
type T12 = P1<(x: number, y: number) => void>;
"#;

/// Pin 1 (ROOT B — the type-parameter comparability census twin): in a
/// relational comparison, an unconstrained T errors ONLY against U and
/// against number-assignable operands (the exactly-one-side-number rule
/// of checker.go:12704), with the enum operand widened to `number`
/// (ROOT A). Before the tranche rust ALSO errored T vs
/// string/void/{ a: string; } — the raw-field constraint read — for a
/// rust-EXTRA 48 (98 vs GO 50 on the corpus record).
#[test]
fn fcs36_type_parameter_relational_census_twin_pin() {
    let (mut c, file) = check_with(TYPE_PARAMETER_SLICE);
    let ds = diags(&mut c, file);
    let d = codes(&ds, 2365);
    // GO emits exactly: t<u, t<b, t<e, b<t, e<t (the enum renders number).
    let mut expected = vec![
        vec!["<".to_string(), "T".to_string(), "U".to_string()],
        vec!["<".to_string(), "T".to_string(), "number".to_string()],
        vec!["<".to_string(), "T".to_string(), "number".to_string()],
        vec!["<".to_string(), "number".to_string(), "T".to_string()],
        vec!["<".to_string(), "number".to_string(), "T".to_string()],
    ];
    expected.sort();
    let mut got = d.clone();
    got.sort();
    assert_eq!(got, expected, "the GO 2365 census for the r1a/r1b block; whole unit: {ds:?}");
    // The raw-field constraint regression: none of the T-vs-non-number
    // comparisons may report (comparable(T, X) is true through the
    // unknown-constraint fast path).
    assert!(
        !d.iter().flatten().any(|x| x == "boolean" || x == "string" || x == "void" || x == "{ a: string; }" || x == "E"),
        "no 2365 may mention the non-number operands; got {d:?}"
    );
}

/// Pin 2 (ROOT A — the comparison-flavored widening): the enum operand
/// renders as `number` in the 2365 message on BOTH sides
/// (`b < e` → boolean/number, `e < b` → number/boolean), while
/// number-vs-enum stays compatible (no error, `a < e`).
#[test]
fn fcs36_enum_renders_number_for_comparison_pin() {
    let (mut c, file) = check_with(PRIMITIVE_SLICE);
    let ds = diags(&mut c, file);
    let d = codes(&ds, 2365);
    let mut expected = vec![
        vec!["<".to_string(), "boolean".to_string(), "number".to_string()],
        vec!["<".to_string(), "number".to_string(), "boolean".to_string()],
    ];
    expected.sort();
    let mut got = d.clone();
    got.sort();
    assert_eq!(got, expected, "only b<e and e<b report, both rendering the enum as number; whole unit: {ds:?}");
}

/// Pin 3 (ROOT C — the enum-member literal reduces to the enum): the
/// `+` arm's reportOperatorError keeps GO's base types — `E.a` renders
/// as `E` (getBaseTypeOfEnumLikeType real), and the `+=` compound twins
/// it (`boolean` / `E`, never `E.a`).
#[test]
fn fcs36_enum_member_renders_enum_for_addition_pin() {
    let (mut c, file) = check_with(ADDITION_SLICE);
    let ds = diags(&mut c, file);
    let d = codes(&ds, 2365);
    let mut expected = vec![
        vec!["+".to_string(), "E".to_string(), "C".to_string()],
        vec!["+=".to_string(), "boolean".to_string(), "E".to_string()],
    ];
    expected.sort();
    let mut got = d.clone();
    got.sort();
    assert_eq!(got, expected, "E.a must render as E in +/+= messages; whole unit: {ds:?}");
    assert!(
        !d.iter().flatten().any(|x| x == "E.a"),
        "the unreduced enum-member literal must not reach the message; got {d:?}"
    );
}

/// Pin 4 (ROOT D — the inferred rest constraint): `...tail: infer B`
/// must NOT 2370 — the inferred parameter's constraint is `unknown[]`
/// (createArrayType(unknownType)), which stays assignable to
/// anyReadonlyArrayType. Before the tranche's arm-walk fix every arm
/// switched on the InferType node itself and the constraint read as
/// `unknown`/none, 2370-ing the genericRestParameters2 P1 line.
#[test]
fn fcs36_infer_rest_parameter_no_2370_pin() {
    let (mut c, file) = check_with(INFER_REST_SLICE);
    let ds = diags(&mut c, file);
    let d = codes(&ds, 2370);
    assert!(d.is_empty(), "the infer B rest parameter must not report 2370; got {d:?} (whole unit: {ds:?})");
}

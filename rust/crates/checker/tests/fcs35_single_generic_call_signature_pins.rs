//! FCS35 pins (full-corpus check sweep, tranche 35): the S9
//! generic-function-argument seam — Go checker.go:7809-7870
//! `instantiateTypeWithSingleGenericCallSignature` — the fcs34 residual
//! queue's head. ("FCS tranche 35" PORT notes in-tree.)
//!
//! ROOT — the whole tail of the Go function was the dormant M6 S9 TODO in
//! spine.rs: an expression that is an ARGUMENT of a generic function
//! whose type has a single generic call signature and whose contextual
//! type has a single NON-generic call signature instantiates the
//! signature with its own type parameters (getUniqueTypeParameters —
//! renamed + target/mapper-wired), infers contravariantly from the
//! contextual signature's parameters (and, on candidates, its return),
//! and — when the fresh inferences do not overlap the outer context's —
//! ADOPTS them (mergeInferences) and appends the signature's type
//! parameters to `context.inferredTypeParameters` (which fcs34's
//! chooseOverload fix reads at checker.go:9289/9313). Without it rust
//! erased the contextual signature's type parameters to `{}`, so
//! genericContextualTypes1 diverged: the f40 `flip(zip)` 2322 rendered
//! `(y: {}, x: {}) => [{}, {}]` against `<A, B>(b: B, a: A) => [A, B]`
//! (rust-EXTRA at the f40 line) and the f13 `compose(unbox, unlist)`
//! 2322 rendered `{}`/`{}` where GO keeps the candidate to `unknown`
//! (hash-only delta). Collateral in-family fixes: `skippedGenericFunction`
//! sets the per-context InferenceFlagsSkippedGenericFunction (checker.go:
//! 10250) instead of the checker-global isInferencePartiallyBlocked, and
//! chooseOverload propagates it onto argCheckMode (checker.go:9282);
//! `instantiateSignatureInContextOf` gained its non-nil inferenceContext
//! arm (checker.go:19873-19885 — the restType non-fixing/mapper choice
//! and the return-type pass gated on `inferenceContext == nil`).
//!
//! After the tranche, genericContextualTypes1 is byte-identical to the
//! go-bench driver (16 errors, the f13 2322 rendering
//! `(a: Box<{}>) => unknown` vs `<T>(x: Box<{}>) => T`), and the
//! full-corpus monotonicity is 839 -> 836 divergent with 0
//! newly-divergent (dump-diffed through both drivers; evidence in
//! ~/.fcs35-continuation.md on the rig).
//!
//! All pins run WITHOUT libs (the corpus twin seam — the per-unit
//! checker), matching the go-bench driver's construction; the source is
//! the genericContextualTypes1.ts corpus case verbatim (through f40).

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

/// The genericContextualTypes1.ts corpus case verbatim through the f40
/// line (the @target/@strict/@declaration preamble maps to the unit
/// harness's ES2015 + strict).
const GCT1: &str = r#"
type Box<T> = { value: T };

declare function wrap<A, B>(f: (a: A) => B): (a: A) => B;

declare function compose<A, B, C>(f: (a: A) => B, g: (b: B) => C): (a: A) => C;

declare function list<T>(a: T): T[];

declare function unlist<T>(a: T[]): T;

declare function box<V>(x: V): Box<V>;

declare function unbox<W>(x: Box<W>): W;

declare function map<T, U>(a: T[], f: (x: T) => U): U[];

declare function identity<T>(x: T): T;

declare function zip<A, B>(a: A, b: B): [A, B];

declare function flip<X, Y, Z>(f: (x: X, y: Y) => Z): (y: Y, x: X) => Z;

const f00: <A>(x: A) => A[] = list;
const f01: <A>(x: A) => A[] = x => [x];
const f02: <A>(x: A) => A[] = wrap(list);
const f03: <A>(x: A) => A[] = wrap(x => [x]);

const f10: <T>(x: T) => Box<T[]> = compose(a => list(a), b => box(b));
const f11: <T>(x: T) => Box<T[]> = compose(list, box);
const f12: <T>(x: Box<T[]>) => T = compose(a => unbox(a), b => unlist(b));
const f13: <T>(x: Box<T[]>) => T = compose(unbox, unlist);

const arrayMap = <T, U>(f: (x: T) => U) => (a: T[]) => a.map(f);
const arrayFilter = <T>(f: (x: T) => boolean) => (a: T[]) => a.filter(f);

const f20: (a: string[]) => number[] = arrayMap(x => x.length);
const f21: <A>(a: A[]) => A[][] = arrayMap(x => [x]);
const f22: <A>(a: A[]) => A[] = arrayMap(identity);
const f23: <A>(a: A[]) => Box<A>[] = arrayMap(value => ({ value }));

const f30: (a: string[]) => string[] = arrayFilter(x => x.length > 10);
const f31: <T extends Box<number>>(a: T[]) => T[] = arrayFilter(x => x.value > 10);

const f40: <A, B>(b: B, a: A) => [A, B] = flip(zip);
"#;

/// Pin 1 (the tranche target — genericContextualTypes1 f40, `flip(zip)`):
/// the contextual-signature candidate dance keeps `[A, B]` as the
/// candidates, so the instantiated flip matches the annotation and NO
/// 2322 mentions the flip shape. Before the tranche rust erased the
/// candidates to `{}` and reported a rust-EXTRA 2322
/// `(y: {}, x: {}) => [{}, {}]` vs `<A, B>(b: B, a: A) => [A, B]`.
#[test]
fn fcs35_flip_zip_keeps_candidates_no_extra_2322_pin() {
    let (mut c, file) = check_with(GCT1);
    let ds = diags(&mut c, file);
    let d = codes(&ds, 2322);
    assert!(
        !d.iter()
            .any(|a| a.iter().any(|x| x.contains("[A, B]") || x.contains("[{}, {}]"))),
        "flip(zip) must not surface a 2322 (the [A, B] candidates survive); got {d:?}"
    );
}

/// Pin 2 (the f13 rendering — `compose(unbox, unlist)`): the single 2322
/// of the unit renders `(a: Box<{}>) => unknown` against
/// `<T>(x: Box<{}>) => T` — the second pass keeps the RETURN candidate to
/// `unknown` (GO's rendering; the fcs34 hash-only delta was `{}` there).
#[test]
fn fcs35_compose_unbox_unlist_renders_unknown_return_pin() {
    let (mut c, file) = check_with(GCT1);
    let ds = diags(&mut c, file);
    let d = codes(&ds, 2322);
    assert_eq!(d.len(), 1, "expected exactly one 2322 (the f13 line); got {ds:?}");
    assert_eq!(d[0][0], "(a: Box<{}>) => unknown", "the f13 source rendering");
    assert_eq!(d[0][1], "<T>(x: Box<{}>) => T", "the f13 target rendering");
}

/// Pin 3 (the whole-unit census twin): the genericContextualTypes1 shape
/// through f40 produces exactly the six non-global diags the go-bench
/// driver reports (1x 2322, 2x 2339, 3x 18046) — the 2318
/// missing-global-type set is the no-libs corpus twin seam and is
/// asserted only by count.
#[test]
fn fcs35_generic_contextual_types1_census_twin_pin() {
    let (mut c, file) = check_with(GCT1);
    let ds = diags(&mut c, file);
    assert_eq!(codes(&ds, 2322).len(), 1, "one 2322 (f13): {ds:?}");
    assert_eq!(codes(&ds, 2339).len(), 2, "two 2339 (map/filter): {ds:?}");
    assert_eq!(codes(&ds, 18046).len(), 3, "three 18046 (x): {ds:?}");
    // The go-bench driver's record for this case (dump-diffed): 16 total
    // errors — 10 of them the 2318 missing-global-type family.
    assert_eq!(codes(&ds, 2318).len(), 10, "the no-libs 2318 family: {ds:?}");
    assert_eq!(ds.len(), 16, "the whole-unit census must twin GO's 16");
}

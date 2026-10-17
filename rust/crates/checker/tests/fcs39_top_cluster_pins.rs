//! FCS39 pins (full-corpus check sweep, tranche 39): the top-3 census
//! clusters of the fresh t38-final-dump census — TS2882 rust-missing
//! x59/33rec (the side-effect-import resolution family), TS17009
//! rust-missing x58/16rec (the super-before-this flow family), TS2374
//! rust-missing x56/5rec (the duplicate index signature family). ("FCS
//! tranche 39" PORT notes in-tree.)
//!
//! THREE roots, all probe-evidenced on the rig (temporary eprintln
//! probes, removed; evidence in ~/.fcs39-continuation.md):
//!
//! ROOT A — TS2882: the side-effect-import arm of
//! `check_import_declaration` (Go checker.go:5477) gates on
//! `NoUncheckedSideEffectImports.IsTrueOrUnknown()` — the UNSET
//! (Unknown) option fires the arm. The port's `== Tristate::TSTrue`
//! skipped it entirely, so every unresolvable `import "./x"` under the
//! corpus configs reported nothing where Go reports TS2882 through the
//! module-not-found ladder (checker.go:15709-15748).
//!
//! ROOT B — TS17009: `check_this_before_super` (Go checker.go:12513)
//! was the M6 W3 TODO stub — a no-op. The flow engine is REAL now
//! (`is_post_super_flow_node`), so the port gained the exact Go body:
//! skip when the containing class has no extends clause or extends
//! null, otherwise probe the reference's flow node and report the
//! this/super-before-super error when the node is not post-super.
//!
//! ROOT C — TS2374: `check_type_for_duplicate_index_signatures` (Go
//! checker.go:5028) had TWO breaks: (a) the type-literal caller
//! (`check_type_literal`, Go checker.go:3189) was a TODO stub that
//! never called it; (b) inside the loop, the index-signature
//! PARAMETER's type node was read through the function-like
//! RETURN-type accessor (`signature_type_node`), which answers None
//! for a `Parameter` node — dropping every duplicate-index probe even
//! for classes/interfaces. The port now reads the Parameter's own
//! `type_` node (Go `parameters[0].Type()`).
//!
//! All pins run WITHOUT libs (the corpus twin seam — the per-unit
//! checker), matching the go-bench driver's construction; the sources
//! are the corpus cases' shapes.

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::options_generated::{CompilerOptions, ScriptTarget};
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
            implied_node_format: tsc_core::compileroptions::ResolutionModeNone,
        }
    }
    fn file_exists(&self, path: &str) -> bool {
        path == self.name.as_string()
    }
}

/// The check twin: parse + bind + check one unit under the given
/// options at the given target, with the unit's rooted name carried by
/// the `source_file_name` seam.
fn check_with_options(src: &str, name: &str, options: CompilerOptions) -> (Checker, tsc_ast::NodeId) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let program = TestProgram {
        options,
        name: RootedFilePath::from(name),
        source_file: sf,
        bind,
        binder_symbol_count,
        checker_symbols: Vec::new(),
    };
    let mut checker = new_checker(Box::new(program));
    checker.check_source_file(file, true);
    (checker, file)
}

fn check_with(src: &str, target: ScriptTarget) -> (Checker, tsc_ast::NodeId) {
    check_with_options(src, "/x/unit.ts", CompilerOptions { target, ..CompilerOptions::default() })
}

/// The (code, args) list of the checker's WHOLE semantic collection.
fn diags(c: &mut Checker) -> Vec<(i32, Vec<String>)> {
    c.semantic_diagnostics()
        .iter()
        .map(|d| (d.code(), d.message_args().to_vec()))
        .collect()
}

fn count(ds: &[(i32, Vec<String>)], code: i32) -> usize {
    ds.iter().filter(|(c, _)| *c == code).count()
}

// ── ROOT A — TS2882 (the side-effect-import resolution family) ──

/// The importAssertion1.ts shape (ROOT A): a side-effect import of an
/// unresolvable module under the DEFAULT (Unknown)
/// noUncheckedSideEffectImports — Go's IsTrueOrUnknown gate fires the
/// resolution check and the module-not-found ladder reports TS2882.
/// The pre-tranche port's `== TSTrue` gate skipped the whole arm.
#[test]
fn fcs39_unresolved_side_effect_import_reports_2882_pin() {
    let src = "import './missing';\n";
    let (mut c, _) = check_with(src, ScriptTarget::ES2015);
    let ds = diags(&mut c);
    assert_eq!(
        count(&ds, 2882),
        1,
        "the unresolved side-effect import must report TS2882 under the Unknown default: {ds:?}"
    );
}

/// The explicitly-DISABLED counterpart (ROOT A): with
/// noUncheckedSideEffectImports = false the arm never fires — no
/// TS2882 (Go-exact IsTrueOrUnknown semantics: only TSTrue/TSUnknown
/// pass the gate).
#[test]
fn fcs39_side_effect_import_2882_gated_by_false_pin() {
    let src = "import './missing';\n";
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        no_unchecked_side_effect_imports: Tristate::TSFalse,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_with_options(src, "/x/unit.ts", options);
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 2882), 0, "TSFalse must disable the 2882 arm: {ds:?}");
}

// ── ROOT B — TS17009 (the super-before-this flow family) ──

/// The superCallBeforeThisAccessing3.ts shape (ROOT B): `this` access
/// before the `super()` call in a derived constructor reports TS17009.
/// The pre-tranche `check_this_before_super` was a no-op stub.
#[test]
fn fcs39_this_before_super_reports_17009_pin() {
    let src = "class Base { constructor() { } }\nclass D extends Base {\n    private _t;\n    constructor() {\n        this._t;\n        super();\n    }\n}\n";
    let (mut c, _) = check_with(src, ScriptTarget::ES2015);
    let ds = diags(&mut c);
    assert_eq!(
        count(&ds, 17009),
        1,
        "this access before super() in a derived constructor must report TS17009: {ds:?}"
    );
}

/// The post-super counterpart (ROOT B): the same `this` access AFTER
/// the super() call is post-super in the flow graph — no TS17009.
#[test]
fn fcs39_this_after_super_no_17009_pin() {
    let src = "class Base { constructor() { } }\nclass D extends Base {\n    private _t;\n    constructor() {\n        super();\n        this._t;\n    }\n}\n";
    let (mut c, _) = check_with(src, ScriptTarget::ES2015);
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 17009), 0, "post-super this access must not report: {ds:?}");
}

/// The arrow-function counterpart (ROOT B, the superCallBeforeThisAccessing3
/// negative): a `this` inside an arrow body BEFORE super() is not
/// constructor-contained — GetThisContainer stops at the arrow, so
/// checkThisBeforeSuper never fires for it (Go-exact).
#[test]
fn fcs39_this_in_arrow_before_super_no_17009_pin() {
    let src = "class Base { constructor() { } }\nclass D extends Base {\n    private _t;\n    constructor() {\n        let x = () => { this._t; };\n        super();\n    }\n}\n";
    let (mut c, _) = check_with(src, ScriptTarget::ES2015);
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 17009), 0, "arrow-contained this is not the constructor's: {ds:?}");
}

/// The no-extends counterpart (ROOT B): a base-class constructor (no
/// heritage element) never reports — Go skips the probe when
/// GetClassExtendsHeritageElement answers nil.
#[test]
fn fcs39_this_in_base_class_constructor_no_17009_pin() {
    let src = "class B {\n    private _t;\n    constructor() {\n        this._t;\n    }\n}\n";
    let (mut c, _) = check_with(src, ScriptTarget::ES2015);
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 17009), 0, "no extends clause, no super-call obligation: {ds:?}");
}

// ── ROOT C — TS2374 (the duplicate index signature family) ──

/// The duplicateNumericIndexers.ts interface shape (ROOT C, part b):
/// two numeric index signatures on one interface — the parameter's
/// type node must be read from the Parameter itself (Go
/// `parameters[0].Type()`), one TS2374 per declaration.
#[test]
fn fcs39_duplicate_numeric_index_signature_on_interface_pin() {
    let src = "interface I {\n    [x: number]: string;\n    [x: number]: string;\n}\n";
    let (mut c, _) = check_with(src, ScriptTarget::ES2015);
    let ds = diags(&mut c);
    assert_eq!(
        count(&ds, 2374),
        2,
        "two numeric index signatures must report one TS2374 each: {ds:?}"
    );
}

/// The multipleNumericIndexers.ts class shape (ROOT C, part b): the
/// same duplicate inside a class declaration.
#[test]
fn fcs39_duplicate_numeric_index_signature_on_class_pin() {
    let src = "class C {\n    [x: number]: string;\n    [x: number]: string;\n}\n";
    let (mut c, _) = check_with(src, ScriptTarget::ES2015);
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 2374), 2, "the class arm must report the duplicates too: {ds:?}");
}

/// The multipleNumericIndexers.ts type-literal shape (ROOT C, part a):
/// `check_type_literal` must call check_type_for_duplicate_index_signatures
/// (Go checker.go:3189) — the pre-tranche TODO stub never did.
#[test]
fn fcs39_duplicate_index_signature_on_type_literal_pin() {
    let src = "var a: {\n    [x: number]: string;\n    [y: number]: string;\n}\n";
    let (mut c, _) = check_with(src, ScriptTarget::ES2015);
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 2374), 2, "the type-literal arm must report the duplicates: {ds:?}");
}

/// The distinct-kind counterpart (ROOT C): one string and one numeric
/// index signature is legal — no TS2374 (the map is keyed by the
/// distributed parameter type).
#[test]
fn fcs39_distinct_index_signatures_no_2374_pin() {
    let src = "interface I {\n    [x: string]: string;\n    [y: number]: string;\n}\n";
    let (mut c, _) = check_with(src, ScriptTarget::ES2015);
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 2374), 0, "distinct index kinds are legal: {ds:?}");
}

/// The single-signature counterpart (ROOT C): one index signature
/// never trips the `declarations.len() <= 1` gate.
#[test]
fn fcs39_single_index_signature_no_2374_pin() {
    let src = "interface I {\n    [x: number]: string;\n}\n";
    let (mut c, _) = check_with(src, ScriptTarget::ES2015);
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 2374), 0, "a single index signature is legal: {ds:?}");
}

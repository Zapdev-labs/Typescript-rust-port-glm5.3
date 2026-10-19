//! FCS61 pins (full-corpus check sweep, micro-tranche 60 — the const
//! type-parameter family, worked directly by the orchestrator): the
//! tranche's roots —
//!
//!  * `is_const_type_variable` (inference.rs) was a permissive-false M6
//!    stub, so `primitiveConstraint` in getCovariantInference never fired
//!    for const type parameters and fresh-tuple candidates widened
//!    instead of preserving literal elements. Implemented Go's
//!    checker.go:13916 walk (const modifier on type-parameter declarations
//!    via has_syntactic_modifier + the union/intersection /
//!    indexed-access / conditional / substitution / mapped / generic-tuple
//!    arms; get_constraint_of_conditional_type surfaced pub(crate)).
//!
//!  * `create_array_literal_type` (expressions.rs) was an identity stub —
//!    array-literal ORIGIN types never carried the
//!    ObjectFlagsArrayLiteral | ObjectFlagsContainsObjectOrArrayLiteral
//!    pair, so `is_object_or_array_literal_type` stayed false for tuple
//!    candidates and `union_object_and_array_literal_candidates` never
//!    merged them: with the const walk live, `f4([[1, "x"], [2, "y"]])`
//!    (`@param {[T, T]}`, `const T`) walked the common supertype to
//!    `[1, "x"]` instead of the merged union (4 extra TS2322s in
//!    jsdocTemplateTag6). Landed the cachedTypes ArrayLiteralType key +
//!    the cloneTypeReference + flag pair (Go checker.go:8305-8317).
//!
//!  * checkArrayLiteral's tuple arm hardcoded the readonly bit false
//!    (Go checker.go:8289 passes `inConstContext && !(contextualType !=
//!    nil && someType(contextualType, c.isMutableArrayLikeType))`), so
//!    const-context tuples printed `[..]` where Go prints `readonly [..]`
//!    and the mutable-vs-readonly re-check passes diverged. Landed the
//!    real bit.
//!
//!  * `get_contextual_type_for_element_expression` (contextual.rs) walked
//!    union contextuals through the NON-nil map_type twin — a nil-answering
//!    constituent collapsed the walk to unknown, so the pass-2 re-check of
//!    `f2(["a", ["b", "c"]])` (param `T | undefined`, `const T`) widened
//!    its elements to `[string, {}]` (two TS2322s Go never reports).
//!    Landed the Option-returning `map_type_opt` (typenodes.rs — Go's
//!    mapTypeEx nil-drop semantics) and converted the closure.
//!
//!  * `get_constraint_type_from_mapped_type` (members.rs) read the raw
//!    `AsTypeParameter().constraint` payload instead of Go's FULL
//!    getConstraintOfTypeParameter, so the FRESH type parameter of an
//!    instantiated homomorphic mapped type resolved its constraint to
//!    errorType ("any") — `isConstTypeVariable`'s MAPPED arm walked tv=None
//!    and `thingMapped({ foo: "" })` lost its const context (TS2345
//!    `{ foo: string; }` → `never`).
//!
//! ("FCS tranche 60" PORT notes in-tree; pins named fcs61 by census epoch.)

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::compileroptions::ResolutionModeNone;
use tsc_core::options_generated::{CompilerOptions, ModuleKind, ScriptTarget};
use tsc_core::scriptkind::ScriptKind;
use tsc_core::tristate::Tristate;
use tsc_parser::parse_source_file;
use tsc_tspath::RootedFilePath;

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
    fn get_emit_module_format_of_file(&self, _file: tsc_ast::NodeId) -> ModuleKind {
        ModuleKind::CommonJS
    }
    fn get_implied_node_format_for_emit(&self, _file: tsc_ast::NodeId) -> ModuleKind {
        ModuleKind::CommonJS
    }
    fn source_file_implied_node_format(&self, _file: tsc_ast::NodeId) -> ModuleKind {
        ModuleKind::CommonJS
    }
}

fn check_with(src: &str, options: CompilerOptions) -> (Checker, tsc_ast::NodeId) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let program = TestProgram {
        options,
        name: RootedFilePath::from("/x/fcs61_unit.ts".to_string()),
        source_file: sf,
        bind,
        binder_symbol_count,
        checker_symbols: Vec::new(),
    };
    let mut checker = new_checker(Box::new(program));
    checker.check_source_file(file, true);
    (checker, file)
}

fn check_strict(src: &str) -> (Checker, tsc_ast::NodeId) {
    check_with(
        src,
        CompilerOptions {
            target: ScriptTarget::ESNext,
            strict: Tristate::TSTrue,
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

// ── CLUSTER A — the const type-parameter tuple family ──

/// The typeParameterConstModifiers x12 shape: `f1(["a", ["b", "c"]])`
/// under `<const T>` infers T as `readonly ["a", readonly ["b", "c"]]`
/// (the const walk preserves the literal elements AND the tuple arm marks
/// the const-context tuples readonly), so the reveal assignment to the
/// exact readonly tuple type checks with no TS2322.
#[test]
fn fcs61_const_tuple_readonly_preserved_pin() {
    let (mut c, file) = check_strict(
        "declare function f1<const T>(x: T): T;\nconst x12 = f1([\"a\", [\"b\", \"c\"]]);\nconst reveal: readonly [\"a\", readonly [\"b\", \"c\"]] = x12;\n",
    );
    let ds = diags(&mut c, file);
    assert!(
        codes(&ds, 2322).is_empty(),
        "const T infers the readonly nested tuple exactly: {ds:?}"
    );
}

// ── CLUSTER B — the array-literal candidate merge family ──

/// The jsdocTemplateTag6 f4 shape (a bare-ts twin): `f4([[1, "x"],
/// [2, "y"]])` with `@param {[T, T]}` and `const T` — the two fresh tuple
/// candidates carry the ARRAY_LITERAL flag (createArrayLiteralType's
/// cloneTypeReference + flag pair), so unionObjectAndArrayLiteralCandidates
/// merges them into one union candidate and getCovariantInference's
/// common supertype answers the merged union: NO TS2322 where the
/// un-merged walk picked `[1, "x"]` against `[2, "y"]`.
#[test]
fn fcs61_array_literal_candidates_merge_pin() {
    let (mut c, file) = check_strict(
        "declare function f4<const T>(obj: [T, T]): T;\nconst x41 = f4([[1, \"x\"], [2, \"y\"]]);\n",
    );
    let ds = diags(&mut c, file);
    assert!(
        codes(&ds, 2322).is_empty(),
        "the two fresh tuple candidates merge into one union candidate: {ds:?}"
    );
}

// ── CLUSTER C — the union-contextual element family ──

/// The typeParameterConstModifiers x22 shape: `f2(["a", ["b", "c"]])`
/// with param `T | undefined` and `const T` — getContextualTypeForElement
/// Expression's mapTypeEx nil-drop walk keeps the pass-2 element
/// contextuals at the tuple's elements (`"a"`, `readonly ["b", "c"]`)
/// instead of collapsing to unknown, so the re-checked arg stays
/// `["a", ["b", "c"]]` and assigns to the instantiated param with NO
/// TS2322 (the pre-fix walk widened the elements to `[string, {}]`).
#[test]
fn fcs61_union_contextual_elements_not_widened_pin() {
    let (mut c, file) = check_strict(
        "declare function f2<const T, U>(x: T | undefined): T;\nconst x22 = f2([\"a\", [\"b\", \"c\"]]);\n",
    );
    let ds = diags(&mut c, file);
    assert!(
        codes(&ds, 2322).is_empty(),
        "the union contextual's tuple constituent keeps its element contextuals: {ds:?}"
    );
}

// ── CLUSTER D — the mapped-constraint const walk family ──

/// The typeParameterConstModifiers tMapped shape: `thingMapped({ foo: "" })`
/// with `NotEmptyMapped<O> = keyof O extends never ? never : { [K in keyof
/// O]: O[K] }` and `const O` — the instantiated homomorphic mapped type's
/// FRESH type parameter resolves its constraint through the combined
/// mapper (keyof O, not the raw payload's None), so isConstTypeVariable's
/// MAPPED arm walks tv=O and the arg keeps its const context: the
/// conditional instantiates to the mapped `{ foo: "" }` — NO TS2345
/// `{ foo: string; }` → `never`.
#[test]
fn fcs61_mapped_constraint_const_context_pin() {
    let (mut c, file) = check_strict(
        "type NotEmptyMapped<T extends { [k: string]: any }> = keyof T extends never ? never : { [K in keyof T]: T[K] };\nconst thingMapped = <const O extends { [k: string]: any }>(o: NotEmptyMapped<O>) => o;\nconst tMapped = thingMapped({ foo: \"\" });\n",
    );
    let ds = diags(&mut c, file);
    assert!(
        codes(&ds, 2345).is_empty(),
        "the instantiated mapped type's constraint resolves through the fresh parameter's mapper: {ds:?}"
    );
}

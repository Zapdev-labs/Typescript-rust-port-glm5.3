//! FCS25 pins (full-corpus check sweep, tranche 25): the
//! privateNameAndObjectRestSpread missing-2339 family — the 4-instance
//! census on the classes/members/privateNames/
//! privateNameAndObjectRestSpread.ts record.
//!
//! ROOT (single): `Checker::get_rest_type` (Go checker.go:18193-18233,
//! `getRestType` — the object binding-pattern rest element's type) was the
//! M6 S4/S7/S8 permissive stub: it returned the source's non-nullable form,
//! so private (`#name`) members survived into the rest type and
//! `const { ...rest } = other; rest.#prop` resolved through the source
//! instead of erroring TS2339. Go builds the member-excluded rest type:
//! the omitted (destructured) names' literal types form `omitKeyType`;
//! each source property is excluded when its literal type is assignable to
//! the omit key, or it carries a private/protected modifier, or it is not
//! spreadable — the survivors become `getSpreadSymbol` members of a fresh
//! anonymous `ObjectRestType` (generic sources route through
//! `Omit<source, omitKeys>`).
//!
//! The spread-expression cases (`const obj = { ...other }; obj.#prop`)
//! were already identical between the drivers (the object-spread path is
//! real) — only the REST-DESTRUCTURED accesses (`rest.#prop`,
//! `sRest.#propStatic`) diverged.
//!
//! Expectations verified byte-same against the go-bench twin's check-stage
//! dumps (rig probes f25p1/f25p2/f25p3/f25p4, both drivers, DIFF 0 on all
//! four) — the pins run WITHOUT libs (the corpus twin seam).

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::compileroptions::ResolutionModeNone;
use tsc_core::options_generated::CompilerOptions;
use tsc_core::scriptkind::ScriptKind;
use tsc_core::tristate::Tristate;
use tsc_parser::parse_source_file;
use tsc_tspath::RootedFilePath;

// ── The single-file Program scaffold (the fcs23/fcs24 idiom) ──

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

/// Check `src` (TS), no libs, strict — the conformance harness configuration
/// the corpus case declares (`@strict: true`, `@target: es6`).
fn check_ts(src: &str) -> (Checker, tsc_ast::NodeId) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let options = CompilerOptions {
        strict: Tristate::TSTrue,
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

/// The args of every TS2339 whose first message arg is `name`, in order.
fn errors_2339_on(ds: &[(i32, Vec<String>)], name: &str) -> Vec<Vec<String>> {
    ds.iter()
        .filter(|(code, args)| *code == 2339 && args.first() == Some(&name.to_string()))
        .map(|(_, args)| args.clone())
        .collect()
}

// ── Pin 1: the instance rest binding excludes the private member —
// `const { ...rest } = other` has type `{}`, so `rest.#prop` reports
// TS2339 (pre-fix the stub returned `C` and the access resolved) ──

const INSTANCE_REST: &str = "\
class C {\n\
    #prop = 1;\n\
    method(other: C) {\n\
        const { ...rest } = other;\n\
        rest.#prop;\n\
    }\n\
}\n\
";

#[test]
fn instance_rest_binding_drops_private_member_pin() {
    // Post-fix (Go f25p1 twin, byte-same): exactly one 2339, `#prop` on `{}`.
    // Pre-fix: silent — the stub's rest type was `C`, so `#prop` resolved.
    let (mut c, file) = check_ts(INSTANCE_REST);
    let ds = diags(&mut c, file);
    let on_prop = errors_2339_on(&ds, "#prop");
    assert_eq!(on_prop.len(), 1, "rest.#prop errors: {ds:?}");
    assert_eq!(on_prop[0][1], "{}", "rest type is the empty rest type: {ds:?}");
}

// ── Pin 2: the static-side rest binding (`const { ...sRest } = C`)
// excludes the private static — the rest type is `{ prototype: C; }` ──

const STATIC_REST: &str = "\
class C {\n\
    static #propStatic = 1;\n\
    method() {\n\
        const { ...sRest } = C;\n\
        sRest.#propStatic;\n\
    }\n\
}\n\
";

#[test]
fn static_rest_binding_drops_private_static_pin() {
    // Post-fix (Go f25p2 twin, byte-same): exactly one 2339, `#propStatic`
    // on `{ prototype: C; }`. Pre-fix: silent.
    let (mut c, file) = check_ts(STATIC_REST);
    let ds = diags(&mut c, file);
    let on_static = errors_2339_on(&ds, "#propStatic");
    assert_eq!(on_static.len(), 1, "sRest.#propStatic errors: {ds:?}");
    assert_eq!(
        on_static[0][1], "{ prototype: C; }",
        "sRest keeps only the public statics: {ds:?}"
    );
}

// ── Pin 3: the no-over-apply guard — the spread-EXPRESSION path
// (`const obj = { ...other }; obj.#prop`) still reports its TS2339 exactly
// as before (it was already Go-identical; the rest fix must not touch it) ──

const SPREAD_EXPRESSION: &str = "\
class C {\n\
    #prop = 1;\n\
    method(other: C) {\n\
        const obj = { ...other };\n\
        obj.#prop;\n\
    }\n\
}\n\
";

#[test]
fn spread_expression_private_access_unchanged_pin() {
    // Both pre- and post-fix (Go f25p3 twin, byte-same): exactly one 2339,
    // `#prop` on `{}`.
    let (mut c, file) = check_ts(SPREAD_EXPRESSION);
    let ds = diags(&mut c, file);
    let on_prop = errors_2339_on(&ds, "#prop");
    assert_eq!(on_prop.len(), 1, "obj.#prop errors: {ds:?}");
    assert_eq!(on_prop[0][1], "{}", "spread type is unchanged: {ds:?}");
}

// ── Pin 4: the omitKeyType machinery — destructured names are omitted
// from the rest type alongside the private members: `{ a, ...rest }` of
// `{ a; #p; b }` yields `{ b: number; }` — `rest.b` stays silent while
// `rest.#p` and `rest.a` both report TS2339 on that exact type ──

const OMIT_KEYS_AND_PRIVATE: &str = "\
class C {\n\
    a = 1;\n\
    #p = 2;\n\
    b = 3;\n\
    method(other: C) {\n\
        const { a, ...rest } = other;\n\
        rest.b;\n\
        rest.#p;\n\
        rest.a;\n\
    }\n\
}\n\
";

#[test]
fn rest_type_omits_destructured_names_and_private_pin() {
    // Post-fix (Go f25p4 twin, byte-same): rest = `{ b: number; }` —
    // `rest.b` resolves (no 2339 for `b`), `rest.#p` and `rest.a` both
    // error on `{ b: number; }`. Pre-fix: rest = `C` minus nothing —
    // `rest.#p` AND `rest.a` resolved silently.
    let (mut c, file) = check_ts(OMIT_KEYS_AND_PRIVATE);
    let ds = diags(&mut c, file);
    assert!(
        errors_2339_on(&ds, "b").is_empty(),
        "rest.b resolves through the spread symbol: {ds:?}"
    );
    let on_p = errors_2339_on(&ds, "#p");
    let on_a = errors_2339_on(&ds, "a");
    assert_eq!(on_p.len(), 1, "rest.#p errors: {ds:?}");
    assert_eq!(on_a.len(), 1, "rest.a errors (omitted by omitKeyType): {ds:?}");
    assert_eq!(on_p[0][1], "{ b: number; }", "the rest member type: {ds:?}");
    assert_eq!(on_a[0][1], "{ b: number; }", "the rest member type: {ds:?}");
}

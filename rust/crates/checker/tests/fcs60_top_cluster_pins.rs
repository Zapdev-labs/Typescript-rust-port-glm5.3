//! FCS60 pins (full-corpus check sweep, micro-tranche 59 — the "FCS tranche
//! 59" PORT notes in-tree; pins named fcs60 by census epoch): the tranche's
//! cluster — the TS2345 inference-candidate family (23 rust-extra TS2345s
//! over 12 records; 9 records converged, 0 newly-divergent) — three stacked
//! roots in the naked-type-parameter candidate resolution:
//!
//!  * getCommonSupertype's strictNullChecks arm (inference.go:1543): under
//!    strictNullChecks every candidate has its NULLABLE constituents
//!    filtered (filterType; `undefined` collapses to neverType and drops
//!    from the supertype walk), the supertype is computed over the FILTERED
//!    list, and the removed nullability is re-added with getNullableType —
//!    the S7-era stub passed the unfiltered list through both arms, so
//!    [undefined, {x,z}|{x,y}] walked to `undefined` and the second/third
//!    arguments over-reported TS2345 (typeArgumentInference a9e/arr,
//!    taggedTemplateStringsTypeArgumentInference{,ES6},
//!    typeArgumentInferenceWithConstraints).
//!
//!  * the union-of-fresh-literals widening (checker.go:18807-18898):
//!    getWidenedTypeWithContext's UNION arm constructs the sibling
//!    WideningContext when the caller passes none, and
//!    getWidenedTypeOfObjectLiteral adds every sibling-literal property the
//!    literal itself lacks as `prop?: undefined` (getPropertiesOfContext /
//!    getSiblingsOfContext / getUndefinedProperty) — the S9 TODO stub added
//!    nothing, so `{x,z} | {x,y}` widened without the cross-constituent
//!    optional-undefined members.
//!
//!  * newAnonymousType's member list (checker.go:25624): Go routes through
//!    setStructuredTypeMembers -> getNamedMembers — the reserved-name/value
//!    filter + the declaration-position sort — where the port's
//!    new_anonymous_type_with_members copied the members table's insertion
//!    order; the sibling `z?: undefined` addition (whose declaration sits at
//!    an EARLIER source position than the widened literal's own members)
//!    printed out of order.

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

fn check_with(src: &str, strict_null_checks: Tristate) -> (Checker, tsc_ast::NodeId) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let program = TestProgram {
        options: CompilerOptions {
            target: ScriptTarget::ESNext,
            strict_null_checks,
            ..CompilerOptions::default()
        },
        name: RootedFilePath::from("/x/fcs60_unit.ts".to_string()),
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

fn codes(ds: &[(i32, Vec<String>)], code: i32) -> Vec<Vec<String>> {
    ds.iter()
        .filter(|(c, _)| *c == code)
        .map(|(_, args)| args.clone())
        .collect()
}

/// The typeArgumentInference a9e shape (strictNullChecks on): the candidate
/// list [undefined, {x,z}, {x,y}] infers `({x,z}|{x,y}) | undefined` — the
/// nullable filtering never walks `undefined` as the common supertype, so
/// the later arguments do NOT over-report TS2345.
#[test]
fn fcs60_common_supertype_strict_null_no_2345_pin() {
    let (mut c, file) = check_with(
        "declare function f<T>(a: T, b: T, c: T): T;\nvar v: number = f(undefined, { x: 6, z: 1 }, { x: 6, y: \"\" });\n",
        Tristate::TSTrue,
    );
    let ds = diags(&mut c, file);
    assert!(
        codes(&ds, 2345).is_empty(),
        "the strictNullChecks common-supertype arm re-adds the filtered nullability: {ds:?}"
    );
    // The 2322 revealer: the inferred T prints with BOTH constituents'
    // sibling optional-undefined members, the nullable re-addition last.
    let e2322 = codes(&ds, 2322);
    assert_eq!(e2322.len(), 1, "one 2322 revealer diagnostic: {ds:?}");
    assert_eq!(
        e2322[0][0],
        "{ x: number; z: number; y?: undefined; } | { z?: undefined; x: number; y: string; } | undefined"
    );
}

/// The non-strict a9e shape (strictNullChecks off): the widening adds the
/// cross-constituent optional-undefined members (`y?: undefined` to the
/// {x,z} arm, `z?: undefined` to the {x,y} arm) and getNamedMembers orders
/// the synthesized member (whose declaration sits in the EARLIER argument)
/// first — the exact Go shape.
#[test]
fn fcs60_widened_union_sibling_undefined_members_pin() {
    let (mut c, file) = check_with(
        "declare function f<T>(a: T, b: T, c: T): T;\nvar v: number = f(undefined, { x: 6, z: 1 }, { x: 6, y: \"\" });\n",
        Tristate::TSFalse,
    );
    let ds = diags(&mut c, file);
    assert!(
        codes(&ds, 2345).is_empty(),
        "no inference mismatch in the non-strict shape: {ds:?}"
    );
    let e2322 = codes(&ds, 2322);
    assert_eq!(e2322.len(), 1, "one 2322 revealer diagnostic: {ds:?}");
    assert_eq!(
        e2322[0][0],
        "{ x: number; z: number; y?: undefined; } | { z?: undefined; x: number; y: string; }"
    );
}

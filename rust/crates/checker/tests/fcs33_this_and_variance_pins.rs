//! FCS33 pins (full-corpus check sweep, tranche 33): the TS2322 family's
//! two largest coherent clusters, each root-caused to a single seam and
//! fixed Go-exactly ("FCS tranche 33" PORT notes in-tree).
//!
//! ROOT 1 — the thisType family (rust-EXTRA 11 instances / 7 records +
//! collateral): `get_declared_type_of_class_or_interface`
//! (typeofsymbol.rs, Go checker.go:17706) set the self-reference's
//! `resolved_type_arguments` to `all_type_parameters` (thisType INCLUDED);
//! Go sets `d.TypeParameters()` (thisType EXCLUDED). The bogus extra
//! type argument made `getTypeWithThisArgument`'s
//! `len(TypeParameters()) == len(typeArguments)` guard bail, so
//! `getApparentType`'s reference arm never substituted `this` with the
//! type-parameter receiver — `x.self()` on `x: U` typed as `() => this`
//! instead of `() => U`, producing spurious `this`-vs-`T`/`U` TS2322s.
//!
//! ROOT 2 — the variance-annotation family (rust-MISSING 8+ instances /
//! 2 records: varianceAnnotations, jsdocTemplateTag8 — plus the
//! un-annotated alias shape): `get_type_alias_type_parameters`
//! (relater.rs, Go relater.go:3430) was the M6 stub returning an empty
//! list, so the alias-variance probe in structuredTypeRelatedToWorker
//! filled BOTH type-argument lists to empty and
//! `typeArgumentsRelatedTo` vacuously succeeded —
//! `Covariant<unknown>` became assignable to `Covariant<string>`.
//!
//! All pins run WITHOUT libs (the corpus twin seam — the per-unit
//! checker), matching the go-bench driver's construction; shapes mirror
//! the conformance records varianceAnnotations.ts and
//! thisTypeAndConstraints.ts (dump-diffed through both drivers).

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

// ── ROOT 1 pins: the thisType family ──

/// Pin 1 (thisTypeAndConstraints.ts shape): `x.self()` on a
/// type-parameter receiver `x: U` types as `() => U` (the apparent-type
/// reference arm substitutes `this` via getTypeWithThisArgument), so
/// `x = x.self()` assigns cleanly — NO 2322. Before the fix the source
/// typed as `this` and rust reported a bogus `this`-vs-`U` 2322.
#[test]
fn fcs33_this_substituted_to_type_parameter_receiver_pin() {
    let src = "class A {\n    self() {\n        return this;\n    }\n}\n\
               class B<T extends A> {\n    bar<U extends T>(x: U) {\n        x = x.self();\n    }\n}\n";
    let (mut c, file) = check_with(src);
    let ds = diags(&mut c, file);
    assert!(
        codes(&ds, 2322).is_empty(),
        "expected no 2322 (this substituted with U); got {ds:?}"
    );
}

/// Pin 2: the induced-error probe — `const n: number = x.self()` reports
/// the source as `U` (Go-faithful substitution), not `this`.
#[test]
fn fcs33_this_return_type_prints_receiver_pin() {
    let src = "class A {\n    self() {\n        return this;\n    }\n}\n\
               class B<T extends A> {\n    bar(x: T) {\n        const n: number = x.self();\n    }\n}\n";
    let (mut c, file) = check_with(src);
    let ds = diags(&mut c, file);
    let d = codes(&ds, 2322);
    assert_eq!(d.len(), 1, "expected exactly one 2322; got {ds:?}");
    assert_eq!(d[0][0], "T", "the method return must print T, not `this`");
    assert_eq!(d[0][1], "number");
}

/// Pin 3: the un-substituted shape — `this.self` INSIDE class A stays
/// `() => this` (the self-reference's default thisArgument is the
/// thisType itself; no regression in the bare-class path).
#[test]
fn fcs33_bare_class_this_unchanged_pin() {
    let src = "class A {\n    self() {\n        return this;\n    }\n    check() {\n        const n: number = this.self;\n    }\n}\n";
    let (mut c, file) = check_with(src);
    let ds = diags(&mut c, file);
    let d = codes(&ds, 2322);
    assert_eq!(d.len(), 1, "expected exactly one 2322; got {ds:?}");
    assert_eq!(d[0][0], "() => this");
    assert_eq!(d[0][1], "number");
}

// ── ROOT 2 pins: the variance-annotation family ──

/// Pin 4 (varianceAnnotations.ts shape): a covariant alias rejects the
/// widening assignment `sub_covariant = super_covariant` — the
/// alias-variance probe now sees the real type-parameter list.
#[test]
fn fcs33_covariant_alias_rejects_widening_pin() {
    let src = "type Covariant<out T> = {\n    x: T;\n}\n\
               declare let super_covariant: Covariant<unknown>;\n\
               declare let sub_covariant: Covariant<string>;\n\
               super_covariant = sub_covariant;\n\
               sub_covariant = super_covariant;  // Error\n";
    let (mut c, file) = check_with(src);
    let ds = diags(&mut c, file);
    let d = codes(&ds, 2322);
    assert_eq!(d.len(), 1, "expected exactly one 2322; got {ds:?}");
    assert_eq!(d[0][0], "Covariant<unknown>");
    assert_eq!(d[0][1], "Covariant<string>");
}

/// Pin 5: a contravariant alias rejects the narrowing assignment only.
#[test]
fn fcs33_contravariant_alias_pin() {
    let src = "type Contravariant<in T> = {\n    f: (x: T) => void;\n}\n\
               declare let super_contravariant: Contravariant<unknown>;\n\
               declare let sub_contravariant: Contravariant<string>;\n\
               super_contravariant = sub_contravariant;  // Error\n\
               sub_contravariant = super_contravariant;\n";
    let (mut c, file) = check_with(src);
    let ds = diags(&mut c, file);
    let d = codes(&ds, 2322);
    assert_eq!(d.len(), 1, "expected exactly one 2322; got {ds:?}");
    assert_eq!(d[0][0], "Contravariant<string>");
    assert_eq!(d[0][1], "Contravariant<unknown>");
}

/// Pin 6: an invariant alias rejects both directions.
#[test]
fn fcs33_invariant_alias_pin() {
    let src = "type Invariant<in out T> = {\n    f: (x: T) => T;\n}\n\
               declare let super_invariant: Invariant<unknown>;\n\
               declare let sub_invariant: Invariant<string>;\n\
               super_invariant = sub_invariant;  // Error\n\
               sub_invariant = super_invariant;  // Error\n";
    let (mut c, file) = check_with(src);
    let ds = diags(&mut c, file);
    assert_eq!(codes(&ds, 2322).len(), 2, "expected two 2322s; got {ds:?}");
}

/// Pin 7: the UN-ANNOTATED alias — measured (marker-probe) variance
/// still rejects the widening assignment (`Bland<unknown>` to
/// `Bland<string>`), matching Go's getVariancesWorker measurement.
#[test]
fn fcs33_unannotated_alias_measured_variance_pin() {
    let src = "type Bland<T> = { x: T; }\n\
               declare let super_b: Bland<unknown>;\n\
               declare let sub_b: Bland<string>;\n\
               sub_b = super_b;  // Error\n";
    let (mut c, file) = check_with(src);
    let ds = diags(&mut c, file);
    let d = codes(&ds, 2322);
    assert_eq!(d.len(), 1, "expected exactly one 2322; got {ds:?}");
    assert_eq!(d[0][0], "Bland<unknown>");
    assert_eq!(d[0][1], "Bland<string>");
}

/// Pin 8 (negative): a covariant alias accepts the narrowing assignment
/// with no 2322 — the fix must not over-report.
#[test]
fn fcs33_covariant_alias_accepts_narrowing_pin() {
    let src = "type Covariant<out T> = {\n    x: T;\n}\n\
               declare let super_covariant: Covariant<unknown>;\n\
               declare let sub_covariant: Covariant<string>;\n\
               super_covariant = sub_covariant;\n";
    let (mut c, file) = check_with(src);
    let ds = diags(&mut c, file);
    assert!(
        codes(&ds, 2322).is_empty(),
        "covariant narrowing must stay clean; got {ds:?}"
    );
}

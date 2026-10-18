//! FCS45 pins (full-corpus check sweep, tranche 45): the top coherent
//! clusters of the fresh t45-final-dump census — TS2502 rust-MISSING
//! x14/6rec (the recursiveTypesWithTypeof "referenced directly or
//! indirectly in its own type annotation" family) and the TS2344/1338
//! rust-EXTRA x22+1 family (inferTypes1 — the dormant S9
//! contextual-return seam).
//! ("FCS tranche 45" PORT notes in-tree.)
//!
//! TWO roots, both Go-exact:
//!
//! ROOT A — TS2502 (+ the 7022/2303 arms of the same function): Go's
//! `reportCircularityError` (checker.go:19229) reports TS2502 when the
//! symbol's value declaration carries a TYPE ANNOTATION (returning
//! `errorType`), TS7022 when it is un-annotated under noImplicitAny and
//! is not a bare parameter (or has an initializer), TS2303 for alias
//! symbols without a value declaration, and returns `anyType` outside
//! the 2502 arm. The port's spike stub returned errorType SILENTLY —
//! every 2502 in the corpus was dropped.
//!
//! ROOT B — TS1338 + TS2344: (1) Go's `checkInferType`
//! (checker.go:3322) runs `ast.FindAncestor` from the node ITSELF
//! (ast/utilities.go:911 starts the candidate walk at `node`), so an
//! `infer B` that IS the extendsType of its own parent conditional
//! (`(infer A) extends infer B ? ...`) matches immediately; the port
//! started at the parent and over-fired TS1338 on exactly those.
//! (2) Go's `getInferredTypeParameterConstraint` type-reference arm
//! (checker.go:17534-17551) gives `Foo<infer T>`'s T the constraint of
//! Foo's corresponding type parameter (instantiated through a DEFERRED
//! type mapper, discarding self-referential inferences); the port had
//! the TODO(port M6 S5/S7) `let _ = ...` stub, so every infer param in
//! a type reference stayed unconstrained and
//! `checkTypeArgumentConstraints` over-fired TS2344 on the extends
//! clause's `Foo<infer U>` nodes.

use tsc_ast::{NodeStore, SourceFile, SourceFileParseOptions, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::options_generated::{CompilerOptions, ModuleKind, ScriptTarget};
use tsc_core::scriptkind::ScriptKind;
use tsc_core::tristate::Tristate;
use tsc_parser::parse_source_file;
use tsc_tspath::RootedFilePath;

// ── The single-file Program scaffold (the fcs43 idiom) ──

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

/// The check twin's unit parse (the fcs43 idiom).
fn check_unit(src: &str, name: &str, options: CompilerOptions) -> (Checker, tsc_ast::NodeId) {
    let file_name = RootedFilePath::from(name);
    let mut sf = parse_source_file(
        SourceFileParseOptions {
            file_name,
            ..Default::default()
        },
        src,
        ScriptKind::TS,
    );
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

// ── ROOT A — TS2502 (reportCircularityError) ──

/// The recursiveTypesWithTypeof.ts shape (ROOT A): `var c: typeof c;`
/// circularly references itself in its own type annotation and reports
/// exactly one TS2502 with the variable's name as the argument. The
/// pre-tranche port's silent errorType stub dropped every instance.
#[test]
fn fcs45_circular_annotation_2502_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::CommonJS,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "var c: typeof c;\nvar c: any;\n",
        "/x/circularAnnotation.ts",
        options,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 2502), 1, "expected one TS2502, got: {ds:?}");
    let ones: Vec<_> = ds.iter().filter(|(k, _)| *k == 2502).collect();
    assert!(
        ones.iter().any(|(_, args)| args.first().map(String::as_str) == Some("c")),
        "expected the 'c' arg, got: {ds:?}"
    );
}

/// The indirect pair (recursiveTypesWithTypeof's `d`/`e`): two
/// declarations cross-referencing through `typeof` — BOTH report TS2502
/// (each pushes its own resolution, each falls out circular).
#[test]
fn fcs45_circular_annotation_pair_2502_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::CommonJS,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "var d: typeof e;\nvar d: any;\nvar e: typeof d;\nvar e: any;\n",
        "/x/circularPair.ts",
        options,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 2502), 2, "expected two TS2502, got: {ds:?}");
}

/// The truly-recursive control (recursiveTypesWithTypeof's `g`): a
/// declaration whose annotation references itself only INSIDE a type
/// literal is a valid circular definition — NO TS2502 (the deferred
/// resolution completes).
#[test]
fn fcs45_truly_recursive_no_2502_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::CommonJS,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "var g: { x: typeof g; };\nvar g: typeof g.x;\n",
        "/x/trulyRecursive.ts",
        options,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 2502), 0, "unexpected TS2502, got: {ds:?}");
}

/// The 7022 arm of the same function (ROOT A): an un-annotated variable
/// that circularly references itself in its INITIALIZER reports exactly
/// one TS7022 under noImplicitAny (the pre-tranche port reported
/// nothing for the whole family).
#[test]
fn fcs45_circular_initializer_7022_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::CommonJS,
        no_implicit_any: Tristate::TSTrue,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "var q = q;\n",
        "/x/circularInitializer.ts",
        options,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 7022), 1, "expected one TS7022, got: {ds:?}");
    assert_eq!(
        count(&ds, 7022),
        ds.iter()
            .filter(|(k, a)| *k == 7022 && a.first().map(String::as_str) == Some("q"))
            .count(),
        "expected the 'q' arg, got: {ds:?}"
    );
}

// ── ROOT B(1) — TS1338 (checkInferType's FindAncestor start node) ──

/// The inferTypes1.ts T60/T61 shape (ROOT B1): a bare `infer U` in a
/// type-alias body reports TS1338, and `(infer A) extends infer B ?`
/// reports TS1338 for the A/C/D positions but NOT for the extends
/// position `infer B` (it IS the extendsType of its own parent
/// conditional). Pre-tranche the port over-fired on `infer B`.
#[test]
fn fcs45_infer_extends_position_no_1338_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::CommonJS,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "type T60 = infer U;\ntype T61<T> = (infer A) extends infer B ? infer C : infer D;\n",
        "/x/inferExtends.ts",
        options,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 1338), 4, "expected four TS1338, got: {ds:?}");
}

/// The all-permitted control (inferTypes1's T63): every `infer` inside
/// a nested conditional wrapped in parens in the OUTER conditional's
/// extends clause is position-valid — NO TS1338.
#[test]
fn fcs45_nested_conditional_infer_no_1338_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::CommonJS,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "type T63<T> = T extends ((infer A) extends infer B ? infer C : infer D) ? string : number;\n",
        "/x/nestedInfer.ts",
        options,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 1338), 0, "unexpected TS1338, got: {ds:?}");
}

// ── ROOT B(2) — TS2344 (getInferredTypeParameterConstraint's
//    type-reference arm) ──

/// The inferTypes1.ts T71 shape (ROOT B2): `Foo<infer U>` in the
/// extends clause gives U Foo's constraint, so the TRUE branch's
/// `Foo<U>` satisfies the constraint — NO TS2344. Pre-tranche the
/// extends-clause infer stayed unconstrained and BOTH the infer node
/// and the true-branch argument over-fired.
#[test]
fn fcs45_infer_inherits_reference_constraint_no_2344_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::CommonJS,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "type T70<T extends string> = { x: T };\ntype T71<T> = T extends T70<infer U> ? T70<U> : never;\n",
        "/x/inferConstraintOk.ts",
        options,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 2344), 0, "unexpected TS2344, got: {ds:?}");
}

/// The inferTypes1.ts T73 shape (ROOT B2): when the extends clause
/// infers U against a NUMBER-constrained parameter but the true branch
/// needs STRING, exactly ONE TS2344 fires — at the true branch's type
/// argument (the Go behavior; the extends-clause infer itself stays
/// quiet).
#[test]
fn fcs45_infer_constraint_conflict_2344_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::CommonJS,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "type T70<T extends string> = { x: T };\ntype T72<T extends number> = { y: T };\ntype T73<T> = T extends T72<infer U> ? T70<U> : never;\n",
        "/x/inferConstraintConflict.ts",
        options,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 2344), 1, "expected one TS2344, got: {ds:?}");
}

/// The discard-self-inference control (Go checker.go:17538's own
/// example): in `Foo<T extends string, U extends T>` the instantiated
/// constraint for the SECOND parameter IS the inferred X, so that
/// inference is DISCARDED and only the string side constrains —
/// `Foo<infer X, infer X>` reports NO TS2344.
#[test]
fn fcs45_infer_discard_self_inference_no_2344_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::CommonJS,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "type Foo<T extends string, U extends T> = [T, U];\ntype Bar<T> = T extends Foo<infer X, infer X> ? Foo<X, X> : T;\n",
        "/x/inferDiscard.ts",
        options,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 2344), 0, "unexpected TS2344, got: {ds:?}");
}

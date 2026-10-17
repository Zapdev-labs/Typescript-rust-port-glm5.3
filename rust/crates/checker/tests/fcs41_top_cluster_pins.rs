//! FCS41 pins (full-corpus check sweep, tranche 41): the top-3 census
//! clusters of the fresh t40-final-dump census — TS2446 rust-extra
//! x43/9rec (the protected-instance-access nested-class family),
//! TS2441 rust-missing x32/8rec ("Compiler reserves name 'require'/
//! 'exports' in top level scope of a module", the node
//! nodeModulesGeneratedNameCollisions family), TS2558 both-directions
//! x73/10rec ("Expected 0 type arguments, but got 1" — a pure diagnostic
//! SPAN variance: same instances, same counts, Go spans the
//! type-argument list while the port spanned the whole call).
//! ("FCS tranche 41" PORT notes in-tree.)
//!
//! THREE roots, all Go-exact:
//!
//! ROOT A — TS2446: `isClassDerivedFromDeclaringClasses` (Go
//! checker.go:12202) was a permissive stub answering
//! `get_declaring_class(prop).is_some()` — the enclosing-class walk
//! (checker.go:12097-12104) then stopped at the FIRST container class
//! and `hasBaseType(containingType, enclosingClass)` fired 2446 on
//! every nested-class access Go resolves through the outer class. The
//! real body walks the property's synthetic constituents
//! (forEachProperty, checker.go:12145) against `hasBaseType(class,
//! getDeclaringClass(p))`.
//!
//! ROOT B — TS2558: `getTypeArgumentArityError` (Go checker.go:10066)
//! builds the diagnostic with `loc := core.NewTextRange(
//! scanner.SkipTrivia(sourceFile.Text(), typeArgumentList.Loc.Pos()),
//! typeArgumentList.Loc.End())` — the type-argument list's span. The
//! port built it on the whole call node (`new_diagnostic_for_node`),
//! widening every span (e.g. `f<number>()` vs Go's `number`).
//!
//! ROOT C — TS2441: two mistranslations in
//! `checkCollisionWithRequireExportsInGeneratedCode` (Go
//! checker.go:10692): (1) the module-format gate read the configured
//! module_kind (node16 >= ES2015) instead of the PER-FILE emit format
//! (GetEmitModuleFormatOfFile — CommonJS for a .ts unit with no
//! package.json "type", Go's SkipModuleResolution metadata shape), so
//! the whole check never ran under node16/nodenext; (2) the
//! reserved-name gate `!(need("require") && !need("exports"))` dropped
//! the exports-named arm (Go: `!need("require") && !need("exports")`).
//! The statements.rs seam is gone — all six call sites read the checker
//! Program trait, which the testrunner's CheckerUnitProgram overrides
//! with the compiler crate's worker.

use tsc_ast::{NodeStore, SourceFile, SourceFileParseOptions, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::options_generated::{CompilerOptions, ModuleKind, ScriptTarget};
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
    /// The per-file emit format (the CheckerUnitProgram override shape —
    /// ROOT C's pins need the CommonJS answer a .ts unit carries under
    /// node16/nodenext with no package.json "type").
    emit_format: ModuleKind,
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
        self.emit_format
    }
}

/// The check twin's unit parse (post-tranche-40 shape): the REAL rooted
/// unit name rides the parse options.
fn check_unit(
    src: &str,
    name: &str,
    options: CompilerOptions,
    emit_format: ModuleKind,
) -> (Checker, tsc_ast::NodeId) {
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
        emit_format,
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

/// The (code, span) list — ROOT B's pin reads the diagnostic LOCATION.
fn diags_with_span(c: &mut Checker) -> Vec<(i32, usize, usize)> {
    c.semantic_diagnostics()
        .iter()
        .map(|d| {
            let loc = d.loc();
            (d.code(), loc.pos() as usize, loc.end() as usize)
        })
        .collect()
}

fn count(ds: &[(i32, Vec<String>)], code: i32) -> usize {
    ds.iter().filter(|(c, _)| *c == code).count()
}

// ── ROOT A — TS2446 (the protected-instance-access nested-class family) ──

/// The protectedClassPropertyAccessibleWithinNestedClass.ts shape
/// (ROOT A): the access happens inside a NESTED class (C2) whose outer
/// class (C) declares the protected member — Go's enclosing-class walk
/// skips C2 (not derived from C) and lands on C, so the C-typed
/// instance access is legal: NO TS2446. The pre-tranche stub stopped
/// the walk at C2 and fired 2446 on all eight accesses.
#[test]
fn fcs41_nested_class_protected_access_no_2446_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::CommonJS,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "class C {\n    protected x: string;\n    protected foo() { return this.foo; }\n    protected bar() {\n        class C2 {\n            protected foo() {\n                let y = new C();\n                var y1 = y.foo;\n                var y3 = y.x;\n            }\n        }\n    }\n}\n",
        "/x/protectedClassPropertyAccessibleWithinNestedClass.ts",
        options,
        ModuleKind::CommonJS,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 2446), 0, "unexpected TS2446, got: {ds:?}");
}

/// The derivedClassSuperProperties-adjacent positive control (ROOT A):
/// inside Derived2's nested class, a Derived1-typed instance access
/// fires the REAL TS2446 — the enclosing class resolves to Derived2
/// (derived from Base) and `hasBaseType(Derived1, Derived2)` is false.
/// The stub ALSO fired here (same wrong span), so this pin guards the
/// real-body's true-negative arm.
#[test]
fn fcs41_wrong_enclosing_class_instance_reports_2446_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::CommonJS,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "class Base { protected x: string; }\nclass Derived1 extends Base {}\nclass Derived2 extends Base {\n    method2() {\n        class C {\n            m() {\n                let d1: Derived1 = null as any;\n                d1.x;\n            }\n        }\n    }\n}\n",
        "/x/protectedClassPropertyAccessibleWithinNestedSubclass1.ts",
        options,
        ModuleKind::CommonJS,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 2446), 1, "expected one TS2446, got: {ds:?}");
}

// ── ROOT B — TS2558 (the type-argument-list span) ──

/// The callNonGenericFunctionWithTypeArguments.ts shape (ROOT B): the
/// 2558 diagnostic's span is the SkipTrivia'd TYPE-ARGUMENT LIST — the
/// `number` between the brackets — NOT the whole call expression.
#[test]
fn fcs41_arity_error_span_is_type_argument_list_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::CommonJS,
        ..CompilerOptions::default()
    };
    let src = "function f() {}\nf<number>();\n";
    let (mut c, _) = check_unit(src, "/x/callNonGenericFunctionWithTypeArguments.ts", options, ModuleKind::CommonJS);
    let ds = diags_with_span(&mut c);
    let spans: Vec<(usize, usize)> = ds
        .iter()
        .filter(|(code, _, _)| *code == 2558)
        .map(|(_, p, e)| (*p, *e))
        .collect();
    assert_eq!(spans.len(), 1, "expected one TS2558, got: {ds:?}");
    let (p, e) = spans[0];
    assert_eq!(
        &src[p..e],
        "number",
        "TS2558 span must cover the type-argument list, got src[{p}..{e}]"
    );
}

// ── ROOT C — TS2441 (the reserves-name family) ──

/// The nodeModulesGeneratedNameCollisions.ts subfolder/index.ts shape
/// (ROOT C): a CommonJS-format .ts unit under node16 that declares
/// `require` and `exports` at the module's top level reports TS2441
/// TWICE — once per reserved name. The pre-tranche port fired ZERO
/// (the node16 >= ES2015 gate skipped the whole check, and the
/// exports-name arm was unreachable).
#[test]
fn fcs41_cjs_unit_reserves_require_and_exports_2441_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2022,
        module: ModuleKind::Node16,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "function require() {}\nconst exports = {};\nexport {require, exports};\n",
        "/x/subfolder/index.ts",
        options,
        ModuleKind::CommonJS,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 2441), 2, "expected two TS2441, got: {ds:?}");
}

/// The index.ts control (ROOT C): the SAME declarations in an
/// ES-module-format unit do NOT report TS2441 (Go's >= ES2015 gate).
#[test]
fn fcs41_esm_unit_no_2441_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2022,
        module: ModuleKind::Node16,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "function require() {}\nconst exports = {};\nexport {require, exports};\n",
        "/x/index.ts",
        options,
        ModuleKind::ES2015,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 2441), 0, "unexpected TS2441, got: {ds:?}");
}

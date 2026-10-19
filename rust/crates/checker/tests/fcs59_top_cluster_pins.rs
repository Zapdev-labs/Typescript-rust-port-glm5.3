//! FCS59 pins (full-corpus check sweep, micro-tranche 59 — the "FCS tranche
//! 58" census epoch's queued targets, worked directly by the orchestrator):
//! the tranche's families —
//!
//!  * the TS7010->TS7055 generator-yield family (generators/
//!    generatorReturnTypeInferenceNonStrict x4/4, rust extra TS7010 where
//!    Go reports TS7055 "yield type"): Go's reportImplicitAny function-
//!    family arm (checker.go:18736-18747) selects the YIELD message for
//!    WideningKind.GeneratorYield and handles the Reparsed overload arm;
//!    the port always selected the RETURN message (TS7010) and had the
//!    Reparsed arm as a TODO. See typeofsymbol.rs "FCS tranche 58".
//!
//!  * the TS2536 intersection-indexed-access family
//!    (types/intersection/intersectionReduction{,Strict} x4 + keyof
//!    collateral): Go's checkIndexedAccessIndexType everyType walk
//!    (checker.go:8436) distributes over UNION constituents ONLY; an
//!    intersection index type (the mapped-type `U[P & keyof U]` shape) is
//!    checked as a whole, where the relation's someType-over-intersection /
//!    isTypeDerivedFrom arms answer via the identical keyof member. The
//!    port walked INTERSECTION members individually, so the type-parameter
//!    member P failed the keyof-U probe and TS2536 fired rust-extra. See
//!    expressions.rs "FCS tranche 58".
//!
//!  * the TS7022 loop-carried-initializer family (controlFlow/
//!    controlFlowIterationErrors{,Async} x7, rust extra TS7022 where Go
//!    reports nothing / a single TS18046): two stacked roots. (1) Go's
//!    getQuickTypeOfExpression CALL arm (checker.go:7573-7583) resolves
//!    the callee's single non-generic call-signature return WITHOUT
//!    checking the arguments — the FCS-tranche-26 parking left the arm
//!    inert, so every un-annotated `let y = f(x)` initializer re-checked
//!    its arguments through checkExpressionCachedEx (which RESETS the
//!    flow loop state mid back-antecedent walk), diverging loop-carried
//!    references like `let y = f(x); x = y + 1` into TS7022 circularity
//!    errors (Go instead resolves y quickly and the loop junction
//!    in-process branch terminates the nested walk). (2) The re-landing
//!    exposed the tranche-26 "spread seam": the 1117 duplicate-property
//!    grammar arm early-returned after the first report where Go
//!    (grammarchecks.go:1121) CONTINUES, so `{ b: 1, ...o, b: 2, ...o2,
//!    b: 3 }` dropped the b:3 TS1117. Both landed with the arm. See
//!    spine.rs + grammarchecks.rs "FCS tranche 58".
//!
//! ("FCS tranche 58" PORT notes in-tree; pins named fcs59 by census epoch.)

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
        name: RootedFilePath::from("/x/fcs59_unit.ts".to_string()),
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
    // The generator corpus case (@noImplicitAny: true, @strictNullChecks:
    // false, @target: esnext): under strictNullChecks the undefined
    // widening marker is not created (create_widening_type's first arm),
    // so the implicit-any yield report never forms.
    check_with(
        src,
        CompilerOptions {
            target: ScriptTarget::ESNext,
            no_implicit_any: Tristate::TSTrue,
            strict_null_checks: Tristate::TSFalse,
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

// ── CLUSTER A — the TS7055 generator-yield family (7010 extra gone) ──

/// The generatorReturnTypeInferenceNonStrict g301 shape: a named
/// generator function lacking a return-type annotation whose yield type
/// is the implicit-any bare `yield;` — Go reports TS7055 "'g', which
/// lacks return-type annotation, implicitly has an 'any' yield type."
/// (the GeneratorYield widening kind), NOT the TS7010 "return type"
/// message the port always selected.
#[test]
fn fcs59_generator_yield_type_not_return_type_pin() {
    let (mut c, file) = check_strict("function* g001() {\n    yield;\n}\n");
    let ds = diags(&mut c, file);
    assert!(
        codes(&ds, 7010).is_empty(),
        "generator widening must not report the TS7010 return-type message: {ds:?}"
    );
    let e7055 = codes(&ds, 7055);
    assert_eq!(e7055.len(), 1, "one TS7055 yield-type diagnostic: {ds:?}");
    assert_eq!(e7055[0][0], "g001", "the diagnostic names the generator");
    assert_eq!(e7055[0][1], "any", "the implicit yield type is any");
}

// ── CLUSTER B — the TS2536 intersection-indexed-access family ──

/// The intersectionReduction Merge2 shape: the mapped-type
/// `U[P & keyof U]` indexed access — Go's everyType distributes over
/// unions ONLY, so the intersection `P & keyof U` is checked as a whole
/// (someType semantics: the identical keyof-U member relates) and NO
/// TS2536 "Type 'P & keyof U' cannot be used to index type 'U'" fires.
#[test]
fn fcs59_intersection_indexed_access_no_2536_pin() {
    let (mut c, file) = check_strict(
        "type Merge2<T, U> = { [P in keyof T | keyof U]: P extends keyof T ? T[P] : U[P & keyof U] }\ntype M3 = Merge2<{ a: 1, b: 2 }, { a: 2, c: 3 }>;\n",
    );
    let ds = diags(&mut c, file);
    assert!(
        codes(&ds, 2536).is_empty(),
        "the intersection index type checks as a whole (Go everyType is union-only): {ds:?}"
    );
}

// ── CLUSTER C — the TS7022 loop-carried-initializer family ──

/// The controlFlowIterationErrors h3 shape: the loop-carried
/// `let y = asNumber(x); x = y + 1;` — with the quick-type CALL arm live,
/// the initializer resolves without re-checking its arguments (no
/// mid-walk flow-state reset), so no TS7022 "'y' implicitly has type
/// 'any' because it ... is referenced directly or indirectly in its own
/// initializer" circularity fires and `y` types as number.
#[test]
fn fcs59_loop_carried_const_no_7022_pin() {
    let (mut c, file) = check_strict(
        "declare function asNumber(x: string | number): number;\ndeclare const cond: boolean;\nfunction h3() {\n    let x: string | number | boolean;\n    x = \"0\";\n    while (cond) {\n        let y = asNumber(x);\n        x = y + 1;\n        x;\n    }\n}\n",
    );
    let ds = diags(&mut c, file);
    assert!(
        codes(&ds, 7022).is_empty(),
        "the loop-carried const resolves via the quick type without a circularity: {ds:?}"
    );
}

/// The objectSpreadNegative duplicated shape: `{ b: 1, ...o, b: 2, ...o2,
/// b: 3 }` — Go's 1117 grammar arm does NOT early-return, so BOTH later
/// duplicates report (b:2 AND b:3); the tranche-26 early return dropped
/// the b:3 report.
#[test]
fn fcs59_spread_duplicate_1117_reports_every_later_duplicate_pin() {
    let (mut c, file) = check_strict(
        "declare const o: { b: 1 };\ndeclare const o2: { b: 2 };\nlet duplicated = { b: 3, ...o, b: 4, ...o2, b: 5 };\n",
    );
    let ds = diags(&mut c, file);
    let e1117 = codes(&ds, 1117);
    assert_eq!(
        e1117.len(),
        2,
        "both later duplicate properties report TS1117 (Go continues after the first): {ds:?}"
    );
}

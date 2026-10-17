//! FCS38 pins (full-corpus check sweep, tranche 38): the top-3 census
//! clusters of the fresh t37-final-dump census — TS7006 rust-extra
//! x98/9rec (the decorator-factory arrow-parameter family), TS2504
//! rust-missing x75/26rec (the asyncGenerators `yield*` family), TS7059
//! rust-missing x64/4rec (nodeModulesForbidenSyntax's .mts/.cts
//! reserved-syntax family). ("FCS tranche 38" PORT notes in-tree.)
//!
//! THREE roots, all probe-evidenced on the rig (temporary eprintln
//! probes, removed; evidence in ~/.fcs38-continuation.md):
//!
//! ROOT A — TS7006: `new_decorator_call_signature` /
//! `new_function_type_for_decorator` (statements.rs) anchored their
//! signatures at `declaration: None`, so `get_or_create_type_from_signature`
//! took its declaration-less fallback (`kind == KindUnknown =>
//! isConstructor`) and stored the decorator context CALL signature as a
//! CONSTRUCT signature — `get_contextual_call_signature(Call)` found
//! nothing, `contextuallyCheckFunctionExpressionOrObjectLiteralMethod`
//! fell to `assignNonContextualParameterTypes`, and every decorator
//! factory arrow parameter reported a phantom TS7006. Go anchors at a
//! factory-made synthetic FunctionTypeNode (checker.go:31067-31070);
//! the port now does too (`new_synthetic_function_type_node`).
//!
//! ROOT B — TS2504: `produce_deferred_diagnostics`' ReportTypeNotIterable
//! arm (spine.rs) was the M6 S3 TODO stub discarding the queued callback
//! — the iteration worker had queued the "Type '{}' must have a
//! Symbol.asyncIterator method" 2504 (Go checker.go:6547-6554) but the
//! drain dropped it. The arm now runs `report_type_not_iterable_error`
//! + the related-info merge.
//!
//! ROOT C — TS7059: `check_assertion`'s mts/cts guard
//! (checker.go:12528-12531) was a permissive skip; the naive port read
//! the SourceFile's own file name, which is EMPTY in the check twin
//! (the §5.1 parse keeps SourceFiles anonymous), so the guard rides the
//! FCS-tranche-6 `source_file_name` Program seam.
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

/// The check twin: parse + bind + check one unit under `strict`
/// (noImplicitAny is what makes the TS7006 family observable) at the
/// given target, with the unit's rooted name carried by the
/// `source_file_name` seam (ROOT C reads it).
fn check_with_named(src: &str, name: &str, target: ScriptTarget) -> (Checker, tsc_ast::NodeId) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let options = CompilerOptions {
        target,
        strict: Tristate::TSTrue,
        ..CompilerOptions::default()
    };
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
    check_with_named(src, "/x/unit.ts", target)
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

/// The esDecorators-contextualTypes.ts shape (ROOT A): the decorator
/// factory arrow `@((t, c) => { })` — its `t`/`c` parameters are
/// contextually typed by the decorator's synthetic call signature
/// (target, context), so NO TS7006 may fire under noImplicitAny. The
/// pre-tranche port stored that signature as a CONSTRUCT signature, so
/// the contextual signature lookup failed and both parameters reported
/// phantom TS7006s (42 instances in the corpus record).
#[test]
fn fcs38_decorator_arrow_params_not_implicit_any_pin() {
    let src = "@((t, c) => { })\nclass C { }";
    let (mut c, _) = check_with(src, ScriptTarget::ESNext);
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 7006), 0, "decorator factory params must be contextually typed: {ds:?}");
    // The class decorator's arrow must also not trip the "accepts too few
    // arguments" probe (1329) — the contextual signature carries both
    // parameters.
    assert_eq!(count(&ds, 1329), 0, "unexpected decorator-arity error: {ds:?}");
}

/// The legacyDecorators-contextualTypes.ts shape (ROOT A, the legacy
/// arm — `get_legacy_decorator_call_signature` builds the same
/// anchored signature through `new_decorator_call_signature`):
/// `@dec((t, c) => { })` with `@dec` a decorator FACTORY — the arrow
/// argument's `t`/`c` parameters are contextually typed by the
/// factory's annotated parameter `(t: number, c: number) => void`, so
/// no TS7006 may fire (the pre-tranche port stored the contextual
/// signature as a construct signature and reported phantom 7006s).
#[test]
fn fcs38_legacy_decorator_factory_contextual_params_pin() {
    let src = "function dec(f: (t: number, c: number) => void) { }\n@dec((t, c) => { })\nclass C { }";
    let (mut c, _) = check_with(src, ScriptTarget::ES2015);
    let ds = diags(&mut c);
    assert_eq!(
        count(&ds, 7006),
        0,
        "the factory argument's arrow parameters must be contextually typed: {ds:?}"
    );
}

/// The emitter.asyncGenerators.objectLiteralMethods O4.ts shape (ROOT
/// B): `yield* [1]` in an async generator method of a lib-less unit —
/// the operand type has no `[Symbol.asyncIterator]` (nor
/// `[Symbol.iterator]` without libs), so the iteration worker queues
/// the deferred TS2504 and the drain must produce it. The pre-tranche
/// port queued it and dropped it (the corpus's 75 missing 2504s).
#[test]
fn fcs38_async_yield_star_reports_2504_pin() {
    let src = "const o4 = {\n    async * f() {\n        const x = yield* [1];\n    }\n}\n";
    let (mut c, _) = check_with(src, ScriptTarget::ES2015);
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 2504), 1, "the yield* operand must report TS2504: {ds:?}");
}

/// The non-async yield* counterpart: a SYNC generator's `yield*` operand
/// with no `[Symbol.iterator]` (lib-less) reports the SYNC form TS2488
/// ("must have a Symbol.iterator method") — same deferred drain, the
/// allowAsyncIterables=false arm.
#[test]
fn fcs38_sync_yield_star_reports_2488_pin() {
    let src = "const o4 = {\n    * f() {\n        const x = yield* [1];\n    }\n}\n";
    let (mut c, _) = check_with(src, ScriptTarget::ES2015);
    let ds = diags(&mut c);
    assert_eq!(
        count(&ds, 2488),
        1,
        "the sync yield* operand must report TS2488: {ds:?}"
    );
}

/// The nodeModulesForbidenSyntax index.cts shape (ROOT C): `<any>x` type
/// assertions are reserved in .mts/.cts files — TS7059. The file name
/// rides the `source_file_name` seam (the twin's SourceFile is
/// anonymous), mirroring Go's `file.FileName().ExtensionIsOneOf(...)`.
#[test]
fn fcs38_type_assertion_reserved_in_cts_pin() {
    let src = "let a = <any>1;\n";
    let (mut c, _) = check_with_named(src, "/x/index.cts", ScriptTarget::ES2015);
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 7059), 1, "the .cts assertion must report TS7059: {ds:?}");
}

/// The .ts counterpart: the same assertion in a plain .ts unit reports
/// NO TS7059 (the guard is extension-scoped, Go-exact).
#[test]
fn fcs38_type_assertion_allowed_in_plain_ts_pin() {
    let src = "let a = <any>1;\n";
    let (mut c, _) = check_with_named(src, "/x/index.ts", ScriptTarget::ES2015);
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 7059), 0, "plain .ts assertions are not reserved: {ds:?}");
}

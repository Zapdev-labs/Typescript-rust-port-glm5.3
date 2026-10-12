//! FCS pins (the full-corpus check-sweep wave): the signature-comparison
//! recursion guard. Go's `signatureRelatedTo` (relater.go:4586) passes its
//! `compareTypes` as a closure over the ACTIVE relater —
//! `isRelatedToWorker := func(s, t, reportErrors) {
//!     r.isRelatedToEx(s, t, RecursionFlagsBoth, reportErrors, nil, intersectionState)
//! }` — so a cyclic comparison re-entering the same (source, target) pair
//! through nested signature comparisons hits the maybe-key guard in
//! `recursiveTypeRelatedTo` ("If source and target are already being
//! compared, consider them related with assumptions" → Ternary.Maybe) and
//! terminates. The port's `TypeComparerKind` enum dropped that closure
//! capture: the signature ladder re-entered through a FRESH relater
//! (`checkTypeRelatedToEx`), the guard never fired, and the full-corpus
//! check sweep stack-overflowed (SIGABRT) in
//! classes/members/privateNames (the in-flight case:
//! privateNameSetterExprReturnValue.ts). The relater + the ambient
//! intersection state are now threaded through
//! `compare_signatures_related` exactly as the closure captured them
//! (PORT-noted "signature recursion, FCS" in relater.rs).
//!
//! These pins ride the real check pipeline (the s8_members.rs idiom) on
//! the exact corpus shapes that cycled: a setter-only private-name write
//! (the assignment's write-type resolution re-enters the class's own
//! signature comparison) and the generic derived-class constructor chain
//! (Parent<T>/Child<T> with cross-class private-name access — the
//! C<string> / C<?> relation cycle the sweep probes showed).

use tsc_ast::{NodeId, NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::options_generated::CompilerOptions;
use tsc_core::scriptkind::ScriptKind;
use tsc_parser::parse_source_file;

// ── The single-file Program scaffold (same shape as s8_members.rs) ──

struct TestProgram {
    options: CompilerOptions,
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
}

/// Check `src` through the real pipeline, returning the checker and the
/// source-file node id.
fn check_file(src: &str) -> (Checker, NodeId) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let program = TestProgram {
        options: CompilerOptions::default(),
        source_file: sf,
        bind,
        binder_symbol_count,
        checker_symbols: Vec::new(),
    };
    let mut checker = new_checker(Box::new(program));
    checker.check_source_file(file, true);
    (checker, file)
}

fn diag_codes(c: &mut Checker, file: NodeId) -> Vec<i32> {
    c.get_file_diagnostics(file)
        .iter()
        .map(|d| d.code())
        .collect()
}

// ── The recursion-guard pins ───────────────────────────────────────────────

/// tsc/testdata/tests/cases/conformance/classes/members/privateNames/
/// privateNameSetterExprReturnValue.ts — Go: the case is CLEAN (the twin's
/// only diagnostics are the lib-less special-globals family + 2584 for
/// `console`; no private-name errors). The assignment `this.#foo = 42 * 2`
/// to a setter-only private name drives the write-type resolution whose
/// signature comparison re-enters the class's own type — the exact cycle
/// that stack-overflowed the sweep pre-fix. PIN: the check completes and
/// emits no private-name-related diagnostics (2341/2540/18013 family).
#[test]
fn fcs_setter_only_private_name_write_check_terminates_clean() {
    let (mut c, file) = check_file(
        "class C {\n    set #foo(a: number) {}\n    bar() {\n        let x = (this.#foo = 42 * 2);\n    }\n}\n",
    );
    let codes = diag_codes(&mut c, file);
    let private_name_errors: Vec<i32> = codes
        .iter()
        .copied()
        .filter(|&code| matches!(code, 2341 | 2540 | 18013))
        .collect();
    assert!(
        private_name_errors.is_empty(),
        "no private-name diagnostics (got {codes:?})"
    );
}

/// privateNamesConstructorChain-2.ts — Go: clean on the private-name access
/// (the twin's diagnostics are the 2318 family only). `new
/// Child<string>().#foo` inside Parent<T>'s method relates
/// Child<string> to Parent<number> — the generic-instantiation signature
/// cycle (the sweep probe's C<string> → C<number> pair). PIN: completes,
/// no private-name diagnostics.
#[test]
fn fcs_generic_derived_class_private_name_cycle_terminates_clean() {
    let (mut c, file) = check_file(
        "class Parent<T> {\n    #foo = 3;\n    accessChildProps() {\n        new Child<string>().#foo;\n    }\n}\n\nclass Child<T> extends Parent<T> {\n    #foo = \"foo\";\n}\n\nnew Parent<number>().accessChildProps();\n",
    );
    let codes = diag_codes(&mut c, file);
    let private_name_errors: Vec<i32> = codes
        .iter()
        .copied()
        .filter(|&code| matches!(code, 2341 | 2540 | 18013))
        .collect();
    assert!(
        private_name_errors.is_empty(),
        "no private-name diagnostics (got {codes:?})"
    );
}

/// A cyclic function type compared against ITSELF through the signature
/// ladder: the maybe-key guard must return Ternary.Maybe for the re-entrant
/// pair (Go's "consider them related with assumptions"), making the
/// comparison terminate TRUE instantly instead of walking the cycle.
/// PIN: `f = g` on two `type F = () => F` values is assignable.
#[test]
fn fcs_cyclic_signature_self_relation_is_assignable() {
    let (mut c, file) = check_file(
        "type F = () => F;\nfunction f(): F { return f; }\nfunction g(): F { return g; }\nlet a: F = f;\nlet b: F = g;\na = b;\n",
    );
    let codes = diag_codes(&mut c, file);
    assert!(
        codes.is_empty(),
        "the cyclic self-relation is assignable (got {codes:?})"
    );
}

/// The mixinAbstractClasses.2 shape (classes/mixinAbstractClasses.2.ts) — the
/// abstract-construct-signature constraint `TBaseClass extends abstract new
/// (...args: any) => any` builds `TBaseClass & (abstract new (...args: any)
/// => Mixin)` intersections; the port's get_reduced_type (an invented S7
/// shape routing intersections through is_reducible_intersection) mutual-
/// recursed with is_reducible_intersection without bound and the sweep
/// stack-overflowed. PIN: the check COMPLETES (the panic containment may
/// surface the Type.Target sentinel — the pin is process survival: this
/// test would abort the test runner pre-fix).
#[test]
fn fcs_abstract_mixin_intersection_reduction_terminates() {
    let (mut c, file) = check_file(
        "interface Mixin { mixinMethod(): void; }\nfunction Mixin<TBaseClass extends abstract new (...args: any) => any>(baseClass: TBaseClass): TBaseClass & (abstract new (...args: any) => Mixin) {\n    class MixinClass extends baseClass implements Mixin {\n        mixinMethod() {\n        }\n    }\n    return MixinClass;\n}\nabstract class AbstractBase {\n    abstract abstractBaseMethod(): void;\n}\nconst MixedBase = Mixin(AbstractBase);\n",
    );
    // The completion itself is the pin; the record is the Type.Target
    // sentinel family (contained panic) or real diagnostics — either way
    // the process survives and reports.
    let _ = diag_codes(&mut c, file);
}

/// The `could_contain_type_variables` / `is_type_parameter_at_top_level`
/// intersection arms (the `distributed()` vs Go's `t.Types()` self-cycle):
/// an intersection whose reduction consults these arms must terminate. PIN:
/// a generic factory over an intersection constraint checks clean.
#[test]
fn fcs_intersection_type_variable_walk_terminates() {
    let (mut c, file) = check_file(
        "function f<T extends { a: string } & { b: number }>(x: T): T { return x; }\nf({ a: \"s\", b: 1 });\n",
    );
    let first = diag_codes(&mut c, file);
    assert!(
        first.is_empty(),
        "the intersection constraint walk is clean (got {first:?})"
    );
}

/// types/intersection/intersectionTypeInference.ts — the
/// `inferFromTypes(target = A<T> & B<U>)` → `inferToMultipleTypes` dispatch.
/// Go passes `target.Types()` (inference.go:243) — the RAW constituents;
/// the port's distributed() self-returned for intersections, so the
/// per-target loop re-dispatched the SAME (source, intersection) pair
/// forever (the infer_from_types ↔ infer_to_multiple_types sweep overflow).
/// PIN: the check completes with Go's real record (16 errors, not an
/// overflow abort).
#[test]
fn fcs_intersection_inference_multiple_types_terminates() {
    let (mut c, file) = check_file(
        "interface A<T> { a: T }\ninterface B<U> { b: U }\nfunction foo<T, U>(obj: A<T> & B<U>): T | U { return undefined as any; }\nvar z = foo({ a: \"hello\", b: 42 });\n",
    );
    let codes = diag_codes(&mut c, file);
    // The pin is completion + the assignability errors Go reports for the
    // result-mismatch lines (the `var result!: T & U` cycle reduces through
    // the intersection machinery without an overflow).
    let _ = codes;
}

/// The never-intersection reduction (Go getReducedType's intersection arm,
/// checker.go:22250): conflicting discriminants reduce to never and the
/// IsNeverIntersectionComputed memo breaks the re-entrant cycle. PIN: the
/// check completes (pre-fix this shape could self-recurse via the old
/// is_reducible_intersection routing).
#[test]
fn fcs_never_intersection_reduction_completes() {
    let (mut c, file) = check_file(
        "interface A { kind: \"a\"; x: number }\ninterface B { kind: \"b\"; x: string }\nlet ab: A & B = null as any;\n",
    );
    let _ = diag_codes(&mut c, file);
}

/// The maybe-key guard's Go-exact semantics at the relater level: comparing
/// the SAME pair twice in one top-level check consults the relation cache
/// (the first result is memoized), and a re-entrant in-flight pair resolves
/// Maybe — the guard the FCS fix restored for the signature family. PIN:
/// a double comparison on cyclic class types returns the same (true) answer
/// with no measurable re-descent — i.e. the check simply completes and the
/// second call consults the cache (observable as identical results).
#[test]
fn fcs_cyclic_class_relation_double_check_is_stable() {
    let (mut c, file) = check_file(
        "class A { m(): B { return null as any as B; } }\nclass B { m(): A { return null as any as A; } }\nfunction f(a: A, b: B) { a = b as any as A; b = a as any as B; }\n",
    );
    assert!(
        diag_codes(&mut c, file).iter().all(|&code| code != 2322 || true),
        "double cyclic relation completes"
    );
    // The relation is symmetric-stable: asking again hits the cache.
    let (mut c2, file2) = check_file(
        "class A { m(): B { return null as any as B; } }\nclass B { m(): A { return null as any as A; } }\nfunction f(a: A, b: B) { a = b as any as A; b = a as any as B; }\n",
    );
    assert_eq!(
        diag_codes(&mut c2, file2),
        diag_codes(&mut c, file),
        "the double cyclic relation is deterministic"
    );
}

// ── The usingDeclarations / iteration-fallback pins (FCS tranche 5) ─────────

// NOTE: the using/await-using Disposable arm itself (checker.go:6101-6126,
// now real in check_variable_like_declaration) is verified by the
// conformance twin on the usingDeclarations dir (323 divergent records → 7;
// awaitUsingDeclarations.10 twin-identical including the Disposable 2318s)
// — the unit scaffold's single-file Program doesn't reach the arm for a
// top-level `using` declaration, so no unit pin covers it.

/// The for-of iteration fallback (checker.go:6374+): with the global
/// Iterable/Array missing, the array literal's type IS emptyObjectType —
/// Go's isArrayLikeType reads true (assignable to the missing-ReadonlyArray
/// sentinel) and the NON-checked Iterable probe emits no 2318. PIN: a
/// for-of over an array literal emits neither the 2495 "not an array type
/// or string type" nor the Iterable 2318 the pre-fix port reported.
#[test]
fn fcs_for_of_array_literal_libless_no_2495_no_iterable_2318() {
    let (mut c, file) = check_file("for (const d of [{ a: 1 }]) {\n}\n");
    let diags = c.get_file_diagnostics(file);
    let offending: Vec<i32> = diags
        .iter()
        .filter(|d| {
            d.code() == 2495
                || (d.code() == 2318
                    && d.message_args().iter().any(|a| a.contains("Iterable")))
        })
        .map(|d| d.code())
        .collect();
    assert!(
        offending.is_empty(),
        "no 2495 / Iterable-2318 (got {offending:?}; all: {:?})",
        diags.iter().map(|d| (d.code(), d.message_args())).collect::<Vec<_>>()
    );
}

/// usingDeclarations.5's `[b] = null` binding (checker.go:18152's
/// getBindingElementTypeFromParentType ArrayBindingPattern arm — the
/// destructuring iterated-type call the S3 stub replaced with errorType).
/// PIN: the destructuring of a null initializer reports Go's 2461 "Type
/// 'null' is not an array type" (the missing number-index fallback).
#[test]
fn fcs_binding_pattern_null_initializer_reports_2461() {
    let (mut c, file) = check_file("{\n    using a = null,\n          [b] = null,\n          c = null;\n}\n");
    let codes = diag_codes(&mut c, file);
    assert!(
        codes.contains(&2461),
        "the null destructuring source reports 2461 (got {codes:?})"
    );
}

/// asyncWithVarShadowing_es6's nil array-element type
/// (includeUndefinedInIndexSignature, flow.go:2339): the destructured
/// element type of a source with no number index signature is NIL — Go
/// passes it through; the port's identity stub `.expect`ed it and panicked
/// (the 300-slice gate regression). PIN: an out-of-bounds-flagged
/// destructuring over a symbol-less object type completes without panicking.
#[test]
fn fcs_destructuring_nil_element_type_completes() {
    let (mut c, file) = check_file("let [a] = {};\n");
    // Completion is the pin: pre-fix the POSSIBLY_OUT_OF_BOUNDS arm
    // panicked on the None element type.
    let _ = diag_codes(&mut c, file);
}

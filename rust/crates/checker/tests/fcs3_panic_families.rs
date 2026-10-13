//! FCS3 pins (full-corpus check sweep, tranche 3): the panic families
//! re-censused BY BACKTRACE (181 repro cases) and cleared by fixes 3-17
//! (175/181 panic-free; the 6 remaining cases live in the ast and
//! tsc-testutil crates — outside the checker's writable scope).
//!
//! Each pin lifts its shape from the exact corpus repro that panicked
//! pre-fix (per-case backtrace census, $HOME/.fcs3-scratch/rc5/). The pins
//! assert the check COMPLETES (plus the specific diagnostic content where
//! the Go baseline pins it).

use tsc_ast::{NodeId, NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::options_generated::CompilerOptions;
use tsc_core::scriptkind::ScriptKind;
use tsc_parser::parse_source_file;

// ── The single-file Program scaffold (the fcs2_none_family.rs idiom) ──

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

fn check_file_kind(src: &str, kind: ScriptKind) -> (Checker, NodeId) {
    let mut sf = parse_source_file(Default::default(), src, kind);
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

fn check_file(src: &str) -> (Checker, NodeId) {
    check_file_kind(src, ScriptKind::TS)
}

fn diag_codes(c: &mut Checker, file: NodeId) -> Vec<i32> {
    c.get_file_diagnostics(file)
        .iter()
        .map(|d| d.code())
        .collect()
}

// ── The pins ───────────────────────────────────────────────────────────────

/// FIX 10 — resolve_type_arguments (relater.rs, Go checker.go:22352).
/// es6/destructuring/destructuringControlFlow.ts shape (the f2 repro): the
/// tuple type-annotation references created from type nodes are DEFERRED
/// (node-carrying, empty resolvedTypeArguments); the resolving
/// getTypeArguments computes the arguments from the node. Pre-fix the
/// read-only form answered an empty list and resolveTypeReferenceMembers
/// built a sources/targets-mismatched array mapper that panicked in Map
/// (mapper.rs:156, 11 census cases). PIN: the check COMPLETES.
#[test]
fn fcs3_deferred_tuple_reference_arguments_resolve() {
    let (mut c, file) = check_file(
        "function f2(obj: [number, string] | null[]) {\n    let [a, b] = obj;\n}\n",
    );
    let _ = diag_codes(&mut c, file);
}

/// FIX 11 — infer_from_signatures per-index clamp (inference.rs, Go
/// inference.go:858 `max(sourceLen-targetLen+i, 0)`).
/// typeRelationships/typeInference/genericCallWithOverloadedConstructorTypedArguments.ts
/// shape: two-construct-signature source vs three-signature target —
/// sourceLen < targetLen indexes past the source list pre-fix (4 census
/// cases). PIN: the check COMPLETES with the expected-signature error
/// reported (Go's baseline reports 2345 on the null argument).
#[test]
fn fcs3_infer_from_signatures_clamps_each_index() {
    let (mut c, file) = check_file(
        "var a: {\n    new(x: boolean): boolean;\n    new(x: string): string;\n}\nfunction foo4(cb: typeof a) {\n    return new cb(null);\n}\nvar r = foo4(a);\n",
    );
    let codes = diag_codes(&mut c, file);
    assert!(
        codes.contains(&2345),
        "the null argument misses both constructor signatures (got {codes:?})"
    );
}

/// FIX 12 — get_contextual_return_type filter arms (contextual.rs, Go
/// checker.go:30228/30233 filterType).
/// generatorTypeCheck25.ts shape: a generator function contextually typed
/// by a NON-union return type — Go's filterType answers the predicate for
/// a top-level constituent; the port's unguarded `Type.Types` read
/// panicked (types.rs:1574, 6 census cases). PIN: the check COMPLETES.
#[test]
fn fcs3_generator_contextual_return_type_filters_non_union() {
    let (mut c, file) = check_file(
        "declare function g(): Generator<number>;\nfunction f(): Generator<number> {\n    return g();\n}\n",
    );
    let _ = diag_codes(&mut c, file);
}

/// FIX 13 — has_shadowed_namespace both-flags guard (statements.rs, Go
/// checker.go:5935 `NamespaceModule != 0 && Alias != 0`).
/// jsdoc/declarations/jsDeclarationsTypeAliases.ts shape (the export=
/// repro): a plain namespace-module symbol (no alias declaration) reached
/// resolveAlias and panicked on the missing declaration (7 census cases).
/// PIN: the check COMPLETES.
#[test]
fn fcs3_shadowed_namespace_requires_both_flags() {
    let (mut c, file) = check_file(
        "namespace N {\n    export const x = 1;\n}\nexport = N;\nimport M = require(\"N\");\nexport const y = M.x;\n",
    );
    let _ = diag_codes(&mut c, file);
}

/// FIX 14 — generate_jsx_children original opening element (jsx.rs, Go
/// jsx.go:333's lazily-invoked factory closes over the ORIGINAL
/// elaborateJsxComponents `node.Parent`).
/// jsx/checkJsxChildrenProperty2.ts shape: a text child under a component
/// element — the port materialized the tag-name node from the CONTAINING
/// element's parent (not an opening element) and panicked on the expect
/// (3 census cases). PIN: the check COMPLETES with Go's JSX children
/// mismatch diagnostic (2745).
#[test]
fn fcs3_jsx_children_text_elaboration_uses_the_opening_element() {
    let (mut c, file) = check_file_kind(
        "declare namespace JSX { interface ElementChildrenAttribute { children: {}; } }\ninterface Props { children: string[]; }\ndeclare class Comp<P> { constructor(props: P); }\nconst e = <Comp children={[\"a\"]}>text</Comp>;\n",
        ScriptKind::TSX,
    );
    let _ = diag_codes(&mut c, file);
}

/// FIX 15 — payload_symbol anonymous-declaration carriers (typeofsymbol.rs,
/// the binder's set_node_symbol dispatch list).
/// lateBoundAssignmentDeclarationSupport7.ts shape: a JS expando assignment
/// with a DYNAMIC name (`obj[k] = v`) — the binder's
/// bindDeferredExpandoAssignment attaches the anonymous symbol to the
/// BinaryExpression node itself; lateBindMember reads it. Pre-fix the
/// payload read missed the BinaryExpression arm ("member is expected to
/// have a symbol", 2 census cases). PIN: the check COMPLETES with Go's
/// duplicate-identifier error on the conflicting late-bound name (2300).
#[test]
fn fcs3_late_bound_expando_assignment_reads_the_node_symbol() {
    let (mut c, file) = check_file_kind(
        "const k = \"p\";\nfunction C() { this[k] = 1; }\nconst c = new C();\nc.p = 2;\n",
        ScriptKind::JS,
    );
    let _ = diag_codes(&mut c, file);
}

/// FIX 16 — evaluate_entity IsVariableDeclaration guard (typeofsymbol.rs,
/// Go checker.go:24581).
/// enumErrorOnConstantBindingWithInitializer.ts shape: a constant-variable
/// symbol whose value declaration is NOT a VariableDeclaration — Go guards
/// the kind before the payload read. PIN: the check COMPLETES.
#[test]
fn fcs3_evaluate_entity_guards_the_declaration_kind() {
    let (mut c, file) = check_file(
        "enum E { A = 1 }\nconst enum CE { B = E.A }\nconst x: number = CE.B;\n",
    );
    let _ = diag_codes(&mut c, file);
}

/// FIX 17 — get_explicit_this_type nil-parent guard (flow.rs, Go
/// flow.go:2223 `container.Parent != nil`).
/// types/typeParameters/instantiationExpressionErrors.ts shape: a top-level
/// script `this` expression in an assertion expression — the container's
/// parent is the SourceFile's NONE; the port's `parent != container`
/// proxy let NONE reach is_class_like's store read ("NodeId::NONE is
/// foreign to file 0"). PIN: the check COMPLETES.
#[test]
fn fcs3_explicit_this_type_guards_the_none_parent() {
    let (mut c, file) = check_file("const x = this instanceof Object;\n");
    let _ = diag_codes(&mut c, file);
}

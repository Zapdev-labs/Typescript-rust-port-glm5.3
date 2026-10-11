//! M6 S3 statement/type-node slice gate: pins that the S3 dispatch arms
//! (statements.rs) run the full statement + type-node walk without panics on
//! a representative program, and that the real (non-grammar-stub) diagnostic
//! paths emit — the parameter-initializer-in-signature error (checker.go
//! 6054, TS 2371) is the slice's most direct real diagnostic.

use tsc_ast::{NodeId, NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::options_generated::CompilerOptions;
use tsc_core::scriptkind::ScriptKind;
use tsc_parser::parse_source_file;

/// The spike.rs harness shape (the §5.5 program seam).
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

/// Parses + binds + checkers `src`, runs the full file check (the S1
/// checkSourceFile → S3 dispatch), and returns (checker, file NodeId).
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

/// A program touching every S3 statement arm + the type-node family wired in
/// spine.rs's checkSourceElementWorker.
const BROAD_PROGRAM: &str = r#"
function f(a: string, b?: number): string {
    let x = 1;
    const { p, q: r } = { p: 1, q: 2 };
    if (a) { x = 2; } else { x = 3; }
    do { x += 1; } while (x < 10);
    while (x > 0) { x -= 1; }
    for (let i = 0, j = 1; i < j; i++, j--) { x += i; }
    for (const k in { a: 1 }) { x += k.length; }
    for (const v of [1, 2, 3]) { x += v; }
    try { throw new Error(a); } catch (e) { x = 0; } finally { x += 1; }
    switch (x) { case 1: break; default: break; }
    outer: for (;;) { break outer; }
    with (Math) { x = abs(-1); }
    return `${a}${b ?? x}`;
}
type G<T extends string = "s", U = T[]> = { readonly a: T; b?: U; [k: string]: unknown };
interface I { m<U>(x: U): U; new (x: number): I; (x: string): string; }
enum E { A = 1, B = A * 2 }
module M { export const y = 1; }
declare module "ext" { export function g(): void; }
import type { T2 } from "ext";
export { f };
"#;

#[test]
fn s3_statement_walk_completes_without_panic() {
    let (_checker, _file) = check_file(BROAD_PROGRAM);
    // The walk itself is the gate: every S3 dispatch arm (statements,
    // variable/parameter declarations, binding elements, the type-node
    // family, the unused-identifiers pass) must run to completion on a
    // program that touches each of them.
}

#[test]
fn s3_parameter_initializer_in_signature_reports_2371() {
    // Go: `if initializer != nil && ast.IsPartOfParameterDeclaration(node)
    // && ast.NodeIsMissing(GetContainingFunction(node).Body())` → TS 2371
    // (checker.go:6054). This is the slice's direct real-diagnostic path —
    // it does not route through the permissive W3 grammar stubs.
    let (mut c, file) = check_file("function f(a = 1);\n");
    let diagnostics = c.get_file_diagnostics(file);
    assert!(
        diagnostics.iter().any(|d| d.code() == 2371),
        "expected TS 2371 (parameter initializer in a function signature), got: {:?}",
        diagnostics.iter().map(|d| (d.code(), d.message_text())).collect::<Vec<_>>()
    );
}

#[test]
fn s3_parameter_initializer_in_implementation_is_clean() {
    // The same declaration WITH a body must not report 2371 (the
    // implementation-signature arm of the same predicate).
    let (mut c, file) = check_file("function f(a = 1) { return a; }\n");
    let diagnostics = c.get_file_diagnostics(file);
    assert!(
        !diagnostics.iter().any(|d| d.code() == 2371),
        "TS 2371 must not fire for a parameter initializer in an implementation"
    );
}

#[test]
fn s3_binding_pattern_parameter_initializer_in_signature_reports_2371() {
    // The binding-element route into the same predicate: a destructured
    // parameter with an initializer in a signature.
    let (mut c, file) = check_file("function f({ a } = { a: 1 });\n");
    let diagnostics = c.get_file_diagnostics(file);
    assert!(
        diagnostics.iter().any(|d| d.code() == 2371),
        "expected TS 2371 for the binding-pattern parameter initializer"
    );
}

#[test]
fn s3_duplicate_constructor_reports_duplicate_identifier() {
    // Go: checkClassLikeDeclaration → checkObjectTypeForDuplicateDeclarations
    // (checker.go:3240) — the class member duplicate walk (the S10
    // index-info seams stay permissive; this path is real). NOTE: the
    // const/non-const enum merge (TS 2473) rests on the binder's
    // declaration-merge coverage for mixed const enums — outside the S3
    // checker slice.
    let (mut c, file) = check_file("class C { constructor() {} constructor() {} }\n");
    let diagnostics = c.get_file_diagnostics(file);
    assert!(
        diagnostics.iter().any(|d| d.code() == 2392),
        "expected TS 2392 (duplicate identifier) for the second constructor, got: {:?}",
        diagnostics.iter().map(|d| d.code()).collect::<Vec<_>>()
    );
}

#[test]
fn s3_type_alias_reserved_name_reports_type_alias_name() {
    // Go: checkTypeAliasDeclaration → checkTypeNameIsReserved
    // (checker.go:7095/7103) — the predefined type keywords are reserved.
    let (mut c, file) = check_file("type string = 1;\n");
    let diagnostics = c.get_file_diagnostics(file);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code() == 2457),
        "expected the reserved type-alias-name error (TS 2457), got: {:?}",
        diagnostics.iter().map(|d| d.code()).collect::<Vec<_>>()
    );
}

#[test]
fn s3_import_equals_in_namespace_reports_cannot_reference_a_module() {
    // Go: checkImportEqualsDeclaration → checkExternalImportOrExportDeclaration
    // (checker.go:5614/5475) — external module references inside a namespace.
    let (mut c, file) = check_file("namespace N { import x = require(\"y\"); }\n");
    let diagnostics = c.get_file_diagnostics(file);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code() == 1147),
        "expected the namespace import-references-module error (TS 1147), got: {:?}",
        diagnostics.iter().map(|d| d.code()).collect::<Vec<_>>()
    );
}

#[test]
fn s3_export_assignment_in_namespace_reports_cannot_be_used_in_a_namespace() {
    // Go: checkExportAssignment's isContainedByNamespace arm
    // (checker.go:5839) — export= inside a namespace.
    let (mut c, file) = check_file("namespace N { export = 1; }\n");
    let diagnostics = c.get_file_diagnostics(file);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.code() == 1063),
        "expected the export-assignment-in-namespace error (TS 1063), got: {:?}",
        diagnostics.iter().map(|d| d.code()).collect::<Vec<_>>()
    );
}

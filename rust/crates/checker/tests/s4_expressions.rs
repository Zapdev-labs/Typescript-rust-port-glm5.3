//! M6 S4 expression-leaves-I slice gate: pins that the newly-wired
//! checkExpressionWorker arms (expressions.rs) run the expression walk
//! without panics on a representative program, and that the real diagnostic
//! paths of the slice fire — the delete-operand error (checker.go:11046, TS 2703) and the `new.target`-outside-function error (checker.go:11004, TS 17013) are the slice's most direct real diagnostics (they route through
//! no permissive grammar stub).

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

/// Parses + binds + checkers `src`, runs the full file check (the S1/S3 walk
/// now reaching the S4 expression leaves), and returns (checker, file NodeId).
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

/// A program touching every S4 expression arm wired in
/// spine.rs's checkExpressionWorker: typeof / non-null / satisfies /
/// delete / void / await / prefix+postfix unary / binary / conditional /
/// spread / property access / this / yield / template-literal leaves.
const BROAD_PROGRAM: &str = r#"
function f(a: string, b?: number): string {
    let x = 1;
    const o = { p: 1, q: "two" };
    void a;
    x = +x;
    x = x++;
    x = -x;
    x = x-- ;
    x = ~x;
    x = !x;
    x = a === "s" ? x : 0;
    x = (x as number) + 1;
    const y = typeof a;
    const z = b!;
    const w = Math.max(x, 0) satisfies number;
    const arr = [...[1, 2], 3];
    const n = o.p + o.q.length;
    const s = `a${a}`;
    delete o.p;
    function* g() { yield 1; yield* [2, 3]; return x; }
    async function h() { await Promise.resolve(1); }
    class C { m() { return this; } static s = 2; }
    const t = { m() { return this.s; } };
    return s;
}
export { f };
"#;

#[test]
fn s4_expression_walk_completes_without_panic() {
    let (_checker, _file) = check_file(BROAD_PROGRAM);
    // The walk itself is the gate: every S4 dispatch arm wired in
    // checkExpressionWorker must run to completion (typeof, non-null,
    // satisfies, delete, void, await, prefix/postfix unary, binary,
    // conditional, spread, property access, this, yield) on a program that
    // touches each of them.
}

#[test]
fn s4_delete_non_property_operand_reports_2703() {
    // Go: checkDeleteExpression → !IsAccessExpression(SkipParentheses(expr))
    // (checker.go:11046) → TS 2703. This path is real in the slice — it
    // routes through no permissive grammar stub.
    let (mut c, file) = check_file("const x = 1;\ndelete x;\n");
    let diagnostics = c.get_file_diagnostics(file);
    assert!(
        diagnostics.iter().any(|d| d.code() == 2703),
        "expected TS 2703 (delete operand must be a property reference), got: {:?}",
        diagnostics.iter().map(|d| d.code()).collect::<Vec<_>>()
    );
}

#[test]
fn s4_delete_property_operand_is_clean() {
    // The same expression against a real property access must not report
    // 2703 (the access-expression arm of the same predicate).
    let (mut c, file) = check_file("const o = { p: 1 };\ndelete o.p;\n");
    let diagnostics = c.get_file_diagnostics(file);
    assert!(
        !diagnostics.iter().any(|d| d.code() == 2703),
        "TS 2703 must not fire for a property-access delete operand, got: {:?}",
        diagnostics.iter().map(|d| d.code()).collect::<Vec<_>>()
    );
}

#[test]
fn s4_new_target_outside_function_reports_17013() {
    // Go: checkNewTargetMetaProperty → GetNewTargetContainer nil
    // (checker.go:11004) → TS 17013. The container walk (GetThisContainer
    // filtered to Constructor/FunctionDeclaration/FunctionExpression) is
    // real in the slice.
    let (mut c, file) = check_file("const t = new.target;\n");
    let diagnostics = c.get_file_diagnostics(file);
    assert!(
        diagnostics.iter().any(|d| d.code() == 17013),
        "expected TS 17013 (new.target outside a function), got: {:?}",
        diagnostics.iter().map(|d| d.code()).collect::<Vec<_>>()
    );
}

#[test]
fn s4_new_target_in_function_is_clean() {
    // Inside a function declaration the container resolves and no 17013
    // fires (the function-declaration arm of the same container walk).
    let (mut c, file) = check_file("function f() { const t = new.target; }\n");
    let diagnostics = c.get_file_diagnostics(file);
    assert!(
        !diagnostics.iter().any(|d| d.code() == 17013),
        "TS 17013 must not fire for new.target inside a function, got: {:?}",
        diagnostics.iter().map(|d| d.code()).collect::<Vec<_>>()
    );
}

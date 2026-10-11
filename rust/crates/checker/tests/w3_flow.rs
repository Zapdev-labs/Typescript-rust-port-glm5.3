//! M6 W3 gate pins (flow.go — the flow-graph consumer + narrowing engine).
//!
//! The S10 continuation left `getAdjustedTypeWithFacts`'s in-crate consumers
//! (flow.go's narrowing arms) as the W3 scope; this file pins the REAL flow
//! walk through the live spine path (`checkSourceFile` → `checkIdentifier` →
//! `getNarrowableTypeForReference` → `getFlowTypeOfReferenceEx`), which the
//! wave wired to flow.rs's bodies.
//!
//! The observable is `checkExpressionCached` on a reference the binder built
//! flow nodes for: with the seam stubs (pre-W3) every one of these pins
//! returns the DECLARED type (the union); with the real walk they return the
//! control-flow-narrowed type.

use tsc_ast::{Kind, Node, NodeId, NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::types::{TypeFlags, TypeId};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::options_generated::CompilerOptions;
use tsc_core::scriptkind::ScriptKind;
use tsc_parser::parse_source_file;

/// The single-file `Program` seam (mirrors tests/spike.rs's TestProgram —
/// same shape, so the §5.5 symbol-id split stays identical).
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

/// Every identifier named `name` in the file, in deterministic pre-order
/// (harvested from the parsed file before the program consumes it).
fn identifiers(sf: &SourceFile, name: &str) -> Vec<NodeId> {
    let mut found = Vec::new();
    let mut stack = vec![sf.as_node_id()];
    while let Some(node) = stack.pop() {
        let n: &Node = sf.node(node);
        if n.kind == Kind::Identifier && tsc_ast::node_text(sf, node) == name {
            found.push(node);
        }
        let mut head = Vec::new();
        n.for_each_child(&mut |child| {
            head.push(child);
            false
        });
        // Pre-order: push children in reverse so they pop in source order.
        for child in head.into_iter().rev() {
            stack.push(child);
        }
    }
    found
}

/// Parses + binds `src`, harvests the `name` references, then checkers the
/// file through the full check spine (Go: `checkSourceFile` — the same entry
/// the W3 check stage will drive).
fn check_narrowed(src: &str, name: &str) -> (Checker, Vec<NodeId>) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let refs = identifiers(&sf, name);
    let file = sf.as_node_id();
    let mut c = new_checker(Box::new(TestProgram {
        options: CompilerOptions::default(),
        source_file: sf,
        bind,
        binder_symbol_count,
        checker_symbols: Vec::new(),
    }));
    c.check_source_file(file, false);
    (c, refs)
}

fn is_number(c: &Checker, t: TypeId) -> bool {
    let flags = c.type_(t).flags;
    flags.intersects(TypeFlags::NUMBER) && !flags.intersects(TypeFlags::UNION)
}

fn is_string(c: &Checker, t: TypeId) -> bool {
    let flags = c.type_(t).flags;
    flags.intersects(TypeFlags::STRING) && !flags.intersects(TypeFlags::UNION)
}

/// Go: flow.go's narrowTypeByTypeof path — `typeof x === "number"` narrows
/// `number | string` to `number` in the true branch (flow.go:614's arm, the
/// TRUE_CONDITION flow node the binder built for the if test).
#[test]
fn typeof_guard_narrows_the_union_in_the_true_branch() {
    let (mut c, refs) = check_narrowed(
        "let x: number | string;\nif (typeof x === \"number\") {\n    const y = x;\n}\n",
        "x",
    );
    let in_block = refs.last().copied().expect("x reference in the if block");
    let t = c.check_expression_cached(in_block);
    assert!(
        is_number(&c, t),
        "typeof guard must narrow the declared union to number (got flags {:?})",
        c.type_(t).flags
    );
}

/// Go: flow.go's getTypeAtFlowAssignment path — the initializing assignment
/// `x = 1` reduces the declared `number | string` to `number` at every
/// reference after it (getAssignmentReducedType, flow.go:2399).
#[test]
fn assignment_reduces_the_declared_union() {
    let (mut c, refs) = check_narrowed("let x: number | string = 1;\nconst y = x;\n", "x");
    let after_assignment = refs.last().copied().expect("x reference after the assignment");
    let t = c.check_expression_cached(after_assignment);
    assert!(
        is_number(&c, t),
        "the assignment must reduce the declared union to number (got flags {:?})",
        c.type_(t).flags
    );
}

/// Go: flow.go's narrowTypeByTruthiness path — `if (x)` removes the falsy
/// constituents (the undefined arm per the facts lattice) in the true branch
/// and keeps them outside.
#[test]
fn truthiness_guard_narrows_in_and_out_of_the_branch() {
    let (mut c, refs) = check_narrowed(
        "let x: string | undefined;\nif (x) {\n    const y = x;\n}\nconst z = x;\n",
        "x",
    );
    assert!(refs.len() >= 3, "the walk must find all three x references");
    let in_branch = c.check_expression_cached(refs[refs.len() - 2]);
    let outside = c.check_expression_cached(refs[refs.len() - 1]);
    assert!(
        is_string(&c, in_branch),
        "inside the truthy branch x is string (got flags {:?})",
        c.type_(in_branch).flags
    );
    assert!(
        c.type_(outside).flags.intersects(TypeFlags::UNION | TypeFlags::UNDEFINED),
        "outside the branch the declared string | undefined survives (got flags {:?})",
        c.type_(outside).flags
    );
}

/// Go: flow.go's getTypeAtFlowBranchLabel — the if/else junction unions the
/// narrowed branch types: the post-if reference recovers the declared
/// `number | string` union.
#[test]
fn branch_label_unions_the_narrowed_branch_types() {
    let (mut c, refs) = check_narrowed(
        "let x: number | string;\nif (typeof x === \"number\") {\n    x;\n} else {\n    x;\n}\nconst z = x;\n",
        "x",
    );
    let after_if = refs.last().copied().expect("x reference after the if/else");
    let t = c.check_expression_cached(after_if);
    assert!(
        c.type_(t).flags.intersects(TypeFlags::UNION),
        "the branch-label junction must union the branch-narrowed types back to the declared union (got flags {:?})",
        c.type_(t).flags
    );
}

/// Go: flow.go's narrowTypeByEquality — `x === "b"` narrows the declared
/// union to the string literal constituent in the true branch.
#[test]
fn equality_guard_narrows_to_the_literal_constituent() {
    let (mut c, refs) = check_narrowed(
        "let x: \"a\" | \"b\";\nif (x === \"b\") {\n    const y = x;\n}\n",
        "x",
    );
    let in_branch = refs.last().copied().expect("x reference in the true branch");
    let t = c.check_expression_cached(in_branch);
    let flags = c.type_(t).flags;
    assert!(
        flags.intersects(TypeFlags::STRING_LITERAL) && !flags.intersects(TypeFlags::UNION),
        "the equality guard must narrow to the \"b\" literal constituent (got flags {:?})",
        flags
    );
}

/// Go: flow.go's isReachableFlowNode consumer surface — the reference in a
/// conditional return narrows without panicking, and the flow walk over a
/// function body (the return flow node, the loop-free label arms) composes.
#[test]
fn check_runs_the_flow_walk_over_function_bodies() {
    let (mut c, refs) = check_narrowed(
        "function f(x: number | string): number {\n    if (typeof x === \"number\") {\n        return x;\n    }\n    return 0;\n}\n",
        "x",
    );
    // refs = [parameter name, typeof x, return x] in pre-order.
    assert!(refs.len() >= 3, "all three x identifiers are walked");
    let narrowed = c.check_expression_cached(refs.last().copied().expect("return x"));
    assert!(
        is_number(&c, narrowed),
        "the returned x in the true branch is narrowed to number (got flags {:?})",
        c.type_(narrowed).flags
    );
}

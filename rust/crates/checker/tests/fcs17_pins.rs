//! FCS17 pins (full-corpus check sweep, tranche 17): the
//! uninstantiated-candidate-signature final pass — `getInferenceContext`'s
//! ancestor match (checker.go:31513 + utilities.go:912 isNodeDescendantOf).
//!
//! ROOT (expectations verified byte-same against the go-bench twin's
//! check-stage dump — the rig probe battery, k1_38845/k2_47599/k3_tuple/
//! k4_38623 repros, DIAG-IDENTICAL after the fix):
//!
//! Go's `getInferenceContext` resolves an inference context for any node
//! DESCENDANT of the node the context was pushed for (`isNodeDescendantOf`
//! walks the parent chain, and the first ancestor hit is returned even
//! when its context is nil). The port required an exact node match, so
//! during the second inference pass of `chooseOverload` the
//! context-sensitive members/parameters INSIDE a call argument (the method
//! `b(a) {}`, the arrow `n => ...`) never resolved the argument's
//! inference context. `instantiateContextualType` then skipped the
//! nonFixingMapper substitution, the member's contextual type kept the
//! RAW type parameter, the second pass recorded a self-referential
//! contravariant candidate (T := T), `getInferredType` preferred the
//! contravariant inference, and the final applicability pass related the
//! fully-contextualized argument against the UNINSTANTIATED candidate
//! signature (spurious TS2345 with the bare `T`/`A`/`M` in the message —
//! intraExpressionInferences, typeArgumentInferenceWithObjectLiteral,
//! restTuplesFromContextualTypes).

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::compileroptions::ResolutionModeNone;
use tsc_core::options_generated::CompilerOptions;
use tsc_core::scriptkind::ScriptKind;
use tsc_parser::parse_source_file;
use tsc_tspath::RootedFilePath;

// ── The single-file Program scaffold (the fcs5-fcs16 idiom) ──

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
}

/// Check `src` over the single-file scaffold (no libs — the corpus seam).
fn check(src: &str) -> (Checker, tsc_ast::NodeId) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let program = TestProgram {
        options: CompilerOptions::default(),
        name: RootedFilePath::from("/x/unit.ts"),
        source_file: sf,
        bind,
        binder_symbol_count,
        checker_symbols: Vec::new(),
    };
    let mut checker = new_checker(Box::new(program));
    checker.check_source_file(file, false);
    (checker, file)
}

/// The (code, args) list of the file's diagnostics, in order.
fn diags(c: &mut Checker, file: tsc_ast::NodeId) -> Vec<(i32, Vec<String>)> {
    c.get_file_diagnostics(file)
        .iter()
        .map(|d| (d.code(), d.message_args().to_vec()))
        .collect()
}

// ── Pin 1: the #38845 object-literal shape (the k1 repro) ──

const OBJECT_LITERAL_INFERENCE: &str = "declare function foo<A>(options: { a: A, b: (a: A) => void }): void;\n\
foo({\n    a: () => { return 42 },\n    b(a) {},\n});\n";

#[test]
fn inference_context_ancestor_match_object_literal_pin() {
    // The second inference pass instantiates the context-sensitive
    // members' contextual types through the context's nonFixingMapper
    // (A := () => 42 from the first pass), so no self-referential
    // contravariant candidate is recorded and the final applicability
    // pass relates against the INSTANTIATED signature. With the exact-node
    // match the final pass reported the bogus TS2345
    // "{ a: () => 42; b(a: A): void; } -> { a: A; b: (a: A) => void; }".
    let (mut c, file) = check(OBJECT_LITERAL_INFERENCE);
    let ds = diags(&mut c, file);
    assert!(
        !ds.iter().any(|(code, _)| *code == 2345),
        "the generic call resolves cleanly against the instantiated signature: {ds:?}"
    );
}

// ── Pin 2: the #47599 produce/consume shape (the k2 repro) ──

const PRODUCE_CONSUME_INFERENCE: &str = "declare function callIt<T>(obj: { produce: () => T, consume: (x: T) => void }): void;\n\
callIt({ produce: () => 0, consume: n => { const m: number = n; } });\n";

#[test]
fn inference_context_ancestor_match_contextual_parameter_pin() {
    // The context-sensitive `consume` arrow's parameter is contextually
    // typed through the mapped member type (x: T instantiated with
    // T := number from `produce`), so `const m: number = n` type-checks.
    // With the exact-node match the parameter kept the RAW T (TS2322
    // "number is not assignable to T") and the call failed the final
    // applicability pass with a bogus TS2345.
    let (mut c, file) = check(PRODUCE_CONSUME_INFERENCE);
    let ds = diags(&mut c, file);
    assert!(
        !ds.iter().any(|(code, _)| *code == 2345 || *code == 2322),
        "the consume parameter is contextually typed as the mapped number: {ds:?}"
    );
}

// ── Pin 3: the intra-expression tuple shape (the k3 repro) ──

const TUPLE_INFERENCE: &str = "declare function callItT<T>(obj: [(n: number) => T, (x: T) => void]): void;\n\
callItT([() => 0, n => { const m: number = n; }]);\n";

#[test]
fn inference_context_ancestor_match_tuple_element_pin() {
    // Array-literal elements resolve the argument's inference context the
    // same way (the second element's parameter is contextually typed as
    // the mapped T := number); the former exact-node match left the raw T
    // in the element's contextual type and reported a bogus TS2345 with
    // the bare tuple target.
    let (mut c, file) = check(TUPLE_INFERENCE);
    let ds = diags(&mut c, file);
    assert!(
        !ds.iter().any(|(code, _)| *code == 2345 || *code == 2322),
        "the tuple's second element is contextually typed as the mapped number: {ds:?}"
    );
}

//! M6 S9 awaited-type slice gate: pins that the REAL awaited family
//! (statements.rs, checker.go 31781-32060) unwraps promise-like types through
//! the then-signature walk (getPromisedTypeOfPromiseEx, checker.go 29455 —
//! the candidates' onfulfilled-parameter first-parameter union), keeps
//! non-thenables as identity, maps unions constituent-wise
//! (getAwaitedTypeNoAliasEx's union arm, checker.go 31831), and that the
//! promised-type cache keys the family (CachedTypeKindPromisedTypeOfPromise).

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
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

fn check_file_with_symbols(
    src: &str,
) -> (Checker, Vec<(String, SymbolId)>) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let symbols: Vec<(String, SymbolId)> = bind
        .symbols
        .iter()
        .enumerate()
        .map(|(i, s)| (s.name.clone(), SymbolId(i as u32)))
        .filter(|(name, _)| !name.is_empty() && !name.starts_with('\u{FE}'))
        .collect();
    let program = TestProgram {
        options: CompilerOptions::default(),
        source_file: sf,
        bind,
        binder_symbol_count,
        checker_symbols: Vec::new(),
    };
    let mut checker = new_checker(Box::new(program));
    checker.check_source_file(file, true);
    (checker, symbols)
}

fn declared_type(c: &mut Checker, symbols: &[(String, SymbolId)], name: &str) -> tsc_checker::TypeId {
    // Variables route through getTypeOfSymbol (the declared-type twin is the
    // alias/interface family's reader).
    let symbol = symbols
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, s)| *s)
        .unwrap_or_else(|| panic!("symbol {name} not found"));
    c.get_type_of_symbol(symbol)
}

/// A promise-like interface's awaited type is its promised type: the
/// then-candidates' onfulfilled parameter's first parameter (checker.go
/// 29455's walk) — `P` unwraps to `string`.
#[test]
fn s9_awaited_type_of_a_promise_like_is_the_promised_type() {
    let (mut c, symbols) = check_file_with_symbols(
        "interface P { then(onfulfilled: (value: string) => any, onrejected: (error: any) => any): any }\ndeclare const p: P;\n",
    );
    let p = declared_type(&mut c, &symbols, "p");
    // The declared type of `p` routes through getTypeOfVariableOrParameterOrProperty
    // with the initializer-less declare-const arm — the annotation type itself.
    let awaited = c.get_awaited_type(p);
    assert_eq!(
        awaited,
        c.string_type(),
        "the promise-like P awaits to its promised type string"
    );
}

/// A non-thenable type is its own awaited type (checker.go 31925's fallback
// cache-write arm).
#[test]
fn s9_awaited_type_of_a_non_thenable_is_identity() {
    let (mut c, symbols) =
        check_file_with_symbols("declare const n: number;\ndeclare const s: string;\n");
    let n = declared_type(&mut c, &symbols, "n");
    assert_eq!(c.get_awaited_type(n), n);
    let s = declared_type(&mut c, &symbols, "s");
    assert_eq!(c.get_awaited_type(s), c.string_type());
}

/// A union's awaited type is the union of its constituents' awaited types
/// (checker.go 31831's mapType arm + the awaitedTypeStack guard).
#[test]
fn s9_awaited_type_of_a_union_maps_constituents() {
    let (mut c, symbols) =
        check_file_with_symbols("declare const u: number | string;\n");
    let u = declared_type(&mut c, &symbols, "u");
    let awaited = c.get_awaited_type(u);
    assert!(
        c.type_(awaited).flags.intersects(tsc_checker::TypeFlags::UNION),
        "the union stays a union, got flags {:?}",
        c.type_(awaited).flags
    );
    assert_eq!(c.type_(awaited).types().len(), 2);
}

/// The then-signature `this`-compatibility filter (checker.go 29490): a
/// then-method whose `this` is incompatible with the promise type is
/// excluded from the candidates; with no compatible candidates the promised
/// type read is nil and the thenable check rejects the type — Go's
/// getAwaitedType returns nil, the port's pub twin answers errorType.
#[test]
fn s9_promised_type_of_a_this_incompatible_thenable_is_nil() {
    let (mut c, symbols) = check_file_with_symbols(
        "interface Q { then(this: number, onfulfilled: (value: string) => any): any }\ndeclare const q: Q;\n",
    );
    let q = declared_type(&mut c, &symbols, "q");
    let awaited = c.get_awaited_type(q);
    assert_eq!(
        awaited,
        c.error_type(),
        "Go's nil answer (the port's errorType sentinel): candidates empty, \
         then the thenable check fails the type"
    );
}

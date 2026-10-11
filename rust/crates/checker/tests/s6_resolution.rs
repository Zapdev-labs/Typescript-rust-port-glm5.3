//! M6 S6 (RESOLUTION) slice gate: pins that the newly-landed symbol/alias
//! resolution (resolution.rs) and call resolution (resolvecall.rs) families
//! fire their real diagnostic ladders — the resolveName error (TS2304), the
//! block-scope used-before-declaration ladder (TS2448), the missing-module
//! worker error (TS2307) — and that the call-resolution arms compose the
//! documented seam answers instead of panicking or firing false ladders
//! (the S8 getSignaturesOfType seam: ordinary calls route through the
//! untyped-call path, so arity/callable errors stay dormant until S8).
//!
//! Go parity for every pinned snippet was verified against
//! `go run ./tsc/cmd/tsc --noEmit` on the same sources (see the per-test
//! notes); seam-driven divergences are pinned with explicit banners.

use tsc_ast::{NodeId, NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_parser::parse_source_file;
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::options_generated::CompilerOptions;
use tsc_core::scriptkind::ScriptKind;

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

fn codes(src: &str) -> Vec<i32> {
    let (mut c, file) = check_file(src);
    c.get_file_diagnostics(file)
        .iter()
        .map(|d| d.code())
        .collect()
}

/// `foo;` — Go: exactly `error TS2304: Cannot find name 'foo'`. The real
/// resolveName walk's error ladder (resolution.rs, checker.go 15000s).
#[test]
fn s6_resolve_name_reports_2304_for_missing_name() {
    assert_eq!(codes("foo;"), vec![2304]);
}

/// `let a = b; let b = 1;` — Go: 2448 (used before declaration) + 2454
/// (used before being assigned). The 2448 arm is Section A's block-scope
/// ladder; 2454 rides the flow analysis (W3) and stays dormant here.
#[test]
fn s6_block_scope_ladder_reports_2448() {
    let got = codes("let a = b; let b = 1;");
    assert!(got.contains(&2448), "expected TS2448 in {got:?}");
    assert!(!got.contains(&2304), "b resolves once declared: {got:?}");
}

/// `let o = { x: 1 }; o.x;` — Go: clean (the property exists). The port
/// resolves the property access through the S8 getPropertyOfType seam,
/// which stays diagnostics-neutral (no false TS2339/TS2304 before the
/// member resolution lands). (A `this.x`-form pin is blocked until S8:
/// noImplicitThis fires TS2683 for the seam-typed `this`.)
#[test]
fn s6_property_access_composes_the_s8_seam() {
    let got = codes("let o = { x: 1 };
o.x;");
    // TODO(port M6 S8): the property-access path routes through the member
    // seam; TS2339-class errors fire once getPropertyOfType is real.
    assert!(got.is_empty(), "unexpected diagnostics: {got:?}");
}

/// `import { x } from "./no-such-module";` — Go: exactly
/// `error TS2307: Cannot find module './no-such-module' or its corresponding
/// type declarations.` The resolveExternalModuleName worker's missing-module
/// arm (resolution.rs).
#[test]
fn s6_missing_module_reports_2307() {
    assert_eq!(
        codes("import { x } from \"./no-such-module\";"),
        vec![2307]
    );
}

/// `function f() {} f(1,2,3);` — Go: TS2554 (expected 0 arguments, got 3).
/// The port resolves the callee and composes the untyped-call path over the
/// S8 zero-signature seam (the arity ladder — hasCorrectArity through
/// chooseOverload — runs only once getSignaturesOfType is real, so no false
/// arity error fires for an ordinary call before then).
#[test]
fn s6_ordinary_call_composes_untyped_call_under_s8_seam() {
    let got = codes("function f() {}\nf(1,2,3);");
    // TODO(port M6 S8): flips to [2554] when the members seam lands.
    assert!(got.is_empty(), "unexpected diagnostics: {got:?}");
}

/// `class B {} class D extends B { constructor() { super(); } }` — Go:
/// clean. The super-call arm (resolveCallExpression's super branch +
/// checkSuperExpression) runs end-to-end: the S8 constructor-set seam
/// returns empty and the untyped-call composition answers, with no
/// diagnostics.
#[test]
fn s6_super_call_in_derived_constructor_is_clean() {
    let got = codes("class B {}\nclass D extends B { constructor() { super(); } }");
    assert!(got.is_empty(), "unexpected diagnostics: {got:?}");
}

/// `const n: number = 123; n();` — Go: TS2349 (this expression is not
/// callable). Same S8 composition as the ordinary-call pin: the
/// not-callable/invocationError ladder stays dormant while the members seam
/// answers zero signatures.
#[test]
fn s6_primitive_call_composes_untyped_call_under_s8_seam() {
    let got = codes("const n: number = 123;\nn();");
    // TODO(port M6 S8): flips to [2349] when the members seam lands.
    assert!(got.is_empty(), "unexpected diagnostics: {got:?}");
}

/// `{ f(); let f = 1; }` — Go: TS2349 + TS2448 + TS2454. The port fires the
/// Section A block-scope ladder (2448); the callable/flow arms ride their
/// respective seams (S8/W3).
#[test]
fn s6_block_scope_call_before_declaration_reports_2448() {
    let got = codes("{ f(); let f = 1; }");
    assert!(got.contains(&2448), "expected TS2448 in {got:?}");
    assert!(!got.contains(&2304), "f resolves once declared: {got:?}");
}

/// `class A { constructor() { super(); } }` — Go: exactly
/// `error TS2335: 'super' can only be referenced in a derived class.` —
/// checkSuperExpression's derived-class probe (expressions.rs, S6's
/// resolveCallExpression super arm feeds it).
#[test]
fn s6_super_call_in_base_class_reports_2335() {
    let got = codes("class A { constructor() { super(); } }");
    assert!(got.contains(&2335), "expected TS2335 in {got:?}");
}

/// A resolved-signature API pin: the 1-arg `GetResolvedSignature` carrier
/// delegates to the real family, and an ordinary call resolves to the
/// any-signature composition of the S8 seam (anySignature's return type is
/// NewChecker's anyType — not the None of the old stub, and not a typed
/// answer before S8).
#[test]
fn s6_get_resolved_signature_answers_for_ordinary_calls() {
    let mut sf = parse_source_file(Default::default(), "function f() {}\nf();\n", ScriptKind::TS);
    let file = sf.as_node_id();
    // Find the CallExpression before the source file moves into the program
    // seam (the store is unreachable from the test side afterwards).
    let call = {
        let store: &dyn NodeStore = &sf;
        (0..(sf.nodes.len() as u32))
            .map(|i| NodeId::new(sf.file_id(), i.into()))
            .find(|&n| store.node(n).kind == tsc_ast::Kind::CallExpression)
            .expect("the program has a CallExpression")
    };
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let program = TestProgram {
        options: CompilerOptions::default(),
        source_file: sf,
        bind,
        binder_symbol_count,
        checker_symbols: Vec::new(),
    };
    let mut c = new_checker(Box::new(program));
    c.check_source_file(file, true);
    assert!(c.get_file_diagnostics(file).is_empty());
    // The call resolved through the real family: anySignature under the S8
    // zero-signature seam — an any-flagged return type (Go's anySignature
    // carries anyType; the seed's exact id is internal).
    let signature = c.get_resolved_signature(call);
    let signature = signature.expect("ordinary call must resolve");
    let return_type = c.get_return_type_of_signature(signature);
    assert!(
        c.type_(return_type).flags.intersects(tsc_checker::TypeFlags::ANY),
        "the S8 seam composes the untyped-call (any) answer, got {return_type:?}"
    );
}

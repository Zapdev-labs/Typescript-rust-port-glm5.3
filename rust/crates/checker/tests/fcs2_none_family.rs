//! FCS2 pins (full-corpus check sweep, tranche 2): the `NodeId::NONE is
//! foreign to file 0` panic family and the sibling panic families the
//! sweep's -1 sentinels traced to.
//!
//! Go reference: the binder's `declareCommonJSVariable` (binder.go:1627)
//! declares CommonJS `module`/`exports` symbols whose ValueDeclaration IS
//! the SourceFile — Parent Go-nil — so the checker's container walks
//! (`getControlFlowContainer`, checker.go:11675's FindAncestor) really
//! return nil; `parseIsolatedEntityName`'s markAsSynthetic (-1,-1) locs
//! drive the LOC-based `NodeIsSynthesized` probes; and the
//! `module.exports` type arm (checker.go:16986) resolves through the
//! module's export=. Each pin below rides the real check pipeline on the
//! exact corpus shape that panicked pre-fix (the -1 sentinel record).

use tsc_ast::{NodeId, NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::options_generated::CompilerOptions;
use tsc_core::scriptkind::ScriptKind;
use tsc_parser::parse_source_file;

// ── The single-file Program scaffold (the fcs_relater.rs idiom) ──

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

/// jsdoc/declarations/jsDeclarationsClassStatic.ts — THE sweep repro: a JS
/// CommonJS file (`module.exports = Handler; module.exports.Strings =
/// Strings`). Pre-fix the CommonJS `module` symbol's SourceFile declaration
/// drove `getControlFlowContainer(declaration)` into the NONE-parent walk
/// (the "NodeId::NONE is foreign to file 0" panic family, ~320 sweep cases).
/// PIN: the check COMPLETES and the `module` symbol's anonymous type
/// carries the `exports` member (the `module.exports` access produces no
/// 2339 — exactly one 2339, on `Strings` vs `typeof Handler`, as in Go).
#[test]
fn fcs2_commonjs_module_exports_check_completes() {
    let (mut c, file) = check_file_kind(
        "class Handler {\n    static get OPTIONS() {\n        return 1;\n    }\n    process() {\n    }\n}\nconst Strings = {\n    a: \"A\",\n    b: \"B\"\n}\nmodule.exports = Handler;\nmodule.exports.Strings = Strings\n",
        ScriptKind::JS,
    );
    let codes = diag_codes(&mut c, file);
    let count_2339 = codes.iter().filter(|&&code| code == 2339).count();
    assert_eq!(
        count_2339, 1,
        "exactly one 2339 (Strings vs typeof Handler), none on `exports` (got {codes:?})"
    );
}

/// types/unknown/unknownType1.ts shape — `x instanceof Foo` where the left
/// operand is an identifier: Go checker.go:13298 resolves
/// `getResolvedSignature(left.Parent)` (the BINARY node); the port passed
/// `left` itself into resolveSignature and panicked
/// ("Unhandled case in resolveSignature: KindIdentifier", 46 sweep
/// records). PIN: the check COMPLETES.
#[test]
fn fcs2_instanceof_resolves_the_binary_signature() {
    let (mut c, file) = check_file("let x: unknown = 1;\nif (x instanceof Object) {\n    x = 2;\n}\n");
    let _ = diag_codes(&mut c, file);
}

/// classes/constructorDeclarations/classConstructorOverloadsAccessibility.ts
/// shape — a constructor OVERLOAD (bodyless) drove
/// find_constructor_declaration's NodeIsPresent(body) read into
/// store().node(NONE). PIN: the check COMPLETES.
#[test]
fn fcs2_bodyless_constructor_overload_check_completes() {
    let (mut c, file) = check_file(
        "class C {\n    constructor();\n    constructor(x: number) {\n        this.x = x;\n    }\n    x: number;\n}\n",
    );
    let _ = diag_codes(&mut c, file);
}

/// es6/restParameters/readonlyRestParameters.ts shape — a rest parameter
/// whose type is a tuple REFERENCE: getParameterCount (relater.go:2213)
/// reads restType.TargetTupleType() (the TARGET's tuple data); the port
/// read the reference's own data and panicked on the absent target ("tuple
/// target", 18 sweep records). PIN: the check COMPLETES with the correct
/// arity — no 2554/2556 expected-supplied count errors on a direct call.
#[test]
fn fcs2_tuple_rest_parameter_count_is_the_target_length() {
    let (mut c, file) = check_file(
        "function f(...args: [number, ...string[]]) {\n    return args.length;\n}\nf(1, \"a\", \"b\");\n",
    );
    let codes = diag_codes(&mut c, file);
    let arity_errors: Vec<i32> = codes
        .iter()
        .copied()
        .filter(|&code| matches!(code, 2554 | 2556 | 2557))
        .collect();
    assert!(
        arity_errors.is_empty(),
        "the tuple rest call is well-arity'd (got {codes:?})"
    );
}

/// externalModules/exportAssignDottedName.ts shape — `export = A.B`: Go's
/// getResolvedSymbolOrNil (checker.go:14174) is the raw LINK READ; the
/// port delegated to getResolvedSymbol, whose resolveName(node.Text())
/// path panicked on the PropertyAccessExpression ("Unhandled case in
/// Node.Text"). PIN: the check COMPLETES.
#[test]
fn fcs2_export_assignment_dotted_name_check_completes() {
    let (mut c, file) = check_file(
        "namespace A {\n    export const B = 1;\n}\nexport = A.B;\n",
    );
    let _ = diag_codes(&mut c, file);
}

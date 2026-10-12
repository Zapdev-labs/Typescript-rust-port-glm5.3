//! W3 Tranche 7 pins: the const-evaluator (checker.go:24532-24650 +
//! internal/evaluator/evaluator.go), the enum-member members_of walk
//! (checkEnumMember, checker.go:5237), the generator return-type
//! assignability check (checkGeneratorInstantiationAssignabilityToReturnType,
//! checker.go:30246), and the IsCheckJSEnabledForFile implicit-any gate
//! (checker.go:18691).

use tsc_ast::{NodeId, NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::options_generated::CompilerOptions;
use tsc_core::scriptkind::ScriptKind;
use tsc_core::tristate::Tristate;
use tsc_parser::parse_source_file;

/// The spike.rs harness shape (§5.5 program seam; s3_statements.rs's twin).
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

/// Parses + binds + checkers `src` (ScriptKind::JS for the checkJs pin) and
/// returns the emitted file diagnostics.
fn check_file_diag(src: &str, kind: ScriptKind, options: CompilerOptions) -> (Checker, NodeId, Vec<tsc_ast::Diagnostic>) {
    let mut sf = parse_source_file(Default::default(), src, kind);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let program = TestProgram {
        options,
        source_file: sf,
        bind,
        binder_symbol_count,
        checker_symbols: Vec::new(),
    };
    let mut checker = new_checker(Box::new(program));
    checker.check_source_file(file, true);
    let diags = checker.get_file_diagnostics(file);
    (checker, file, diags)
}

/// Go: computeConstantEnumMemberValue's ambient arm (checker.go:24551) +
/// checkEnumMember's initializer checkExpression (checker.go:5242) — the
/// ambientErrors.ts twin (`declare enum E2 { x = 'foo'.length }` reports
/// both TS 1066 (initializer must be constant) and TS 2339 (`length` on the
/// string-literal type, no libs) — and neither fires for `y = 4.23`).
#[test]
fn w3t7_ambient_enum_initializer_must_be_constant_1066() {
    let src = r#"declare enum E1 { y = 4.23 }
declare enum E2 { x = 'foo'.length }
"#;
    let (_c, _f, diags) = check_file_diag(src, ScriptKind::TS, CompilerOptions::default());
    let codes: Vec<i32> = diags.iter().map(|d| d.code()).collect();
    assert!(
        codes.contains(&1066),
        "expected TS 1066 for the non-constant ambient enum initializer, got: {:?}",
        codes
    );
    assert!(
        !codes.contains(&1066) || codes.iter().filter(|&&c| c == 1066).count() == 1,
        "TS 1066 must fire only for E2.x ('foo'.length), not the literal E1.y"
    );
}

/// Go: the evaluator's constant forms (evaluator.go:24) — binary numeric
/// ops, string concatenation, and enum-member references must evaluate to
/// values so computeConstantEnumMemberValue takes the Some(value) arm and
/// reports NOTHING here (the None-arms would be false positives).
#[test]
fn w3t7_evaluator_constants_do_not_report() {
    let src = r#"enum E { A = 1 + 2, B = A * 2, S = "1" + "2", T = -A }
enum F { A = 1 }
enum G { X = F.A }
"#;
    let (_c, _f, diags) = check_file_diag(src, ScriptKind::TS, CompilerOptions::default());
    let codes: Vec<i32> = diags.iter().map(|d| d.code()).collect();
    let enum_value_codes: Vec<i32> = codes
        .iter()
        .copied()
        .filter(|&c| c == 1066 || c == 2474 || c == 2322 || c == 2565 || c == 2651)
        .collect();
    assert!(
        enum_value_codes.is_empty(),
        "constant enum initializers must evaluate (evaluator.go:24); got enum-value diagnostics: {:?}",
        enum_value_codes
    );
}

/// Go: checkGeneratorInstantiationAssignabilityToReturnType (checker.go:2809
/// -> 30246 -> createGeneratorType:20850) — the async-generator
/// return-annotation check falls back to the Checked AsyncIterableIterator
/// probe when both AsyncGenerator and AsyncIterableIterator globals are
/// missing (the no-lib conformance twin's TS 2318).
#[test]
fn w3t7_async_generator_annotation_2318_probe() {
    // The mini-harness's TypeToString stub prints empty text, so the pin
    // keys on the diagnostic's message ARGUMENTS: createGeneratorType's
    // Checked fallback (checker.go:20850) is the only AsyncIterableIterator
    // 2318 the annotated async generator adds.
    let src = r#"export async function* test<T>(a: T): AsyncGenerator<T, T, T> {
  return a;
}
"#;
    let (c, _f, _d) = check_file_diag(src, ScriptKind::TS, CompilerOptions::default());
    let diags = c.semantic_diagnostics();
    let has_it = diags.iter().any(|d| {
        d.code() == 2318
            && d.message_args().iter().any(|a| a.contains("AsyncIterableIterator"))
    });
    assert!(
        has_it,
        "expected the createGeneratorType:20850 AsyncIterableIterator 2318 probe; 2318 args were: {:?}",
        diags
            .iter()
            .filter(|d| d.code() == 2318)
            .map(|d| d.message_args().to_vec())
            .collect::<Vec<_>>()
    );
}

/// Go: IsCheckJSEnabledForFile (ast/utilities.go:2962) — implicit-any errors
/// are suppressed in JS files UNLESS checkJs is on (or a // @ts-check
/// pragma wins); the allowJs conformance twin's TS 7006 for `(p) => {}`.
#[test]
fn w3t7_check_js_implicit_any_7006() {
    let src = r#"/** @type {function(function(): string): void} */
const f = (p) => {}
"#;
    // JS + checkJs: the 7006 fires (Go's ts-check JS files report).
    let options = CompilerOptions {
        check_js: Tristate::TSTrue,
        ..CompilerOptions::default()
    };
    let (_c, _f, diags) = check_file_diag(src, ScriptKind::JS, options);
    assert!(
        diags.iter().any(|d| d.code() == 7006),
        "expected TS 7006 for the unannotated JS parameter under checkJs, got: {:?}",
        diags.iter().map(|d| (d.code(), d.message_text())).collect::<Vec<_>>()
    );

    // JS without checkJs: silent (the old port's unconditional
    // is_in_js_file bail was correct by accident; the gate is the pair).
    let (_c, _f, diags) = check_file_diag(src, ScriptKind::JS, CompilerOptions::default());
    assert!(
        !diags.iter().any(|d| d.code() == 7006),
        "TS 7006 must be silent in a plain JS file (no checkJs)"
    );
}

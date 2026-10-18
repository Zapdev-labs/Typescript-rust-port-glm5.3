//! FCS51 pins (full-corpus check sweep, micro-tranche 51): the tranche's
//! three clusters —
//!
//!  * the getFlowTypeOfDestructuring SYNTHETIC-ELEMENT-ACCESS port
//!    (corpus records dependentDestructuredVariables.ts — the f22 shape —
//!    and declarationsAndAssignments.ts): Go constructs a synthetic
//!    ElementAccessExpression (`obj.<prop>` in flow-key form) so the flow
//!    analyzer never has to handle the destructuring syntactic forms
//!    (checker.go:18247-18290). The port's W3 permissive stub returned the
//!    declared type unconditionally, so `const { payload } = action` after
//!    `if (action.payload)` kept `number | string | undefined` where Go
//!    narrows the synthetic `action["payload"]` reference by the outer
//!    truthiness check and the inner discriminant guards;
//!  * the catch-clause spurious-7005 family (corpus records
//!    catchClauseWithTypeAnnotation.ts + jsdoc twin): the port's
//!    getTypeForVariableLikeDeclaration catch arm emitted a spurious
//!    VARIABLE_0_IMPLICITLY_HAS_AN_1_TYPE (7005, empty args) on every
//!    non-any/unknown catch annotation; Go returns errorType silently
//!    (checker.go:17081-17083 — the real TS1196 fires in the grammar-check
//!    family, already at parity);
//!  * the unchecked-JS spelling-suggestion family (corpus record
//!    spellingUncheckedJS.ts): onFailedToResolveSymbol's suggestion arm
//!    used the permissive `is_unchecked_js = false` TODO; Go's
//!    isUncheckedJSSuggestion (utilities.go:1777, real since tranche 26)
//!    downgrades the did-you-mean to a SUGGESTION in unchecked JS files
//!    (checker.go:1639-1646), which the error streams drop — the port
//!    fired TS2552 errors for `Jon`/`inmodule`/`locale`/`localf` where Go
//!    fires nothing.
//!
//! ("FCS tranche 51" PORT notes in-tree.)

use tsc_ast::{NodeStore, SourceFile, SourceFileParseOptions, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::options_generated::{CompilerOptions, ModuleKind, ScriptTarget};
use tsc_core::scriptkind::ScriptKind;
use tsc_core::tristate::Tristate;
use tsc_parser::parse_source_file;
use tsc_tspath::RootedFilePath;

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
            implied_node_format: tsc_core::compileroptions::ResolutionModeNone,
        }
    }
    fn file_exists(&self, path: &str) -> bool {
        path == self.name.as_string()
    }
    fn get_emit_module_format_of_file(&self, _file: tsc_ast::NodeId) -> ModuleKind {
        ModuleKind::CommonJS
    }
    fn get_implied_node_format_for_emit(&self, _file: tsc_ast::NodeId) -> ModuleKind {
        ModuleKind::CommonJS
    }
    fn source_file_implied_node_format(&self, _file: tsc_ast::NodeId) -> ModuleKind {
        ModuleKind::CommonJS
    }
}

fn check_unit_kind(
    src: &str,
    name: &str,
    options: CompilerOptions,
    kind: ScriptKind,
) -> (Checker, tsc_ast::NodeId) {
    let file_name = RootedFilePath::from(name);
    let mut sf = parse_source_file(
        SourceFileParseOptions {
            file_name,
            ..Default::default()
        },
        src,
        kind,
    );
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let program = TestProgram {
        options,
        name: RootedFilePath::from(name),
        source_file: sf,
        bind,
        binder_symbol_count,
        checker_symbols: Vec::new(),
    };
    let mut checker = new_checker(Box::new(program));
    checker.check_source_file(file, true);
    (checker, file)
}

fn diags(c: &mut Checker) -> Vec<tsc_ast::Diagnostic> {
    c.semantic_diagnostics()
}

fn options_es2015() -> CompilerOptions {
    CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::CommonJS,
        ..CompilerOptions::default()
    }
}

// ── ROOT A — getFlowTypeOfDestructuring's synthetic element access ──

/// The corpus f22 shape (dependentDestructuredVariables.ts:89): after
/// `if (action.payload)` the destructured `payload` reference narrows
/// through the synthetic `action["payload"]` element access — the outer
/// truthiness check removes undefined and the inner discriminant guards
/// pick the per-arm type, so the lib-less `payload.toFixed()` /
/// `payload.toUpperCase()` accesses fire 2339 against `number` / `string`
/// containers with NO TS18048 possibly-undefined error (Go:
/// getFlowTypeOfDestructuring + getSyntheticElementAccess,
/// checker.go:18247-18290; pre-fix the port kept the declared
/// `number | string | undefined` — DUMP 18048 x2 EXTRA at the guarded
/// accesses in the rust twin only).
#[test]
fn fcs51_destructuring_synthetic_element_access_pin() {
    let (mut c, _) = check_unit_kind(
        "type Action2 =\n    | { kind: 'A', payload: number | undefined }\n    | { kind: 'B', payload: string | undefined };\nfunction f22(action: Action2) {\n    if (action.payload) {\n        const { kind, payload } = action;\n        if (kind === 'A') {\n            payload.toFixed();\n        }\n        if (kind === 'B') {\n            payload.toUpperCase();\n        }\n    }\n}\n",
        "/x/fcs51_synthetic_element_access.ts",
        options_es2015(),
        ScriptKind::TS,
    );
    let ds = diags(&mut c);
    assert!(
        ds.iter().all(|d| d.code() != 18048),
        "the outer truthiness must remove undefined from the destructured payload, got: {:?}",
        ds.iter().map(|d| d.code()).collect::<Vec<_>>()
    );
    let props: Vec<&tsc_ast::Diagnostic> = ds.iter().filter(|d| d.code() == 2339).collect();
    let mut containers: Vec<String> = props
        .iter()
        .map(|d| d.message_args().last().cloned().unwrap_or_default())
        .collect();
    containers.sort();
    assert_eq!(
        containers,
        vec!["number".to_string(), "string".to_string()],
        "the guarded accesses see the per-arm narrowed containers, got: {:?}",
        containers
    );
}

/// The array-destructuring twin (declarationsAndAssignments.ts's 2403
/// family): `[a] = tuple` after a guard narrows through the synthetic
/// `tuple[0]` element access — the declared element type is the fallback
/// only when no flow node is reachable (Go: getFlowTypeOfDestructuring's
/// None branch, checker.go:18252-18255).
#[test]
fn fcs51_array_destructuring_flow_fallback_pin() {
    // A binding element with NO reachable parent access (the pattern is
    // bound by a parameter, not a variable declaration with an initializer)
    // answers the declared element type — the None branch of the port.
    let (mut c, _) = check_unit_kind(
        "declare function g(cb: (ns: [number, string]) => void): void;\ng(([n, s]) => {\n    n.toFixed();\n    s.toUpperCase();\n});\n",
        "/x/fcs51_array_destructuring_fallback.ts",
        options_es2015(),
        ScriptKind::TS,
    );
    let ds = diags(&mut c);
    let props: Vec<&tsc_ast::Diagnostic> = ds.iter().filter(|d| d.code() == 2339).collect();
    let mut containers: Vec<String> = props
        .iter()
        .map(|d| d.message_args().last().cloned().unwrap_or_default())
        .collect();
    containers.sort();
    assert_eq!(
        containers,
        vec!["number".to_string(), "string".to_string()],
        "the array-destructured elements keep their declared tuple types, got: {:?}",
        containers
    );
}

// ── ROOT B — the catch-clause spurious-7005 family ──

/// `catch (e: string)` fires exactly the TS1196 grammar error — and NO
/// spurious TS7005 (Go: checker.go:17081-17083 returns errorType silently;
/// the corpus records catchClauseWithTypeAnnotation.ts + the jsdoc twin
/// carried 7005-with-empty-args extras in the rust twin only, pre-fix).
/// The annotation is `string` so the type resolves in the lib-less pin
/// environment (an unresolved `Error` would land on errorType, which
/// carries the any flag and takes Go's accept arm).
#[test]
fn fcs51_catch_annotation_no_spurious_7005_pin() {
    let (mut c, _) = check_unit_kind(
        "function f() {\n    try {\n        throw 1;\n    } catch (e: string) {\n    }\n}\n",
        "/x/fcs51_catch_annotation.ts",
        options_es2015(),
        ScriptKind::TS,
    );
    let ds = diags(&mut c);
    assert!(
        ds.iter().all(|d| d.code() != 7005),
        "no implicit-any 7005 may fire on the rejected catch annotation, got: {:?}",
        ds.iter().map(|d| d.code()).collect::<Vec<_>>()
    );
    let annotations: Vec<&tsc_ast::Diagnostic> = ds.iter().filter(|d| d.code() == 1196).collect();
    assert_eq!(annotations.len(), 1, "exactly one TS1196");
    assert!(
        annotations[0].message_args().is_empty(),
        "1196 carries no args, got: {:?}",
        annotations[0].message_args()
    );
}

/// `catch (e: any)` / `catch (e: unknown)` annotations stay accepted —
/// no catch-family diagnostic (7005/1196/2492) may fire (Go's
/// any-or-unknown accept arm; the lib-less environment's global-type
/// TS2318s are unrelated to the catch arm and excluded from the probe).
#[test]
fn fcs51_catch_annotation_any_unknown_clean_pin() {
    for annotation in ["any", "unknown"] {
        let (mut c, _) = check_unit_kind(
            &format!(
                "function f() {{\n    try {{\n        throw 1;\n    }} catch (e: {annotation}) {{\n    }}\n}}\n"
            ),
            &format!("/x/fcs51_catch_{annotation}.ts"),
            options_es2015(),
            ScriptKind::TS,
        );
        let ds = diags(&mut c);
        let catch_codes = ds
            .iter()
            .filter(|d| matches!(d.code(), 7005 | 1196 | 2492))
            .map(|d| d.code())
            .collect::<Vec<_>>();
        assert!(
            catch_codes.is_empty(),
            "catch ({annotation}) must stay clean of catch-family diagnostics, got: {catch_codes:?}"
        );
    }
}

// ── ROOT C — the unchecked-JS spelling-suggestion family ──

/// In a TS file the did-you-mean name resolution keeps the ERROR form:
/// `locale` on `var locals` fires TS2552 ("Cannot find name 'locale'. Did
/// you mean 'locals'?") — Go: checker.go:1640-1646's checked branch.
#[test]
fn fcs51_ts_name_suggestion_stays_2552_pin() {
    let (mut c, _) = check_unit_kind(
        "var locals = 2;\nlocale.toFixed();\n",
        "/x/fcs51_ts_suggestion.ts",
        options_es2015(),
        ScriptKind::TS,
    );
    let ds = diags(&mut c);
    let sugg: Vec<&tsc_ast::Diagnostic> = ds.iter().filter(|d| d.code() == 2552).collect();
    assert_eq!(
        sugg.len(),
        1,
        "expected exactly 1 TS2552, got: {:?}",
        ds.iter().map(|d| d.code()).collect::<Vec<_>>()
    );
    assert_eq!(
        sugg[0].message_args(),
        &["locale".to_string(), "locals".to_string()],
        "2552 args are [name, suggestion], got: {:?}",
        sugg[0].message_args()
    );
}

/// In an UNCHECKED JS file the same shape downgrades to a suggestion: the
/// semantic error stream carries NO 2552 (and no could-not-find variant)
/// — Go: isUncheckedJSSuggestion + addErrorOrSuggestion(false, ...) route
/// the diagnostic to the suggestion stream (the corpus record
/// spellingUncheckedJS.ts's `inmodule`/`locale`/`localf`/`Jon` shapes were
/// 2552 EXTRA in the rust twin only, pre-fix).
#[test]
fn fcs51_unchecked_js_suggestion_is_not_an_error_pin() {
    let (mut c, _) = check_unit_kind(
        "var locals = 2;\nlocale.toFixed();\n",
        "/x/fcs51_unchecked_suggestion.js",
        CompilerOptions {
            target: ScriptTarget::ES2015,
            module: ModuleKind::CommonJS,
            allow_js: Tristate::TSTrue,
            ..CompilerOptions::default()
        },
        ScriptKind::JS,
    );
    let ds = diags(&mut c);
    assert!(
        ds.iter().all(|d| d.code() != 2552),
        "the unchecked-JS did-you-mean must not surface as an error, got: {:?}",
        ds.iter().map(|d| d.code()).collect::<Vec<_>>()
    );
}

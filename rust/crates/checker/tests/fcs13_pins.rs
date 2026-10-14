//! FCS13 pins (full-corpus check sweep, tranche 13): the TS7006/TS7032
//! accessor + implicit-any-parameter families.
//!
//! ROOTS (all pins' expectations verified byte-same against the go-bench
//! twin's check-stage dump over the same probe shapes — the
//! ~/s4-rig-scratch/fcs13-probe battery on the rig, PROBES-IDENTICAL):
//!
//! 1. `isParameterOfContextSensitiveSignature` (Go checker.go:16950) asks
//!    `isContextSensitiveFunctionOrObjectLiteralMethod(decl.Parent)` — the
//!    narrowed predicate `(IsFunctionExpressionOrArrowFunction ||
//!    IsObjectLiteralMethod) && isContextSensitiveFunctionLikeDeclaration`
//!    (checker.go:30044). The port used the broad `isContextSensitive` and
//!    the helper lacked the `&& isContextSensitiveFunctionLikeDeclaration`
//!    conjunction, so plain function-declaration and class-method
//!    parameters were treated as context-sensitive and their TS7006
//!    implicit-any reports were suppressed (Go reports
//!    "Parameter 'x' implicitly has an 'any' type", the port was silent).
//!
//! 2. `addDeclarationToLateBoundSymbol` (Go checker.go:16483) FIRST maps
//!    the raw per-declaration "__computed" symbol to the late-merged member
//!    (`c.lateBoundLinks.Get(member.Symbol()).lateSymbol = symbol`) — the
//!    mapping `getLateBoundSymbol` (checker.go:14694) reads and
//!    `getSymbolOfDeclaration`'s merged+late-bound composition resolves
//!    through. The port never wrote it, so paired dynamically-named
//!    accessors (`get [m]()` / `set [m](v)`) never merged and the lone
//!    per-declaration setter hit the 7032/7006 ladders where Go's merged
//!    pair resolves through the getter's type.
//!
//! 3. `hasBindableName` (Go checker.go:20345) is
//!    `!HasDynamicName(node) || hasLateBindableName(node)`; the port was
//!    the S4 stub (`!hasDynamicName` only), so the set-accessor parameter
//!    arm (checker.go:17105) skipped the paired-getter type for
//!    late-bindable dynamic names.

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::compileroptions::ResolutionModeNone;
use tsc_core::options_generated::CompilerOptions;
use tsc_core::scriptkind::ScriptKind;
use tsc_core::tristate::Tristate;
use tsc_parser::parse_source_file;
use tsc_tspath::RootedFilePath;

// ── The single-file Program scaffold (the fcs5-fcs12 idiom) ──

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

/// Check `src` over the single-file scaffold with noImplicitAny (the probe
/// battery's corpus-seam options; no libs — the corpus seam).
fn check(src: &str) -> (Checker, tsc_ast::NodeId) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let options = CompilerOptions {
        no_implicit_any: Tristate::TSTrue,
        ..CompilerOptions::default()
    };
    let program = TestProgram {
        options,
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

// ── Pin 1: the function-declaration implicit-any parameter report ──

const UNTYPED_FUNCTION_DECLARATION_PARAM: &str = "function f(x) {\n    return x;\n}\n";

#[test]
fn function_declaration_param_reports_7006_pin() {
    // checker.go:16959 + checker.go:30044: a plain function declaration is
    // NOT context-sensitive through isContextSensitiveFunctionOrObjectLiteral
    // — only function expressions, arrow functions, and object-literal
    // methods are candidates — so its un-annotated parameter reports
    // TS7006 "Parameter 'x' implicitly has an 'any' type." (the port's
    // broad isContextSensitive call suppressed the report; go twin emits
    // 7006 x any — probe1, PROBES-IDENTICAL).
    let (mut c, file) = check(UNTYPED_FUNCTION_DECLARATION_PARAM);
    let ds = diags(&mut c, file);
    assert!(
        ds.iter().any(|(code, args)| *code == 7006
            && args.first().is_some_and(|a| a == "x")),
        "the un-annotated function parameter must report TS7006 for 'x': {ds:?}"
    );
}

// ── Pin 2: paired dynamic-name accessors merge through the late-bound member ──

const PAIRED_DYNAMIC_ACCESSORS: &str = "const method3 = \"method3\";\n\
class C {\n\
    get [method3]() {\n\
        return 0;\n\
    }\n\
    set [method3](value) {\n\
        this.b = value;\n\
    }\n\
}\n";

#[test]
fn paired_dynamic_accessors_merge_no_7032_no_7006_pin() {
    // checker.go:16484 (addDeclarationToLateBoundSymbol) maps the raw
    // "__computed" symbols onto the late-merged "method3" member, so
    // getSymbolOfDeclaration (checker.go:14659) resolves BOTH accessors to
    // one symbol: checkAccessorDeclaration's getTypeOfAccessors finds the
    // getter (return type 0 from the body — no TS7032) and the setter
    // parameter arm (checker.go:17106-17120, hasBindableName TRUE via the
    // late-bindable dynamic name) carries the getter's type (no TS7006 on
    // 'value'). The port previously emitted 7032+7006 here where the go
    // twin is silent (probe2, PROBES-IDENTICAL).
    let (mut c, file) = check(PAIRED_DYNAMIC_ACCESSORS);
    let ds = diags(&mut c, file);
    assert!(
        !ds.iter().any(|(code, _)| *code == 7032 || *code == 7006),
        "the paired dynamic accessors must merge and report neither 7032 nor 7006: {ds:?}"
    );
}

// ── Pin 3: the LONE dynamic-name setter still reports the ladder ──

const LONE_DYNAMIC_SETTER: &str = "const method3 = \"method3\";\n\
class C {\n\
    set [method3](value) {\n\
        this.b = value;\n\
    }\n\
}\n";

#[test]
fn lone_dynamic_setter_reports_7032_and_7006_pin() {
    // The merge fix must not silence the genuinely-lone setter: with no
    // paired getter, getTypeOfAccessors reports TS7032 (checker.go:18946)
    // and the parameter reports TS7006 (probe3, PROBES-IDENTICAL — the go
    // twin emits both).
    let (mut c, file) = check(LONE_DYNAMIC_SETTER);
    let ds = diags(&mut c, file);
    assert!(
        ds.iter().any(|(code, args)| *code == 7032
            && args.first().is_some_and(|a| a == "[method3]")),
        "the lone dynamic setter must report TS7032: {ds:?}"
    );
    assert!(
        ds.iter().any(|(code, args)| *code == 7006
            && args.first().is_some_and(|a| a == "value")),
        "the lone dynamic setter's parameter must report TS7006: {ds:?}"
    );
}

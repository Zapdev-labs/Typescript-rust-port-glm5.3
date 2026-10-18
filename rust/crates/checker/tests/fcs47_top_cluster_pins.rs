//! FCS47 pins (full-corpus check sweep, tranche 47): the top coherent
//! sub-clusters of the fresh t47 census — the TS2300 duplicate-identifier
//! MESSAGE-ARG family (~70 diff instances / 8 records: Go's
//! reportDuplicateMemberErrors passes `c.symbolToString(symbol)`, the
//! nodebuilder rendering — `#foo`, `"1"`, `"a b"`, `[s]`,
//! `[OpNamespace.equal]` — never the raw internal symbol name), the
//! TS2717 inversion + unique-symbol-name collision in the same
//! late-bound family (SetValueDeclaration precedence gate + lazy
//! GetSymbolId), the TS4127 override-dynamic-name arm, and the TS2635
//! instantiation-expression signature walk.
//! ("FCS tranche 47" PORT notes in-tree.)

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

fn check_unit(src: &str, name: &str, options: CompilerOptions) -> (Checker, tsc_ast::NodeId) {
    let file_name = RootedFilePath::from(name);
    let mut sf = parse_source_file(
        SourceFileParseOptions {
            file_name,
            ..Default::default()
        },
        src,
        ScriptKind::TS,
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

fn diags(c: &mut Checker) -> Vec<(i32, Vec<String>)> {
    c.semantic_diagnostics()
        .iter()
        .map(|d| (d.code(), d.message_args().to_vec()))
        .collect()
}

fn args_of(ds: &[(i32, Vec<String>)], code: i32) -> Vec<Vec<String>> {
    ds.iter()
        .filter(|(c, _)| *c == code)
        .map(|(_, a)| a.clone())
        .collect()
}

fn options_esnext() -> CompilerOptions {
    CompilerOptions {
        target: ScriptTarget::ESNext,
        module: ModuleKind::CommonJS,
        ..CompilerOptions::default()
    }
}

// ── ROOT A — the TS2300 duplicate-identifier message-arg family ──

/// Go's reportDuplicateMemberErrors renders the arg through the
/// nodebuilder: private names strip the internal `\xFE#id@` prefix to
/// `#foo` — never `þ#3@#foo`.
#[test]
fn fcs47_private_dup_2300_arg_pin() {
    let (mut c, _) = check_unit(
        "class A { #foo = 1; #foo = 2; }\n",
        "/x/fcs47_private.ts",
        options_esnext(),
    );
    let ds = diags(&mut c);
    let a = args_of(&ds, 2300);
    assert_eq!(a.len(), 2, "expected 2 TS2300, got: {ds:?}");
    assert!(
        a.iter().all(|x| x == &["#foo".to_string()]),
        "expected both args '#foo' (Go symbolToString), got: {a:?}"
    );
}

/// String/numeric literal member names render through the nodebuilder as
/// QUOTED string literals — `"1"`, not `1` (numericStringNamedPropertyEquivalence).
#[test]
fn fcs47_numeric_string_dup_2300_arg_pin() {
    let (mut c, _) = check_unit(
        "class C { \"1\": number; \"1.0\": number; 1.0: number; }\ninterface I { \"1\": number; \"1.\": number; 1: number; }\nvar a: { \"1\": number; 1.0: string; }\n",
        "/x/fcs47_numeric.ts",
        options_esnext(),
    );
    let ds = diags(&mut c);
    let a = args_of(&ds, 2300);
    assert_eq!(a.len(), 6, "expected 6 TS2300, got: {ds:?}");
    assert!(
        a.iter().all(|x| x == &["\"1\"".to_string()]),
        "expected all args '\"1\"' (quoted), got: {a:?}"
    );
}

/// `"a b"` stays quoted (stringNamedPropertyDuplicates).
#[test]
fn fcs47_string_named_dup_2300_arg_pin() {
    let (mut c, _) = check_unit(
        "interface I { \"a b\": number; \"a b\": string; }\n",
        "/x/fcs47_strname.ts",
        options_esnext(),
    );
    let ds = diags(&mut c);
    let a = args_of(&ds, 2300);
    assert_eq!(a.len(), 2, "expected 2 TS2300, got: {ds:?}");
    assert!(
        a.iter().all(|x| x == &["\"a b\"".to_string()]),
        "expected both args '\"a b\"' (quoted), got: {a:?}"
    );
}

/// Late-bound unique-symbol members render their computed name source
/// text `[OpNamespace.equal]` — never the internal `þ@equal@0`
/// (uniqueSymbolsPropertyNames).
#[test]
fn fcs47_unique_symbol_prop_2300_arg_pin() {
    let (mut c, _) = check_unit(
        "namespace OpNamespace { export declare const equal: unique symbol; }\nclass Cls2 {\n  [OpNamespace.equal] = \"first\";\n  [OpNamespace.equal] = \"last\";\n}\n",
        "/x/fcs47_uniqprop.ts",
        options_esnext(),
    );
    let ds = diags(&mut c);
    let a = args_of(&ds, 2300);
    assert_eq!(a.len(), 2, "expected 2 TS2300, got: {ds:?}");
    assert!(
        a.iter().all(|x| x == &["[OpNamespace.equal]".to_string()]),
        "expected both args '[OpNamespace.equal]', got: {a:?}"
    );
    assert!(
        !a.iter().any(|x| x.iter().any(|s| s.contains('\u{FE}'))),
        "no internal prefix may leak: {a:?}"
    );
}

// ── ROOT C — distinct unique symbols get distinct internal names ──

/// Two same-named unique symbol declarations (`const s: unique symbol` and
/// `{ s: unique symbol }`) build DIFFERENT `\xFE@s@<id>` names (Go's lazy
/// GetSymbolId), so `{ [s]: "a", [N.s]: "b" }` fires NO duplicate 2300
/// (uniqueSymbols.ts over-fire).
#[test]
fn fcs47_distinct_unique_symbols_no_dup_pin() {
    let (mut c, _) = check_unit(
        "declare const s: unique symbol;\ndeclare const N: { s: unique symbol };\ndeclare const o: { [s]: \"a\", [N.s]: \"b\" };\n",
        "/x/fcs47_distinct.ts",
        options_esnext(),
    );
    let ds = diags(&mut c);
    let a = args_of(&ds, 2300);
    assert_eq!(
        a.len(),
        0,
        "distinct unique symbols must not collide, got 2300s: {a:?} / all: {ds:?}"
    );
}

// ── ROOT B — the 2717 first/next inversion (SetValueDeclaration gate) ──

/// The late-bound symbol's value declaration keeps the FIRST declaration
/// (SetValueDeclaration precedence), so 2717 fires on the SUBSEQUENT
/// declaration with (name, firstType, nextType) = (`[s]`, number, string).
#[test]
fn fcs47_late_bound_2717_on_subsequent_pin() {
    let (mut c, _) = check_unit(
        "declare const s: unique symbol;\nclass A { [s]: number; [s]: string }\n",
        "/x/fcs47_2717.ts",
        options_esnext(),
    );
    let ds = diags(&mut c);
    let a = args_of(&ds, 2717);
    assert_eq!(a.len(), 1, "expected exactly 1 TS2717, got: {ds:?}");
    assert_eq!(
        a[0],
        ["[s]".to_string(), "number".to_string(), "string".to_string()],
        "expected 2717 on the subsequent declaration with (number, string), got: {a:?}"
    );
    // And the 2300 duplicate still fires with the computed-name rendering.
    let d = args_of(&ds, 2300);
    assert_eq!(d.len(), 2, "expected 2 TS2300 with '[s]', got: {ds:?}");
    assert!(d.iter().all(|x| x == &["[s]".to_string()]), "got: {d:?}");
}

// ── ROOT D — the TS4127 override-dynamic-name arm ──

/// `override [nonLiteralProp]()` members report TS4127 (isNonBindableDynamicName
/// = IsDynamicName && !isLateBindableName), and never fall through to 4113.
#[test]
fn fcs47_override_dynamic_name_4127_pin() {
    let options = CompilerOptions {
        no_implicit_override: Tristate::TSTrue,
        ..options_esnext()
    };
    let (mut c, _) = check_unit(
        "let prop = \"foo\"\nclass Base1 {\n  [prop]() {}\n}\nclass Derived1 extends Base1 {\n  override [prop]() {}\n}\nclass Base3 {}\nclass Derived3 extends Base3 {\n  override [prop]() {}\n}\n",
        "/x/fcs47_override.ts",
        options,
    );
    let ds = diags(&mut c);
    let a4127 = args_of(&ds, 4127);
    assert_eq!(
        a4127.len(),
        2,
        "expected TS4127 x2 (Derived1 + Derived3), got: {ds:?}"
    );
    let a4113 = args_of(&ds, 4113);
    assert_eq!(
        a4113.len(),
        0,
        "the 4127 arm must return HasInvalidOverride before the 4113 fall-through, got: {ds:?}"
    );
}

/// A non-bindable `symbol`-typed computed name is also dynamic (overrideLate
/// BindableIndexSignature1): 4127 fires, not 4113.
#[test]
fn fcs47_override_symbol_dynamic_4127_pin() {
    let options = CompilerOptions {
        no_implicit_override: Tristate::TSTrue,
        ..options_esnext()
    };
    let (mut c, _) = check_unit(
        "const sym: symbol = Symbol();\nclass Base1 {\n  [sym]() {}\n}\nclass Derived1 extends Base1 {\n  override [sym]() {}\n}\n",
        "/x/fcs47_override_sym.ts",
        options,
    );
    let ds = diags(&mut c);
    let a4127 = args_of(&ds, 4127);
    assert_eq!(a4127.len(), 1, "expected exactly 1 TS4127, got: {ds:?}");
    assert_eq!(args_of(&ds, 4113).len(), 0, "no 4113 fall-through: {ds:?}");
}

// ── ROOT E — the TS2635 instantiation-expression walk ──

/// The REAL getInstantiationExpressionType: `f<number>` (applicable generic
/// signature) stays silent; every non-applicable shape reports TS2635 with
/// Go's exact errorType rendering.
#[test]
fn fcs47_instantiation_expression_2635_pin() {
    let (mut c, _) = check_unit(
        "interface F {\n    <T>(x: T): T;\n    <T>(x: T, n: number): T;\n    <T, U>(t: [T, U]): [T, U];\n    (): number;\n    g<U>(): U;\n}\ndeclare const f: F;\nconst a = f<number>;\nconst b = f<number, string, boolean>;\ndeclare const o: { x: string; y: string; };\ndeclare const p: { x: string; } & { y: string; };\ndeclare const q: { x: string; };\ndeclare const r: (a: string, b: number) => void;\ndeclare const s: { new (a: string, b: number): void; };\nconst c = o<string>;\nconst d = p<string>;\nconst e = q<string>;\nconst g2 = r<string>;\nconst h = s<string>;\n",
        "/x/fcs47_instexpr.ts",
        options_esnext(),
    );
    let ds = diags(&mut c);
    let a = args_of(&ds, 2635);
    assert_eq!(a.len(), 6, "expected 6 TS2635, got: {ds:?}");
    for expected in [
        "F",
        "{ x: string; y: string; }",
        "{ x: string; } & { y: string; }",
        "{ x: string; }",
        "(a: string, b: number) => void",
        "new (a: string, b: number) => void",
    ] {
        assert!(
            a.iter()
                .any(|x| x.first().map(String::as_str) == Some(expected)),
            "expected a TS2635 arg {expected:?}, got: {a:?}"
        );
    }
    // `f<number>` matched the single-type-parameter signature — silent.
    assert!(
        !a.iter().any(|x| x.first().map(String::as_str) == Some("number")),
        "the applicable f<number> must not report: {a:?}"
    );
}

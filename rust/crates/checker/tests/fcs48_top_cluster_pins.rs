//! FCS48 pins (full-corpus check sweep, micro-tranche 48): the top coherent
//! clusters of the fresh t48 census — the staticPropertyNameConflicts
//! TS2300 family (the binder-injected class `prototype` export —
//! Property|Prototype, no declarations — vs the late-bound static
//! `["prototype"]` METHOD: combineSymbolTables now merges through
//! mergeSymbolTable/mergeSymbol, whose MethodExcludes (Value & ^Method)
//! include Property, so the conflict reaches reportMergeSymbolError with
//! the symbolToString rendering `[FunctionPropertyNames.prototype]`; the
//! PROPERTY variant merges silently because PropertyExcludes excludes
//! Property), and the destructuring accessibility TS2341 family
//! (checkVariableLikeDeclaration's binding-element arm now calls
//! markPropertyAsReferenced + checkPropertyAccessibility — the M6 S4
//! permissive skip).
//! ("FCS tranche 48" PORT notes in-tree.)

use tsc_ast::{NodeStore, SourceFile, SourceFileParseOptions, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::options_generated::{CompilerOptions, ModuleKind, ScriptTarget};
use tsc_core::scriptkind::ScriptKind;
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

fn options_es2015() -> CompilerOptions {
    CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::CommonJS,
        ..CompilerOptions::default()
    }
}

// ── ROOT A — the staticPropertyNameConflicts TS2300 family ──

const PROTOTYPE_SOURCE: &str = concat!(
    "const FunctionPropertyNames = {\n",
    "    prototype: 'prototype',\n",
    "} as const;\n",
    "class StaticPrototypeFn2 {\n",
    "    static [FunctionPropertyNames.prototype]() {} // always an error\n",
    "    [FunctionPropertyNames.prototype]() {} // ok\n",
    "}\n",
);

/// The late-bound static `["prototype"]` METHOD conflicts with the
/// binder-injected `prototype` export (Property|Prototype) in the class's
/// early static table — combineSymbolTables' mergeSymbol arm hits the
/// MethodExcludes conflict and reports TS2300 once, at the computed name
/// node, with the nodebuilder rendering of the late symbol (Go: the
/// reportMergeSymbolError path of mergeSymbol, checker.go:14367/14417).
#[test]
fn fcs48_static_prototype_computed_method_2300_pin() {
    let (mut c, _) = check_unit(
        PROTOTYPE_SOURCE,
        "/x/fcs48_static_proto.ts",
        options_es2015(),
    );
    let ds = diags(&mut c);
    let a = args_of(&ds, 2300);
    assert_eq!(
        a.len(),
        1,
        "expected exactly 1 TS2300 (method variant only), got: {ds:?}"
    );
    assert!(
        a.iter().all(|x| x == &["[FunctionPropertyNames.prototype]".to_string()]),
        "expected arg '[FunctionPropertyNames.prototype]' (Go symbolToString), got: {a:?}"
    );
}

/// The PROPERTY variant merges silently — PropertyExcludes excludes
/// Property, so the injected `prototype` and the late-bound static
/// `["prototype"]` property combine without the duplicate report (the
/// corpus record fires 2300 only on the three `()` method variants).
#[test]
fn fcs48_static_prototype_computed_property_silent_pin() {
    let (mut c, _) = check_unit(
        concat!(
            "const F = { prototype: 'prototype' } as const;\n",
            "class StaticPrototype2 {\n",
            "    static [F.prototype]: number; // no 2300\n",
            "    [F.prototype]: string; // ok\n",
            "}\n",
        ),
        "/x/fcs48_static_proto_prop.ts",
        options_es2015(),
    );
    let ds = diags(&mut c);
    assert_eq!(
        args_of(&ds, 2300).len(),
        0,
        "property variant must merge silently, got: {ds:?}"
    );
}

// ── ROOT B — the destructuring accessibility TS2341 family ──

/// `let { priv } = k;` outside the class now reports TS2341 through the
/// binding-element arm of checkVariableLikeDeclaration
/// (markPropertyAsReferenced + checkPropertyAccessibility, Go
/// checker.go:6031-6033) — the M6 S4 permissive skip.
#[test]
fn fcs48_destructuring_private_2341_pin() {
    let (mut c, _) = check_unit(
        concat!(
            "class K {\n",
            "    private priv;\n",
            "    m() {\n",
            "        let { priv: a } = this; // ok\n",
            "    }\n",
            "}\n",
            "let k = new K();\n",
            "let { priv } = k; // error\n",
        ),
        "/x/fcs48_destructuring.ts",
        options_es2015(),
    );
    let ds = diags(&mut c);
    let a = args_of(&ds, 2341);
    assert_eq!(a.len(), 1, "expected 1 TS2341, got: {ds:?}");
    assert_eq!(
        a[0],
        vec!["priv".to_string(), "K".to_string()],
        "expected args ['priv', 'K'], got: {a:?}"
    );
}

/// The in-class shorthand destructuring stays silent (accessibility
/// passes within the declaring class).
#[test]
fn fcs48_destructuring_private_in_class_ok_pin() {
    let (mut c, _) = check_unit(
        concat!(
            "class K {\n",
            "    private priv;\n",
            "    m() {\n",
            "        let { priv } = this; // ok\n",
            "    }\n",
            "}\n",
        ),
        "/x/fcs48_destructuring_ok.ts",
        options_es2015(),
    );
    let ds = diags(&mut c);
    assert_eq!(args_of(&ds, 2341).len(), 0, "in-class read is legal, got: {ds:?}");
}

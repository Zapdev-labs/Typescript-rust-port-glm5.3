//! FCS52 pins (full-corpus check sweep, micro-tranche 52): the tranche's
//! three clusters —
//!
//!  * the unicodeEscapesInJsxtags JSX-escape family: the scanner/parser
//!    already DECODE `\u0061` into the identifier text (the parser probe
//!    shows `a`, `a-b`, `Compa`, `data-video`); the raw escapes survived
//!    because three checker jsx.rs sites sliced the RAW source span
//!    (`get_text_of_node`) where Go jsx.go:145/1200/1228 reads the DECODED
//!    `tagName.Text()` — so `<\u0061-b>` never matched a declared
//!    IntrinsicElements entry (TS2339 x6). Fixed to `jsx_name_text`;
//!    pinned at the record level in fcs52_top_cluster_records.rs (the
//!    single-file TestProgram harness can't resolve the JSX namespace the
//!    way the corpus runner's react lib does);
//!  * the contextualTypedSpecialAssignment 7006 family: the reparser was
//!    missing Go's `KindExpressionStatement` case in reparseHosted's
//!    JSDocTypeTag arm (reparser.go:373-381) — Go attaches the `/** @type
//!    T */ a.b = expr` tag to the BINARY's type field so
//!    getContextualTypeForBinaryOperand (checker.go:30360) contextually
//!    types the RHS object literal — without it the object-literal
//!    methods' parameters fell to implicit any (TS7006 x8);
//!  * the mergedInterfaces* 2320 family: checkInheritedPropertiesAreIdentical
//!    seeded `seen` with ALL properties of the interface (the permissive
//!    stand-in) instead of Go's declared members (checker.go:5159), and
//!    compareProperties was a types-only stub that missed Go's
//!    accessibility/target-symbol/readonly ladder (checker.go:28206) — so
//!    two inherited private members never compared non-identical, the 2320
//!    "Interface 'X' cannot simultaneously extend types 'Y' and 'Z'" chain
//!    never fired, and the 2430 "incorrectly extends" loop leaked instead.
//!
//! ("FCS tranche 52" PORT notes in-tree.)

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

fn check_unit(
    src: &str,
    name: &str,
    kind: ScriptKind,
    options: CompilerOptions,
) -> (Checker, tsc_ast::NodeId) {
    let file_name = RootedFilePath::from(name);
    let path_key =
        tsc_tspath::CaseSensitivity::CaseSensitive.path_key(&file_name.as_path());
    let opts = SourceFileParseOptions {
        file_name,
        path_key,
        external_module_indicator_options: Default::default(),
    };
    let mut sf = parse_source_file(opts, src, kind);
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

fn options_es2015_js_strict() -> CompilerOptions {
    let mut options = CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::CommonJS,
        ..CompilerOptions::default()
    };
    options.allow_js = Tristate::TSTrue;
    options.check_js = Tristate::TSTrue;
    options.strict = Tristate::TSTrue;
    options
}

// ── CLUSTER B — the reparser's JSDocTypeTag ExpressionStatement arm ──

/// The corpus shape (contextualTypedSpecialAssignment.ts: the property /
/// this-property / exports / prototype assignment family): the `/** @type
/// {DoneStatus} */` above `ns.x = { ... m(n) {} }` must attach to the
/// assignment binary (Go reparser.go:373-381) so the object literal is
/// contextually typed and the method parameter `n` picks up `number` —
/// NOT implicit any (TS7006). Pre-fix the arm was absent and the
/// parameter fell to `Parameter 'n' implicitly has an 'any' type`.
#[test]
fn fcs52_type_tag_assignment_contextual_typing_pin() {
    let (mut c, _) = check_unit(
        "/** @typedef {{\n    status: 'done'\n    m(n: number): void\n}} DoneStatus */\nvar ns = {}\n/** @type {DoneStatus} */\nns.x = {\n    status: 'done',\n    m(n) { }\n}\n",
        "/x/fcs52_type_tag_assignment.js",
        ScriptKind::JS,
        options_es2015_js_strict(),
    );
    let ds = diags(&mut c);
    let implicit_any: Vec<&tsc_ast::Diagnostic> =
        ds.iter().filter(|d| d.code() == 7006).collect();
    assert!(
        implicit_any.is_empty(),
        "the @type-annotated assignment's object-literal method must be contextually typed, got 7006: {:?}",
        implicit_any.iter().map(|d| d.message_args()).collect::<Vec<_>>()
    );
}

/// The discriminating twin: the contextual type must actually FLOW — with
/// `/** @type {T} */ ns.x = { status: 'no' }` against `{{status: 1}}`, the
/// literal's `status` widens silently without the contextual type but
/// mismatches with TS2322 once the annotation reaches the RHS (Go's
/// reparseHosted → getContextualTypeForBinaryOperand chain).
#[test]
fn fcs52_type_tag_assignment_context_flows_pin() {
    let (mut c, _) = check_unit(
        "/** @typedef {{status: 1}} T */\nvar ns = {}\n/** @type {T} */\nns.x = {\n    status: 'no',\n}\n",
        "/x/fcs52_type_tag_assignment_flows.js",
        ScriptKind::JS,
        options_es2015_js_strict(),
    );
    let ds = diags(&mut c);
    let mismatch: Vec<&tsc_ast::Diagnostic> =
        ds.iter().filter(|d| d.code() == 2322).collect();
    assert!(
        mismatch
            .iter()
            .any(|d| d.message_args().first().map(String::as_str) == Some("string")),
        "the @type annotation must contextually type the RHS object literal (2322 string vs 1 expected), got: {:?}",
        ds.iter().map(|d| (d.code(), d.message_args())).collect::<Vec<_>>()
    );
}

// ── CLUSTER C — checkInheritedPropertiesAreIdentical + compareProperties ──

/// The corpus shape (mergedInterfacesWithInheritedPrivates3.ts): two
/// merged `interface A` declarations extending classes C and C2 whose
/// private `x` members originate in different declarations — the inherited
/// property is non-identical, so Go fires the 2320 chain HEAD (`Interface
/// 'A' cannot simultaneously extend types 'C' and 'C2'`,
/// checker.go:5166-5169) and SKIPS the 2430 incorrectly-extends loop.
/// Pre-fix: compareProperties compared only the types (number == number)
/// and the seeding walk included inherited properties, so `identical`
/// stayed true and the 2430 loop leaked.
#[test]
fn fcs52_merged_interfaces_private_2320_pin() {
    let (mut c, _) = check_unit(
        "class C {\n    private x: number;\n}\nclass C2 {\n    private x: number;\n}\ninterface A extends C {\n    y: string;\n}\ninterface A extends C2 {\n    z: string;\n}\n",
        "/x/fcs52_merged_privates.ts",
        ScriptKind::TS,
        CompilerOptions::default(),
    );
    let ds = diags(&mut c);
    let chain: Vec<&tsc_ast::Diagnostic> = ds.iter().filter(|d| d.code() == 2320).collect();
    assert_eq!(
        chain.len(),
        1,
        "exactly one 2320 chain head expected, got: {:?}",
        ds.iter().map(|d| (d.code(), d.message_args())).collect::<Vec<_>>()
    );
    let args = chain[0].message_args();
    assert_eq!(
        args,
        &vec!["A".to_string(), "C".to_string(), "C2".to_string()],
        "the 2320 head args must be the interface and both conflicting bases"
    );
    assert!(
        !ds.iter().any(|d| d.code() == 2430),
        "the 2320 gate must suppress the 2430 incorrectly-extends loop, got: {:?}",
        ds.iter().filter(|d| d.code() == 2430).map(|d| d.message_args()).collect::<Vec<_>>()
    );
}

/// The public-member twin: same-name public properties with different
/// types across two bases are also non-identical (Go compareProperties'
/// public arm + the compareTypes tail, checker.go:28229) — the 2320 fires
/// for `interface B { x: number } interface B2 { x: string } interface A
/// extends B, B2 {}`.
#[test]
fn fcs52_merged_interfaces_public_types_2320_pin() {
    let (mut c, _) = check_unit(
        "interface B {\n    x: number;\n}\ninterface B2 {\n    x: string;\n}\ninterface A extends B, B2 {\n    y: string;\n}\n",
        "/x/fcs52_merged_public.ts",
        ScriptKind::TS,
        CompilerOptions::default(),
    );
    let ds = diags(&mut c);
    let chain: Vec<&tsc_ast::Diagnostic> = ds.iter().filter(|d| d.code() == 2320).collect();
    assert_eq!(
        chain.len(),
        1,
        "the non-identical public members must fire one 2320, got: {:?}",
        ds.iter().map(|d| (d.code(), d.message_args())).collect::<Vec<_>>()
    );
    assert_eq!(
        chain[0].message_args(),
        &vec!["A".to_string(), "B".to_string(), "B2".to_string()],
        "the 2320 head args must be the interface and both conflicting bases"
    );
}

/// The negative control: identical member types across bases keep the
/// interface clean (no 2320, no 2430) — the ladder must not over-fire.
#[test]
fn fcs52_merged_interfaces_identical_members_clean_pin() {
    let (mut c, _) = check_unit(
        "interface B {\n    x: number;\n}\ninterface B2 {\n    x: number;\n}\ninterface A extends B, B2 {\n    y: string;\n}\n",
        "/x/fcs52_merged_identical.ts",
        ScriptKind::TS,
        CompilerOptions::default(),
    );
    let ds = diags(&mut c);
    assert!(
        ds.iter().all(|d| d.code() == 2318),
        "identical inherited members must not fire 2320/2430 (2318 lib-less noise aside), got: {:?}",
        ds.iter().map(|d| (d.code(), d.message_args())).collect::<Vec<_>>()
    );
}

//! FCS54 pins (full-corpus check sweep, micro-tranche 54): the tranche's
//! three clusters —
//!
//!  * the classStaticBlockUseBeforeDef TS7008/TS2729 family:
//!    `getFlowTypeOfProperty` (expressions.rs) was the M6 W3 permissive
//!    stub that returned `initialType` alone (Go's answer only when the
//!    reference carries no flow node), so `getFlowTypeInConstructor` /
//!    `getFlowTypeInStaticBlocks` never ran the flow walk and every
//!    synthetic `this.<name>` probe answered the UNDECLARED initial type
//!    — `static x; static { this.x = 1; }` reported TS7008 "Member 'x'
//!    implicitly has an 'any' type" instead of inferring `number`. Go
//!    checker.go:11688 rides getFlowTypeOfReferenceEx. The residual
//!    TS2729 (`static y = this.x` after the block) was the DIVERGENT
//!    STAND-IN `isPropertyInitializedInStaticBlocks` (resolution.rs) that
//!    scanned for a syntactic top-level assignment (matching the prop
//!    NAME NODE by identity); Go checker.go:5067 probes the block's
//!    return flow with a synthetic `this.<name>` reference and asks
//!    whether the flow type contains undefined;
//!  * the parser.asyncGenerators.objectLiteralMethods TS2318 x4 family:
//!    after the `async * get x() { ... }` parse recovery the member `get`
//!    carries a MISSING zero-length synthesized body; Go's
//!    getReturnTypeOfSignature gates `if !ast.NodeIsMissing(
//!    sig.declaration.Body())` (checker.go:20432) and answers anyType
//!    WITHOUT running getReturnTypeFromBody, so the async-generator wrap
//!    (createGeneratorType → the global AsyncIterableIterator lookup)
//!    never fires. The port's `function_body_of` Option treated the
//!    synthesized block as present (members.rs);
//!  * the interfaceExtendingClassWithProtecteds2 TS2430 x2 family:
//!    `isValidOverrideOf` (relater.rs) was the permissive M6 S8 stub
//!    (always true), so the relater's 2443 protected arm
//!    (checker.go:4310-4323) never failed the assignability and the 2430
//!    "Interface 'I4' incorrectly extends interface 'Foo'/'Bar'" head
//!    with its protected related-info never fired. Go checker.go:12167
//!    rides forEachProperty + isPropertyInClassDerivedFrom.
//!
//! ("FCS tranche 54" PORT notes in-tree.)

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

fn options_strict(target: ScriptTarget) -> CompilerOptions {
    CompilerOptions {
        target,
        module: ModuleKind::CommonJS,
        strict: Tristate::TSTrue,
        ..CompilerOptions::default()
    }
}

// ── CLUSTER A — the classStaticBlockUseBeforeDef family ──

/// The corpus shape (classStaticBlockUseBeforeDef1): the static blocks'
/// assignments infer the un-annotated static properties' types (x and z
/// widen to `number`) and the `static y = this.x` read is legal because x
/// is initialized in a preceding static block — NO TS7008 and NO TS2729.
/// Pre-fix the stub flow answers produced TS7008 x2 ("x any", "z any")
/// and TS2729 x1.
#[test]
fn fcs54_static_block_inference_and_use_ok_pin() {
    let (mut c, _) = check_unit(
        "class C {\n    static x;\n    static {\n        this.x = 1;\n    }\n    static y = this.x;\n    static z;\n    static {\n        this.z = this.y;\n    }\n}\n",
        "/x/fcs54_static_block_def1.ts",
        ScriptKind::TS,
        options_strict(ScriptTarget::ES2022),
    );
    let ds = diags(&mut c);
    assert!(
        !ds.iter().any(|d| d.code() == 7008),
        "the static-block assignments must infer the property types (no TS7008), got: {:?}",
        ds.iter().map(|d| (d.code(), d.message_args())).collect::<Vec<_>>()
    );
    assert!(
        !ds.iter().any(|d| d.code() == 2729),
        "the static y = this.x read is initialized by the preceding static block (no TS2729), got: {:?}",
        ds.iter().map(|d| (d.code(), d.message_args())).collect::<Vec<_>>()
    );
}

/// The block-BEFORE-declaration twin (classStaticBlockUseBeforeDef2):
/// the block's write still infers the type (no TS7008 — Go's answer is
/// `number` here), but the write itself happens before the property's
/// implicit initializer, so TS2729 fires — Go's answer, not a residual.
#[test]
fn fcs54_static_block_use_before_def_still_2729_pin() {
    let (mut c, _) = check_unit(
        "class C {\n    static {\n        this.x = 1;\n    }\n    static x;\n}\n",
        "/x/fcs54_static_block_def2.ts",
        ScriptKind::TS,
        options_strict(ScriptTarget::ES2022),
    );
    let ds = diags(&mut c);
    assert!(
        !ds.iter().any(|d| d.code() == 7008),
        "the block's write still infers the property type (no TS7008), got: {:?}",
        ds.iter().map(|d| (d.code(), d.message_args())).collect::<Vec<_>>()
    );
    let used_before: Vec<&tsc_ast::Diagnostic> =
        ds.iter().filter(|d| d.code() == 2729).collect();
    assert_eq!(
        used_before.len(),
        1,
        "the block-before-declaration write fires TS2729 (Go's answer), got: {:?}",
        ds.iter().map(|d| (d.code(), d.message_args())).collect::<Vec<_>>()
    );
    assert_eq!(
        used_before[0].message_args(),
        &vec!["x".to_string()],
        "the TS2729 must name the property"
    );
}

/// The auto-accessor variant (classStaticBlockUseBeforeDef5): the
/// synthetic flow probe runs for `static accessor x;` too — the block's
/// write infers the backing-property type, so NO TS7008.
#[test]
fn fcs54_static_block_auto_accessor_inference_pin() {
    let (mut c, _) = check_unit(
        "class C {\n    static {\n        this.x = 1;\n    }\n    static accessor x;\n}\n",
        "/x/fcs54_static_block_def5.ts",
        ScriptKind::TS,
        options_strict(ScriptTarget::ES2022),
    );
    let ds = diags(&mut c);
    assert!(
        !ds.iter().any(|d| d.code() == 7008),
        "the static-block write must infer the auto-accessor's backing type (no TS7008), got: {:?}",
        ds.iter().map(|d| (d.code(), d.message_args())).collect::<Vec<_>>()
    );
}

// ── CLUSTER B — the async-generator missing-body return-type gate ──

/// The corpus shape (parser.asyncGenerators.objectLiteralMethods — the
/// asyncGeneratorGetAccessorIsError subfile): after the "'(' expected"
/// parse recovery the member `get` carries a MISSING zero-length
/// synthesized body; the deferred forced return-type resolution answers
/// anyType WITHOUT touching the async-generator wrap, so NO TS2318
/// "Cannot find global type 'AsyncIterableIterator'". Pre-fix the
/// synthesized body read as present and the wrap fired.
#[test]
fn fcs54_async_generator_recovered_accessor_no_2318_pin() {
    let (mut c, _) = check_unit(
        "const o2 = {\n    async * get x() {\n        return 1;\n    }\n};\n",
        "/x/fcs54_async_gen_accessor.ts",
        ScriptKind::TS,
        options_strict(ScriptTarget::ES2018),
    );
    let ds = diags(&mut c);
    assert!(
        !ds
            .iter()
            .any(|d| d.code() == 2318 && d.message_args().iter().any(|a| *a == "AsyncIterableIterator")),
        "the recovered member's missing body must answer anyType without the AsyncIterableIterator wrap, got: {:?}",
        ds.iter().map(|d| (d.code(), d.message_args())).collect::<Vec<_>>()
    );
}

/// The positive control: a VALID async-generator object-literal method
/// still resolves its return type through the wrap — the global
/// AsyncIterableIterator lookup (with error reporting) still fires
/// TS2318 in a lib-less program, on BOTH drivers.
#[test]
fn fcs54_async_generator_valid_method_still_resolves_pin() {
    let (mut c, _) = check_unit(
        "const o1 = {\n    async * f() {\n        yield 1;\n    }\n};\n",
        "/x/fcs54_async_gen_valid.ts",
        ScriptKind::TS,
        options_strict(ScriptTarget::ES2018),
    );
    let ds = diags(&mut c);
    assert!(
        ds
            .iter()
            .any(|d| d.code() == 2318 && d.message_args().iter().any(|a| *a == "AsyncIterableIterator")),
        "the valid async-generator method must still resolve its return type through the AsyncIterableIterator wrap, got: {:?}",
        ds.iter().map(|d| (d.code(), d.message_args())).collect::<Vec<_>>()
    );
}

// ── CLUSTER C — the protected-override relation ──

/// The corpus shape (interfaceExtendingClassWithProtecteds2): I4 declares
/// its own public `x` shadowing the bases' protected `x` — the
/// inherited-identical gate passes (2320 only for I3), and the per-base
/// assignability then FAILS on accessibility: TS2430 "Interface 'I4'
/// incorrectly extends interface 'Bar'" + the Foo twin (Go's
/// isValidOverrideOf — the 2443 protected related-info under the head).
#[test]
fn fcs54_interface_protected_shadow_2430_pin() {
    let (mut c, _) = check_unit(
        "class Foo {\n    protected x!: string;\n}\n\nclass Bar {\n    protected x!: string;\n}\n\ninterface I3 extends Foo, Bar { // error\n}\n\ninterface I4 extends Foo, Bar { // error\n    x: string;\n}\n",
        "/x/fcs54_protecteds2.ts",
        ScriptKind::TS,
        options_strict(ScriptTarget::ES2015),
    );
    let ds = diags(&mut c);
    let incorrectly: Vec<&tsc_ast::Diagnostic> =
        ds.iter().filter(|d| d.code() == 2430).collect();
    assert_eq!(
        incorrectly.len(),
        2,
        "exactly two 2430 heads expected (I4-vs-Bar and I4-vs-Foo), got: {:?}",
        ds.iter().map(|d| (d.code(), d.message_args())).collect::<Vec<_>>()
    );
    let mut arg_pairs: Vec<Vec<String>> =
        incorrectly.iter().map(|d| d.message_args().to_vec()).collect();
    arg_pairs.sort();
    assert_eq!(
        arg_pairs,
        vec![
            vec!["I4".to_string(), "Bar".to_string()],
            vec!["I4".to_string(), "Foo".to_string()],
        ],
        "the 2430 heads must name I4 against each base"
    );
    assert!(
        ds.iter().any(|d| d.code() == 2320),
        "I3's inherited-identical TS2320 head must still fire"
    );
}

/// The clean control: an interface extending a class with a protected
/// member WITHOUT redeclaring it resolves the target property to the SAME
/// symbol (the `sourceProp != targetProp` skip), so the protected arm —
/// and isValidOverrideOf — is never consulted; no errors.
#[test]
fn fcs54_interface_protected_clean_extend_pin() {
    let (mut c, _) = check_unit(
        "class Foo {\n    protected x!: string;\n}\n\ninterface J extends Foo {\n    z: string;\n}\n\ndeclare var j: J;\nvar r: string = j.z;\n",
        "/x/fcs54_protecteds_clean.ts",
        ScriptKind::TS,
        options_strict(ScriptTarget::ES2015),
    );
    let ds = diags(&mut c);
    assert!(
        !ds.iter().any(|d| d.code() == 2430 || d.code() == 2320),
        "the same-symbol protected member must not fire 2430/2320, got: {:?}",
        ds.iter().map(|d| (d.code(), d.message_args())).collect::<Vec<_>>()
    );
}

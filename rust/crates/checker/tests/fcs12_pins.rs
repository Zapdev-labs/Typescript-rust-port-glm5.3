//! FCS12 pins (full-corpus check sweep, tranche 12): the property-access
//! flow-narrowing seam, the this-type flow wrap, the call-target
//! Cannot-invoke reporter, and the accessor implicit-any ladder.
//!
//! ROOTS (all pins' expectations verified byte-same against the go-bench
//! twin's check-stage dump over the same source shapes —
//! $HOME/.fcs11-scratch/repro4/ on the rig, plus the fcs12 probe drills):
//!
//! 1. `getFlowTypeOfAccessExpression` (Go checker.go:11629) was a permissive
//!    seam — `let flow_type = prop_type;` — so NO property access ever
//!    narrowed (`if (this.z && dz) { this.z += dz; }` reported spurious
//!    TS2532 on `this.z`; the 45-extra TS2532 family). The real call is
//!    `getFlowTypeOfReferenceEx(node, propType, addOptionalityEx(propType,
//!    false, assumeUninitialized), nil, nil)` (checker.go:11651).
//!    `tryGetThisTypeAtEx` (checker.go:12394) additionally wraps every
//!    this-type answer in `getFlowTypeOfReference(node, t)` (the
//!    signature arm 12401 and BOTH class arms 12408-12412); the port
//!    returned the raw declared type.
//!
//! 2. `reportCannotInvokePossiblyNullOrUndefinedError` (Go checker.go:10119)
//!    emits the `Cannot_invoke_an_object_which_is_possibly_{undefined}` family
//!    (TS2722/2721/2723) for call targets — the port emitted the generic
//!    Object_is_possibly_undefined (TS2532) family, so every
//!    possibly-undefined invocation diverged (logicalAssignment5's 8
//!    TS2532-extras were TS2722s in the reference).
//!
//! 3. `getTypeOfAccessors`' implicit-any ladder (Go checker.go:18944-18952)
//!    is `errorOrSuggestion(noImplicitAny, ...)`: setter → 7032, getter →
//!    7033, auto-accessor → 7008, each gated by `isPrivateWithinAmbient`.
//!    The port had a TODO stub for the setter arm (no 7032 at all) and an
//!    ungated unconditional 7033.

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::compileroptions::ResolutionModeNone;
use tsc_core::options_generated::CompilerOptions;
use tsc_core::scriptkind::ScriptKind;
use tsc_core::tristate::Tristate;
use tsc_parser::parse_source_file;
use tsc_tspath::RootedFilePath;

// ── The single-file Program scaffold (the fcs5-fcs11 idiom) ──

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

/// Check `src` over the single-file scaffold with strictNullChecks and
/// noImplicitAny (the @strict corpus default for these families; no libs —
/// the corpus seam).
fn check_strict(src: &str) -> (Checker, tsc_ast::NodeId) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let options = CompilerOptions {
        strict_null_checks: Tristate::TSTrue,
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

// ── Pin 1: the property-access flow walk (the this-property TS2532 family) ──

const THIS_PROPERTY_NARROWING: &str = "interface Point {\n\
    x: number;\n\
    y: number;\n\
    z?: number;\n\
    moveBy(dx: number, dy: number, dz?: number): void;\n\
}\n\
let p3: Point | undefined = {\n\
    x: 10,\n\
    y: 20,\n\
    moveBy(dx, dy, dz) {\n\
        this.x += dx;\n\
        this.y += dy;\n\
        if (this.z && dz) {\n\
            this.z += dz;\n\
        }\n\
    }\n\
};\n";

#[test]
fn this_property_access_flow_narrows_pin() {
    // checker.go:11651: getFlowTypeOfAccessExpression must run the flow walk
    // (getFlowTypeOfReferenceEx over the reference's flow node — the
    // antecedent chain of `if (this.z && dz)`), NOT return the declared
    // property type. The former seam left `this.z` at `number | undefined`
    // inside the guard and reported TS2532 on `this.z += dz` (the fcs11 [s7]
    // repro, repro4/k2.ts). Verified byte-same against the go twin
    // (repro4: k2.ts identical, 10/11 errors both drivers).
    let (mut c, file) = check_strict(THIS_PROPERTY_NARROWING);
    let ds = diags(&mut c, file);
    assert!(
        !ds.iter().any(|(code, _)| *code == 2532),
        "the this.z guard must flow-narrow the this.z += dz reference: {ds:?}"
    );
}

// ── Pin 2: the call-target Cannot-invoke reporter ──

const LOGICAL_AND_ASSIGN_CALL: &str = "function foo3(f?: (a: number) => void) {\n\
    f &&= (a => a)\n\
    f(42)\n\
}\n";

#[test]
fn possibly_undefined_call_target_reports_cannot_invoke_pin() {
    // checker.go:8718/10119: resolveCall checks the callee with
    // reportCannotInvokePossiblyNullOrUndefinedError, which picks the
    // Cannot_invoke_an_object_which_is_possibly_undefined message (TS2722) —
    // NOT the generic Object_is_possibly_undefined (TS2532). `f` stays
    // `((a: number) => void) | undefined` after `f &&=` (both drivers agree,
    // matching the logicalAssignment5 reference baseline), and the invocation
    // reports 2722. Verified byte-same against the go twin
    // (logicalAssignment5.ts: 0 diff lines).
    let (mut c, file) = check_strict(LOGICAL_AND_ASSIGN_CALL);
    let ds = diags(&mut c, file);
    assert!(
        ds.iter().any(|(code, _)| *code == 2722),
        "the possibly-undefined callee must report TS2722: {ds:?}"
    );
    assert!(
        !ds.iter().any(|(code, _)| *code == 2532),
        "the call-target arm must not report the generic TS2532: {ds:?}"
    );
}

// ── Pin 3: the accessor implicit-any ladder (7032) ──

const SET_ACCESSOR_WITHOUT_PARAM_ANNOTATION: &str = "let obj1 = {\n\
    a: 1,\n\
    f() {\n\
        return this.a;\n\
    },\n\
    get d() {\n\
        return this.a;\n\
    },\n\
    set e(value) {\n\
        this.b = value;\n\
    }\n\
};\n";

#[test]
fn set_accessor_without_annotation_reports_7032_pin() {
    // checker.go:18946: when neither accessor carries an annotation and no
    // getter body infers a type, the setter arm fires
    // errorOrSuggestion(noImplicitAny, ...) with TS7032 "Property 'e'
    // implicitly has type 'any', because its set accessor lacks a parameter
    // type annotation." — the former TODO stub emitted nothing (the fcs11
    // repro4/k1.ts divergence). Verified byte-same against the go twin
    // (repro4: k1.ts identical, 13/14 errors both drivers).
    let (mut c, file) = check_strict(SET_ACCESSOR_WITHOUT_PARAM_ANNOTATION);
    let ds = diags(&mut c, file);
    assert!(
        ds.iter().any(|(code, args)| *code == 7032
            && args.first().is_some_and(|a| a == "e")),
        "the un-annotated set accessor must report TS7032 for 'e': {ds:?}"
    );
}

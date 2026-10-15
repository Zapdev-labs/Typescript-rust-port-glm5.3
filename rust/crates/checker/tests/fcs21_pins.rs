//! FCS21 pins (full-corpus check sweep, tranche 21): the TS2339 MISSING
//! side — the JS expando assignment-declaration typing seam.
//!
//! ROOT (expectations verified byte-same against the go-bench twin's
//! check-stage dump — the rig probe battery p1_proto/p2_protoRead/
//! p3_ctorThis/p4_addon + the 13-case repro corpus, PROBES-IDENTICAL and
//! typeFromPrototypeAssignment 1/2/3 + moduleExportAlias's 22 recovered
//! 2339s after the fix):
//!
//! Two Go-exact port gaps, one family:
//! 1. `getWidenedTypeForAssignmentDeclaration` (checker.go:18478) was the
//!    M6 S4 permissive anyType stub and `isConstructorDeclaredThisProperty`
//!    (checker.go:18602) the permissive None/None stub — EVERY JS expando
//!    assignment-declaration symbol (`this.x = ...`, `C.prototype = {...}`,
//!    `C.prototype.m = ...`, `exports.x = ...`, Object.defineProperty)
//!    typed as any, so property-not-exist never fired on expando types.
//!    (Real bodies: the kind ladder + declarations scan +
//!    getAssignmentDeclarationInitializerType + the JS all-nullable
//!    implicit-any report; checker.go:18478-18647 + 27919.)
//! 2. `getContextualTypeForAssignmentExpression` (checker.go) was the M6
//!    W2/W4 permissive stub (`Some(get_type_of_expression(left))` — Go's
//!    tail). Go's real body answers nil for `F.id = expr` /
//!    `F[xxx] = expr` assignment declarations (binary.Symbol set) unless
//!    `F` is an annotated VariableDeclaration — the CIRCULARITY GUARD.
//!    The stub's unconditional left-type resolution re-entered
//!    getTypeOfSymbol(F.id) mid-resolution, hit the circularity guard and
//!    cached errorType (any), which re-poisoned the whole family even
//!    after fix 1 landed. (Plus the `/** @type T */ x = expr` binary
//!    annotation arm in getContextualTypeForBinaryOperand.)

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::compileroptions::ResolutionModeNone;
use tsc_core::options_generated::CompilerOptions;
use tsc_core::scriptkind::ScriptKind;
use tsc_parser::parse_source_file;
use tsc_tspath::RootedFilePath;

// ── The single-file Program scaffold (the fcs5-fcs20 idiom, JS flavor) ──

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

/// Check `src` (a JavaScript unit) over the single-file scaffold (no libs —
/// the corpus seam). The expando family only binds in JS files
/// (bindDeferredExpandoAssignments / the checker's isInJSFile arms), so the
/// scaffold parses as ScriptKind::JS.
fn check_js(src: &str) -> (Checker, tsc_ast::NodeId) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::JS);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let program = TestProgram {
        options: CompilerOptions::default(),
        name: RootedFilePath::from("/x/unit.js"),
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

// ── Pin 1: `C.prototype = {...}` types as the literal; `this` inside the
// literal's methods is the literal type ──

const PROTOTYPE_LITERAL_THIS: &str = "var C = function() { this.x = 1; };\n\
C.prototype = {\n\
    m: function() { this.q; }\n\
};\n\
C.prototype.addon = function() {};\n";

#[test]
fn prototype_literal_types_and_this_report_2339_pin() {
    // The expando `prototype` symbol on the JS function types as the
    // WIDENED ASSIGNED type `{ m: () => void; }` (Go
    // getWidenedTypeForAssignmentDeclaration — the permissive-anyType stub
    // typed it any and the 2339s never fired), so:
    //  - `this.q` inside the literal method reports 2339 ('q' on the
    //    literal type — the containing-literal this arm), and
    //  - `C.prototype.addon = ...` reports 2339 on the assignment target
    //    ('addon' not on the literal type yet).
    // Expectations verified byte-same against the go-bench twin (rig
    // probe p1_proto + p4_addon, PROBES-IDENTICAL).
    let (mut c, file) = check_js(PROTOTYPE_LITERAL_THIS);
    let ds = diags(&mut c, file);
    let e2339: Vec<&Vec<String>> = ds
        .iter()
        .filter(|(code, _)| *code == 2339)
        .map(|(_, args)| args)
        .collect();
    assert_eq!(e2339.len(), 2, "exactly the two expando 2339s: {ds:?}");
    assert_eq!(e2339[0][0], "q", "the this-in-literal-method access: {ds:?}");
    assert_eq!(e2339[0][1], "{ m: () => void; }", "the literal type: {ds:?}");
    assert_eq!(e2339[1][0], "addon", "the assignment-target access: {ds:?}");
    assert_eq!(e2339[1][1], "{ m: () => void; }", "the literal type: {ds:?}");
}

// ── Pin 2: a pure READ through the expando prototype reports 2339 ──

const PROTOTYPE_READ: &str = "var C = function() { this.x = 1; };\n\
C.prototype = { m: function() {} };\n\
C.prototype.zz;\n";

#[test]
fn prototype_read_reports_2339_pin() {
    // `C.prototype.zz` — a pure read of a missing property on the expando
    // prototype type `{ m: () => void; }` reports 2339 (Go twin errors=13
    // for this file — identical after the fix; the permissive-anyType stub
    // typed the prototype any and the read was silent).
    let (mut c, file) = check_js(PROTOTYPE_READ);
    let ds = diags(&mut c, file);
    let e2339: Vec<&Vec<String>> = ds
        .iter()
        .filter(|(code, _)| *code == 2339)
        .map(|(_, args)| args)
        .collect();
    assert_eq!(e2339.len(), 1, "exactly one 2339 on the read: {ds:?}");
    assert_eq!(e2339[0][0], "zz", "the read property: {ds:?}");
    assert_eq!(e2339[0][1], "{ m: () => void; }", "the literal type: {ds:?}");
}

// ── Pin 3: the post-assignment expando member — assignment target AND
// subsequent read both 2339 (the literal type is a separate anonymous
// type from the prototype symbol's expando exports) ──

const PROTOTYPE_ADDON_READ: &str = "var C = function() { this.x = 1; };\n\
C.prototype = { m: function() {} };\n\
C.prototype.addon = function() {};\n\
C.prototype.addon;\n\
C.prototype.nothing;\n";

#[test]
fn prototype_addon_assignment_and_read_report_2339_pin() {
    // `C.prototype.addon = fn` reports 2339 on the target ('addon' not on
    // `{ m: () => void; }`), the READ `C.prototype.addon` still reports
    // (the object-literal type assigned to `C.prototype` is a separate
    // anonymous type from the prototype symbol's expando exports), and
    // `C.prototype.nothing` reports. Expectations verified byte-same
    // against the go-bench twin (rig probe p4_addon, IDENTICAL).
    let (mut c, file) = check_js(PROTOTYPE_ADDON_READ);
    let ds = diags(&mut c, file);
    let e2339: Vec<&Vec<String>> = ds
        .iter()
        .filter(|(code, _)| *code == 2339)
        .map(|(_, args)| args)
        .collect();
    assert_eq!(e2339.len(), 3, "addon@assign + addon@read + nothing@read: {ds:?}");
    assert_eq!(e2339[0][0], "addon", "{ds:?}");
    assert_eq!(e2339[1][0], "addon", "{ds:?}");
    assert_eq!(e2339[2][0], "nothing", "{ds:?}");
    for args in &e2339 {
        assert_eq!(args[1], "{ m: () => void; }", "the literal type: {ds:?}");
    }
}

// ── Pin 4: `this.x = ...` inside a JS CONSTRUCTOR FUNCTION types the
// this-property expando via the flow assignment (getFlowTypeInConstructor
// is the class-constructor arm; JS constructor functions take the
// declarations-scan arm) — a same-named this-reference inside the
// initializer suppresses the declaration (containsSameNamedThisProperty) ──

const THIS_PROPERTY_TYPED: &str = "var C = function() { this.x = 1; this.y = this.x; };\n\
C.prototype = { m: function() { this.y; this.x; } };\n";

#[test]
fn this_property_assignment_declared_type_pin() {
    // `this.x = 1` / `this.y = this.x` inside the JS constructor function
    // create this-property expandos; their types come from the widened
    // assigned initializer types (number), NOT any — and the
    // containsSameNamedThisProperty guard does NOT suppress them (the
    // initializer references a DIFFERENT this-property). Inside the
    // prototype literal method, `this.y`/`this.x` report 2339 — the
    // containing-literal this type `{ m: () => void; }` has neither.
    // (Verified against the go-bench twin — the literal-method this
    // accesses on the {m}-only type.)
    let (mut c, file) = check_js(THIS_PROPERTY_TYPED);
    let ds = diags(&mut c, file);
    let e2339: Vec<&Vec<String>> = ds
        .iter()
        .filter(|(code, _)| *code == 2339)
        .map(|(_, args)| args)
        .collect();
    assert_eq!(e2339.len(), 2, "this.y and this.x in the literal method: {ds:?}");
    assert!(e2339.iter().any(|a| a[0] == "y"), "{ds:?}");
    assert!(e2339.iter().any(|a| a[0] == "x"), "{ds:?}");
}

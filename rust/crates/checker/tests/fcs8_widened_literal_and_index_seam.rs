//! FCS8 pins (full-corpus check sweep, tranche 8): the widened-literal
//! layer + the object-literal-method contextual seam + the computed-property
//! index-info seam.
//!
//! Three root causes, all Go-exact ports (every pin's expectation verified
//! byte-same against the go-bench twin's check-stage dump over the same
//! source shape — $HOME/.fcs8-scratch/probe{1,2,5,9,A} on the rig):
//!
//! 1. `getApparentTypeOfContextualType`'s object-literal-method branch passed
//!    the PARENT node to getContextualTypeForObjectLiteralElement; Go
//!    (checker.go:31238) passes the METHOD node to
//!    getContextualTypeForObjectLiteralMethod (checker.go:30513). The bug
//!    starved object literal methods of their contextual signature — their
//!    parameters fell to implicit any (the TS7006 extras on
//!    thisTypeInObjectLiterals2) and getter returns kept unwidened literal
//!    types.
//! 2. `getReturnTypeFromBody`'s isUnitType block (Go checker.go:20615-20633):
//!    a unit-type return widens its fresh literal to the base primitive
//!    (get(): "hi" → string) unless the contextual signature's return type
//!    makes it a literal context (getWidenedLiteralLikeTypeForContextual*
//!    Needed, checker.go:20811/20827 — ported with the block).
//! 3. `getIndexedAccessTypeOrUndefined` was the S7 stub (checker.go:27469 —
//!    index.rs's real resolver never ran for check-stage element accesses,
//!    so the TS7053 noImplicitAny ladder never fired), and
//!    `createObjectLiteralType` (Go checker.go:13439-13450) never
//!    synthesized the computed-property index infos (getObjectLiteralIndexInfo)
//!    — an object literal with a computed string/number/symbol property
//!    indexes through a synthetic index signature (e.g.
//!    `{ [Symbol.foo]: 0 }` indexes as `number`), the corpus's TS7053-missing
//!    family root (missing 314 → 49).

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::options_generated::CompilerOptions;
use tsc_core::scriptkind::ScriptKind;
use tsc_core::tristate::Tristate;
use tsc_parser::parse_source_file;
use tsc_tspath::RootedFilePath;

// ── The single-file Program scaffold (the fcs5/fcs6 idiom) ──

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
}

/// Check `src` over the single-file scaffold with the given strict flags
/// (the conformance default is all-strict for this family's shapes).
fn check_strict(src: &str) -> (Checker, tsc_ast::NodeId) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let options = CompilerOptions {
        no_implicit_any: Tristate::TSTrue,
        no_implicit_this: Tristate::TSTrue,
        strict_null_checks: Tristate::TSTrue,
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

// ── The pins ───────────────────────────────────────────────────────────────

/// Pin 1 — the object-literal-method contextual seam: a method in an object
/// literal contextually typed by an interface receives the contextual
/// signature, so its parameters are contextually typed (NO TS7006 —
/// thisTypeInObjectLiterals2's moveBy(dx, dy, dz) family; both the plain
/// interface and the null/undefined-union forms).
#[test]
fn object_literal_method_contextual_params_pin() {
    let (mut c, file) = check_strict(
        "interface I { m(x: number, y: number): void; }\n\
         let v: I = { m(x, y) { const z = x; } };\n\
         let w: I | null = { m(x, y) { const z = x; } };\n",
    );
    let pairs = diags(&mut c, file);
    assert!(
        !pairs.iter().any(|(code, _)| *code == 7006),
        "the contextually typed method parameters must not report TS7006: {pairs:?}"
    );
}

/// Pin 2 — the widened-literal return layer: an object literal method's
/// unit-type return widens its fresh literal to the base primitive with no
/// contextual signature (`m(): "hi"` → `m(): string` — Go checker.go:20625 +
/// 20811), so the 2322 rendering shows `{ m(): string; }` exactly like the
/// twin (probe5/y4).
#[test]
fn object_literal_method_return_widens_literal_pin() {
    let (mut c, file) = check_strict(
        "let o = { m() { return \"hi\"; } };\nconst t: number = o;\n",
    );
    let pairs = diags(&mut c, file);
    let msg = pairs
        .iter()
        .find(|(code, _)| *code == 2322)
        .map(|(_, args)| args.first().cloned().unwrap_or_default())
        .unwrap_or_default();
    assert_eq!(
        msg, "{ m(): string; }",
        "the widened literal type must render m(): string (got {msg:?}; full {pairs:?})"
    );
}

/// Pin 3 — the getter-shaped member keeps the widened return inside the
/// printed property type (thisTypeInObjectLiterals2's 4406 2339-arg fix):
/// `hello: { get() { return \"hi\"; }, set(value: string) {} }` renders the
/// getter's return as `string` (the twin's exact dump arg:
/// `{ get(): string; set(value: string): void; }` with the widened type).
#[test]
fn getter_member_widened_return_render_pin() {
    let (mut c, file) = check_strict(
        "let h = { hello: { get() { return \"hi\"; }, set(value: string) {} } };\n\
         h.hello.foo;\n",
    );
    let pairs = diags(&mut c, file);
    let msg = pairs
        .iter()
        .find(|(code, _)| *code == 2339)
        .map(|(_, args)| args.get(1).cloned().unwrap_or_default())
        .unwrap_or_default();
    // The exact twin rendering (verified against the go-bench dump over
    // probe1/min2): `{ get(): string; set(value: string); void; }`-shaped —
    // the widened `string` return (not `"hi"`) is what the pin guards.
    assert!(
        msg.contains("get(): string;"),
        "the getter's widened return must render string (got {msg:?}; full {pairs:?})"
    );
}

/// Pin 4 — the computed-property index-info seam (ES5SymbolProperty1's
/// shape): `{ [Symbol.foo]: 0 }` carries the synthetic string index info,
/// so `obj[Symbol.foo]` indexes as number — NO TS7053 (the twin's exact
/// answer; the old port reported the 7053 that Go never does).
#[test]
fn computed_property_index_info_no_7053_pin() {
    let (mut c, file) = check_strict(
        "interface SymbolConstructor { foo: string; }\n\
         var Symbol: SymbolConstructor;\n\
         var obj = {\n    [Symbol.foo]: 0\n}\n\
         obj[Symbol.foo];\n",
    );
    let pairs = diags(&mut c, file);
    assert!(
        !pairs.iter().any(|(code, _)| *code == 7053),
        "the computed string property synthesizes the index info — no TS7053: {pairs:?}"
    );
}

/// Pin 5 — the TS7053 ladder (the real getIndexedAccessTypeOrUndefined path):
/// a number index on an interface without index signatures reports
/// TS7053 with Go's exact args ("number I2") — the corpus's
/// parserharness/indexSignatures1 family (was silent in the port).
#[test]
fn number_index_no_signature_7053_ladder_pin() {
    let (mut c, file) = check_strict(
        "interface I2 { x: number; }\n\
         declare const i2: I2;\n\
         declare const n: number;\n\
         const v = i2[n];\n",
    );
    let pairs = diags(&mut c, file);
    assert!(
        pairs
            .iter()
            .any(|(code, args)| *code == 7053 && args.first().map(String::as_str) == Some("number")),
        "the number index on the index-signature-less interface must report TS7053: {pairs:?}"
    );
}

/// Pin 6 — the same ladder's string form: a `string` index on `I1` reports
/// TS7053 with args ("string I1") — probe9/i1's twin answer.
#[test]
fn string_index_no_signature_7053_ladder_pin() {
    let (mut c, file) = check_strict(
        "interface I1 { x: string; }\n\
         declare const s: string;\n\
         declare const i1: I1;\n\
         const v = i1[s];\n",
    );
    let pairs = diags(&mut c, file);
    assert!(
        pairs
            .iter()
            .any(|(code, args)| *code == 7053 && args.first().map(String::as_str) == Some("string")),
        "the string index on the index-signature-less interface must report TS7053: {pairs:?}"
    );
}

/// Pin 7 — the widened-literal contextual preservation: a method whose
/// contextual signature returns the literal type keeps its fresh literal
/// return (Go's isLiteralOfContextualType arm — the contextual-signature
/// switch in getReturnTypeFromBody's unit-type block takes the
/// `contextualSignature == getSignatureFromDeclaration(fn)` arm and hands
/// `"hi"` to getWidenedLiteralLikeTypeForContextualType against `"hi"`).
/// If the port wrongly widened to `string`, the call would report TS2345
/// (`string` is not assignable to `"hi"`); the Go answer assigns cleanly.
#[test]
fn contextual_literal_return_preserved_pin() {
    let (mut c, file) = check_strict(
        "interface I { m(): \"hi\"; }\n\
         declare function take(x: I): void;\n\
         take({ m() { return \"hi\"; } });\n",
    );
    let pairs = diags(&mut c, file);
    assert!(
        !pairs.iter().any(|(code, _)| *code == 2345),
        "the literal contextually-typed method keeps its literal return — no TS2345: {pairs:?}"
    );
}

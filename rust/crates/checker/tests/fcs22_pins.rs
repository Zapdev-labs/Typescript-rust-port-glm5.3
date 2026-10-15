//! FCS22 pins (full-corpus check sweep, tranche 22): the TS2339 MISSING
//! remainder's top cluster — the computed-name object-literal METHOD
//! contextual-signature seam
//! (es6/computedProperties/computedPropertyNamesContextualType{1,2,3}_ES{5,6},
//! 18 missing 2339 instances / 6 records, each paired with the extra TS7006
//! on the method's un-annotated parameter).
//!
//! ROOT (expectations verified byte-same against the go-bench twin's
//! check-stage dump — the rig probe battery computedPropertyNamesContextual
//! Type1_ES5 both-drivers, PROBES-IDENTICAL after the fix):
//!
//! Go's `getContextualTypeForObjectLiteralElement` (checker.go:30469) reads
//! the element's name through the GENERATED `element.Name()` accessor family
//! (ast.go — MethodDeclaration, PropertyAssignment, accessors, …), at both
//! the `ast.GetNameOfDeclaration` dynamic-name arm (checker.go:30494) and
//! the index-signature arm (checker.go:30500). The port's local `name_of_node`
//! helper in contextual.rs carried only the variable/parameter/property
//! subset, so a computed-name METHOD (`["" + 0](y) { return y.length; }`)
//! fell through both arms with no name node: no contextual type → the
//! method's parameters resolved to implicit any (the extra TS7006) and the
//! `y.length` property access never fired its TS2339. The PropertyAssignment
//! arm of the same object literal worked (its kind had a name arm), which is
//! why the twin dumps agreed on the arrow-function half of every record.
//!
//! FIX: the two element-impl name reads now use the generated Go-exact
//! `Node::name()` twin (ast_generated.rs — the full `Name()` family). The
//! method's contextual type now comes from the applicable index signature
//! (`getLiteralTypeFromPropertyName(ComputedPropertyName)` = the regular
//! type of `"" + 0` = string → `findApplicableIndexInfo` picks
//! `[s: string]: (x: string) => number`), its parameter contextually types as
//! string, and `y.length` reports 2339 like the Go twin (the pin scaffold
//! runs without libs, so `string`'s apparent type has no `length` — the
//! corpus seam; with libs both drivers agree the case is clean).

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::compileroptions::ResolutionModeNone;
use tsc_core::options_generated::CompilerOptions;
use tsc_core::scriptkind::ScriptKind;
use tsc_core::tristate::Tristate;
use tsc_parser::parse_source_file;
use tsc_tspath::RootedFilePath;

// ── The single-file Program scaffold (the fcs21 idiom, TS flavor) ──

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

/// Check `src` (a TypeScript unit, no libs — the corpus seam) with
/// noImplicitAny (the conformance harness configuration the twin drivers
/// run; the pre-fix port reported the TS7006 extra under exactly this
/// configuration).
fn check_ts(src: &str) -> (Checker, tsc_ast::NodeId) {
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
    checker.check_source_file(file, true);
    (checker, file)
}

/// The (code, args) list of the file's diagnostics, in order.
fn diags(c: &mut Checker, file: tsc_ast::NodeId) -> Vec<(i32, Vec<String>)> {
    c.get_file_diagnostics(file)
        .iter()
        .map(|d| (d.code(), d.message_args().to_vec()))
        .collect()
}

// ── Pin 1: the computedPropertyNamesContextualType1 shape — a computed-name
// METHOD contextually typed by the interface's STRING index signature; its
// parameter types as string (the index signature's parameter), so the body's
// `y.length` reports 2339 and the parameter is NOT an implicit any ──

const STRING_INDEX_METHOD: &str = "interface I {\n\
    [s: string]: (x: string) => number;\n\
    [s: number]: (x: any) => number; // Doesn't get hit\n\
}\n\
\n\
var o: I = {\n\
    [\"\" + 0](y) { return y.length; },\n\
    [\"\" + 1]: y => y.length\n\
}\n";

#[test]
fn computed_name_method_contextual_signature_from_string_index_pin() {
    // Pre-fix: the method's `y` resolved to implicit any (an extra TS7006)
    // and its `y.length` never reported 2339; only the arrow-function half
    // of the record matched the Go twin. Post-fix (the generated Name()
    // family at both element-impl name reads): the method's contextual
    // type is the string index signature's `(x: string) => number`, so
    // BOTH `y.length` accesses report 2339 ('length' on 'string' — the
    // no-libs corpus seam) and no TS7006 fires. Expectations verified
    // byte-same against the go-bench twin (rig probe
    // computedPropertyNamesContextualType1_ES5, PROBES-IDENTICAL).
    let (mut c, file) = check_ts(STRING_INDEX_METHOD);
    let ds = diags(&mut c, file);
    let e2339: Vec<&Vec<String>> = ds
        .iter()
        .filter(|(code, _)| *code == 2339)
        .map(|(_, args)| args)
        .collect();
    assert_eq!(e2339.len(), 2, "the method's AND the arrow's y.length: {ds:?}");
    for args in &e2339 {
        assert_eq!(args[0], "length", "the property access: {ds:?}");
        assert_eq!(args[1], "string", "the contextually typed parameter: {ds:?}");
    }
    assert!(
        !ds.iter().any(|(code, _)| *code == 7006),
        "no implicit-any on the method's parameter: {ds:?}"
    );
}

// ── Pin 2: the computedPropertyNamesContextualType2 shape — the computed
// name is NUMERIC (`+"foo"`), so the NUMBER index signature applies and the
// method's parameter types as its `x: string` ──

const NUMBER_INDEX_METHOD: &str = "interface I {\n\
    [s: string]: (x: any) => number; // Doesn't get hit\n\
    [s: number]: (x: string) => number;\n\
}\n\
\n\
var o: I = {\n\
    [+\"foo\"](y) { return y.length; },\n\
    [+\"bar\"]: y => y.length\n\
}\n";

#[test]
fn computed_name_method_contextual_signature_from_number_index_pin() {
    // The numeric computed name `+"foo"` (regular type number) is
    // applicable to the `[s: number]` index signature, so the method's
    // contextual signature is `(x: string) => number` — `y` types as
    // string and both `y.length` accesses report 2339, with no TS7006.
    // (Verified against the go-bench twin — case 2, both records.)
    let (mut c, file) = check_ts(NUMBER_INDEX_METHOD);
    let ds = diags(&mut c, file);
    let e2339: Vec<&Vec<String>> = ds
        .iter()
        .filter(|(code, _)| *code == 2339)
        .map(|(_, args)| args)
        .collect();
    assert_eq!(e2339.len(), 2, "the method's AND the arrow's y.length: {ds:?}");
    for args in &e2339 {
        assert_eq!(args[0], "length", "{ds:?}");
        assert_eq!(args[1], "string", "{ds:?}");
    }
    assert!(
        !ds.iter().any(|(code, _)| *code == 7006),
        "no implicit-any on the method's parameter: {ds:?}"
    );
}

// ── Pin 3: the computed-name method WITHOUT a contextual type still falls
// to the implicit-any report — the fix must not over-apply ──

const NO_INDEX_METHOD: &str = "interface I {\n\
    m: (x: string) => number;\n\
}\n\
\n\
var o: I = {\n\
    [\"\" + 0](y) { return y.length; }\n\
}\n";

#[test]
fn computed_name_method_without_index_signature_keeps_implicit_any_pin() {
    // `I` has no index signature and no property matching the computed
    // name, so the method has NO contextual type: `y` stays an implicit any
    // (the TS7006 fires under noImplicitAny) and `y.length` on any does
    // NOT report 2339 — the pre-existing behavior, unchanged by the fix
    // (the contextual-type lookup returns nil through the same Go arms).
    let (mut c, file) = check_ts(NO_INDEX_METHOD);
    let ds = diags(&mut c, file);
    assert!(
        !ds.iter().any(|(code, _)| *code == 2339),
        "any-typed y.length stays silent: {ds:?}"
    );
    assert!(
        ds.iter().any(|(code, args)| *code == 7006 && args.first() == Some(&"y".to_string())),
        "the un-contextually-typed parameter still reports 7006: {ds:?}"
    );
}

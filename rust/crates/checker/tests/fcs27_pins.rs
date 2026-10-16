//! FCS27 pins (TS2699 single-record family, tranche 27): the
//! rust-extra-2699 x96 census family — every instance in ONE corpus record,
//! classes/propertyMemberDeclarations/staticPropertyNameConflicts.ts
//! (Go 144 / rust 240 across the `@target: es5, es2015` +
//! `@useDefineForClassFields: true,false` 4-config matrix).
//!
//! ROOTS fixed this tranche (both "FCS tranche 27" PORT notes in-tree,
//! statements.rs):
//!
//! 1. THE 96: `checkClassForStaticPropertyNameConflicts` (checker.go:4466)
//!    gates on `c.compilerOptions.GetUseDefineForClassFields()` — the
//!    name/length/caller/arguments TS2699 arm is SILENCED for the
//!    useDefineForClassFields=true configs. The port gated on the
//!    permissive `emit_standard_class_fields` seam stub (always false at
//!    es5/es2015 targets), so rust emitted in ALL four configs — 48 member
//!    positions x 2 extra configs = the 96 rust-extra 2699s. Now gated on
//!    the Go-exact `get_use_define_for_class_fields` twin
//!    (compileroptions.go:170: TSUnknown -> target >= ES2022).
//!
//! 2. The prototype TS2699's second argument (same record, hash tail):
//!    checker.go:3237 passes `c.symbolToString(c.getSymbolOfDeclaration(node))`
//!    — the NODEBUILDER symbol-chain print. The port passed the RAW
//!    program-symbol name, which is the binder fallback "class" for
//!    anonymous class expressions and "default" for export-default classes;
//!    Go resolves those through the symbol chain to the assigned variable /
//!    class name. statements.rs now routes through the REAL
//!    `relater::symbol_to_string` nodebuilder twin (the sibling
//!    name/length/caller/arguments site already did).
//!
//! Same-record tail NOT fixed this tranche (reported honestly): Go's 3x
//! TS2300 "Duplicate identifier '[FunctionPropertyNames.prototype]'"
//! positions (static + instance computed `prototype` pairs, 12 diags) — a
//! binder-side computed-name collision family, not the 2699 gate.
//!
//! Expectations verified byte-same against the go-bench twin's check-stage
//! dumps (rig probes f27p1..f27p3 + the full corpus record, both drivers,
//! DIFF 0 on 2699) — pins run WITHOUT libs (the corpus twin seam).

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::compileroptions::ResolutionModeNone;
use tsc_core::options_generated::{CompilerOptions, ScriptTarget};
use tsc_core::scriptkind::ScriptKind;
use tsc_core::tristate::Tristate;
use tsc_parser::parse_source_file;
use tsc_tspath::RootedFilePath;

// ── The single-file Program scaffold (the fcs23+ idiom) ──

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

fn check_with(src: &str, options: CompilerOptions) -> (Checker, tsc_ast::NodeId) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
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

/// es5-target strict options with the given useDefineForClassFields
/// tristate (the corpus matrix axis).
fn es5_options(udcff: Tristate) -> CompilerOptions {
    CompilerOptions {
        strict: Tristate::TSTrue,
        target: ScriptTarget::ES5,
        use_define_for_class_fields: udcff,
        ..CompilerOptions::default()
    }
}

/// The (code, args) list of the file's diagnostics, in order.
fn diags(c: &mut Checker, file: tsc_ast::NodeId) -> Vec<(i32, Vec<String>)> {
    c.get_file_diagnostics(file)
        .iter()
        .map(|d| (d.code(), d.message_args().to_vec()))
        .collect()
}

fn codes(ds: &[(i32, Vec<String>)], code: i32) -> Vec<Vec<String>> {
    ds.iter()
        .filter(|(c, _)| *c == code)
        .map(|(_, args)| args.clone())
        .collect()
}

// ── Pin 1: the useDefineForClassFields gate — the +96 family ──
//
// `static name` reports TS2699 ("name", "C") ONLY when
// getUseDefineForClassFields() is false: explicitly false at es5, or
// TSUnknown with a pre-ES2022 target. The true config and the
// TSUnknown-at-es2022+ default are SILENT (Go checker.go:4468).

const STATIC_NAME_CLASS: &str = "class C {\n    static name: number;\n    name: string;\n}\n";

#[test]
fn static_name_conflicts_2699_udcff_gate_pin() {
    // udcff = false -> exactly one 2699, Go-identical args.
    let (mut c, f) = check_with(STATIC_NAME_CLASS, es5_options(Tristate::TSFalse));
    let ds = diags(&mut c, f);
    let a = codes(&ds, 2699);
    assert_eq!(a, vec![vec!["name".to_string(), "C".to_string()]], "{ds:?}");

    // udcff = true -> the whole arm is gated OFF (pre-fix: still emitted).
    let (mut c, f) = check_with(STATIC_NAME_CLASS, es5_options(Tristate::TSTrue));
    let ds = diags(&mut c, f);
    assert!(codes(&ds, 2699).is_empty(), "{ds:?}");

    // TSUnknown -> target-based default: es5 keeps the arm, es2022+ does not.
    let (mut c, f) = check_with(STATIC_NAME_CLASS, es5_options(Tristate::TSUnknown));
    let ds = diags(&mut c, f);
    let a = codes(&ds, 2699);
    assert_eq!(a, vec![vec!["name".to_string(), "C".to_string()]], "{ds:?}");
    let (mut c, f) = check_with(
        STATIC_NAME_CLASS,
        CompilerOptions {
            strict: Tristate::TSTrue,
            use_define_for_class_fields: Tristate::TSUnknown,
            ..CompilerOptions::default()
        },
    );
    let ds = diags(&mut c, f);
    assert!(codes(&ds, 2699).is_empty(), "{ds:?}");
}

// ── Pin 2: the computed-name axis of the same record —
// `[FunctionPropertyNames.name]` (a const literal) resolves through
// getEffectivePropertyNameForPropertyNameNode and hits the same gate ──

const COMPUTED_NAME_CLASS: &str = "const FunctionPropertyNames = {\n    name: 'name',\n} as const;\n\
class C {\n    static [FunctionPropertyNames.name]: number;\n    [FunctionPropertyNames.name]: number;\n}\n";

#[test]
fn static_computed_name_conflicts_2699_pin() {
    let (mut c, f) = check_with(COMPUTED_NAME_CLASS, es5_options(Tristate::TSFalse));
    let ds = diags(&mut c, f);
    let a = codes(&ds, 2699);
    assert_eq!(a, vec![vec!["name".to_string(), "C".to_string()]], "{ds:?}");

    let (mut c, f) = check_with(COMPUTED_NAME_CLASS, es5_options(Tristate::TSTrue));
    let ds = diags(&mut c, f);
    assert!(codes(&ds, 2699).is_empty(), "{ds:?}");
}

// ── Pin 3: prototype TS2699 is NOT udcff-gated ("always an error") and its
// second argument is the NODEBUILDER symbol-chain print — the assigned
// variable name for anonymous class expressions (pre-fix: raw "class") ──

const ANON_PROTOTYPE: &str =
    "var StaticPrototype_Anonymous = class {\n    static prototype: number;\n};\n";

#[test]
fn static_prototype_anonymous_class_2699_arg_pin() {
    // prototype 2699 fires for BOTH udcff configs.
    for udcff in [Tristate::TSTrue, Tristate::TSFalse] {
        let (mut c, f) = check_with(ANON_PROTOTYPE, es5_options(udcff));
        let ds = diags(&mut c, f);
        let a = codes(&ds, 2699);
        assert_eq!(
            a,
            vec![vec!["prototype".to_string(), "StaticPrototype_Anonymous".to_string()]],
            "udcff={udcff:?} {ds:?}"
        );
    }
}

// ── Pin 4: export-default class inside a namespace — raw binder name
// "default" resolves through the symbol chain to the class name ──

const DEFAULT_PROTOTYPE: &str =
    "namespace N { export default class StaticPrototype { static prototype: number; } }\n";

#[test]
fn static_prototype_export_default_class_2699_arg_pin() {
    let (mut c, f) = check_with(DEFAULT_PROTOTYPE, es5_options(Tristate::TSFalse));
    let ds = diags(&mut c, f);
    let a = codes(&ds, 2699);
    assert_eq!(
        a,
        vec![vec!["prototype".to_string(), "StaticPrototype".to_string()]],
        "{ds:?}"
    );
}

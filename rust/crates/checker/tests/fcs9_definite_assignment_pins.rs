//! FCS9 pins (full-corpus check sweep, tranche 9): the TS2564
//! strictPropertyInitialization family — the definite-assignment postfix
//! `!`, the containsUndefinedType gate, and the full
//! isPropertyInitializedInConstructor flow walk.
//!
//! Three root causes, all Go-exact ports (every pin's expectation verified
//! byte-same against the go-bench twin's check-stage dump over the same
//! source shape — $HOME/.fcs9-scratch/probe2564/k1.ts on the rig):
//!
//! 1. `isPropertyWithoutInitializer` (Go checker.go:5081 +
//!    utilities.go's isExclamationToken) — the W3 TODO stub ignored the
//!    definite-assignment assertion `x!: T`, so every `!`-asserted property
//!    reported a bogus TS2564.
//! 2. `checkPropertyInitialization`'s containsUndefinedType gate (Go
//!    checker.go:5101 — a union contains undefined iff its FIRST
//!    constituent does) — optional-undefined types skip the check; the W3
//!    stub was permissive false.
//! 3. `isPropertyInitializedInConstructor` (Go checker.go:5085) — the W3
//!    stub answered "initialized" whenever ANY constructor exists, so no
//!    conditional-assignment error ever fired (the 107 MISSING-side
//!    corpus records). The real port synthesizes the `this.<name>`
//!    reference (NodeStore::alloc seam, the new_this_property_access_synthetic
//!    idiom), wires it to the constructor's ReturnFlowNode, and runs the
//!    real getFlowTypeOfReferenceEx walk (flow.rs).

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::options_generated::CompilerOptions;
use tsc_core::scriptkind::ScriptKind;
use tsc_core::tristate::Tristate;
use tsc_parser::parse_source_file;
use tsc_tspath::RootedFilePath;

// ── The single-file Program scaffold (the fcs5/fcs6/fcs8 idiom) ──

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

/// Check `src` with strict flags incl. strictPropertyInitialization (the
/// conformance default for this family's shapes).
fn check_strict(src: &str) -> (Checker, tsc_ast::NodeId) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let options = CompilerOptions {
        no_implicit_any: Tristate::TSTrue,
        no_implicit_this: Tristate::TSTrue,
        strict_null_checks: Tristate::TSTrue,
        strict_property_initialization: Tristate::TSTrue,
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

/// The property names that reported TS2564, in order.
fn prop_names_with_2564(c: &mut Checker, file: tsc_ast::NodeId) -> Vec<String> {
    c.get_file_diagnostics(file)
        .iter()
        .filter(|d| d.code() == 2564)
        .map(|d| d.message_args().first().cloned().unwrap_or_default())
        .collect()
}

const PIN_SOURCE: &str = "class Definite { a!: string; b?: number; }\n\
     class Ok { c: string; constructor() { this.c = \"x\"; } }\n\
     class NotInit { d: string; }\n\
     class CondInit { e: string; constructor(f: boolean) { if (f) { this.e = \"x\"; } } }\n\
     class CondInit2 { g: string; constructor(f: boolean) { if (f) { this.g = \"x\"; } else { this.g = \"y\"; } } }\n\
     class UndefType { h: string | undefined; }\n";

// ── The pins ───────────────────────────────────────────────────────────────

/// Pin 1 — the twin's exact answer for the whole battery: ONLY the
/// no-initializer/no-constructor `d` and the one-branch-conditional `e`
/// report TS2564 (probe2564/k1.ts — byte-same against the go-bench twin).
#[test]
fn definite_assignment_battery_pin() {
    let (mut c, file) = check_strict(PIN_SOURCE);
    let names = prop_names_with_2564(&mut c, file);
    assert_eq!(
        names,
        vec!["d".to_string(), "e".to_string()],
        "only d (no ctor) and e (conditionally assigned) may report TS2564"
    );
}

/// Pin 2 — the definite-assignment assertion `a!: string` and the optional
/// `b?: number` never report TS2564 (the postfix-token seam).
#[test]
fn definite_assignment_assertion_pin() {
    let (mut c, file) = check_strict("class A { a!: string; b?: number; }\n");
    let names = prop_names_with_2564(&mut c, file);
    assert!(
        names.is_empty(),
        "the `!`-asserted and optional properties must not report TS2564 (got {names:?})"
    );
}

/// Pin 3 — the constructor flow walk: `this.g = ...` on BOTH branches is
/// definitely assigned (no error), a single-branch assignment is not (the
/// isPropertyInitializedInConstructor walk over the constructor's
/// ReturnFlowNode), and a `string | undefined` property skips the check
/// (the containsUndefinedType gate).
#[test]
fn constructor_flow_definite_assignment_pin() {
    let (mut c, file) = check_strict(
        "class Both { g: string; constructor(f: boolean) { if (f) { this.g = \"x\"; } else { this.g = \"y\"; } } }\n\
         class Undef { h: string | undefined; }\n",
    );
    let names = prop_names_with_2564(&mut c, file);
    assert!(
        names.is_empty(),
        "both-branch assignment + undefined-typed property must not report TS2564 (got {names:?})"
    );
}

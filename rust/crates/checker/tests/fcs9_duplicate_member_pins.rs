//! FCS9 pins (full-corpus check sweep, tranche 9): the duplicate-member
//! TS2300 family — `checkObjectTypeForDuplicateDeclarations`'
//! `checkPropertyOrAccessor` closure.
//!
//! ROOT (verified byte-same against the go-bench twin's check-stage dump
//! over the same source shape — $HOME/.fcs9-scratch/probe2300/k1.ts on the
//! rig): Go's closure (checker.go:3197) reports
//! `reportDuplicateMemberErrors(node, symbol.Name, true, isStatic,
//! Duplicate_identifier_0)` on the second property / property+accessor
//! combination and records state 3. The W-port stub recorded state 3
//! WITHOUT reporting — the whole duplicate-member TS2300 family never
//! fired (the 125-record missing side: duplicatePropertyNames 14x,
//! privateNameDuplicateField 18x, staticPropertyNameConflicts 12x,
//! numericNamedPropertyDuplicates 8x, ...).
//!
//! Residuals on the family (NOT this root; handoff notes): the
//! `[FunctionPropertyNames.prototype]` computed-name static duplicates
//! (a computed-name symbol-merge seam) and the private-identifier
//! symbolToString arg rendering (`A#foo` vs Go's `#foo`).

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

fn check(src: &str) -> (Checker, tsc_ast::NodeId) {
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

/// The member names that reported TS2300, in order.
fn names_with_2300(c: &mut Checker, file: tsc_ast::NodeId) -> Vec<String> {
    c.get_file_diagnostics(file)
        .iter()
        .filter(|d| d.code() == 2300)
        .map(|d| d.message_args().first().cloned().unwrap_or_default())
        .collect()
}

// ── The pins ───────────────────────────────────────────────────────────────

/// Pin 1 — the twin's exact answer over the battery: duplicate properties
/// in an interface, a class, a type literal, a property+accessor
/// combination, and duplicate constructor parameter properties ALL report
/// TS2300 (probe2300/k1.ts — byte-same against the go-bench twin).
#[test]
fn duplicate_member_battery_pin() {
    let (mut c, file) = check(
        "interface I { a: string; a: number; }\n\
         class C { x: number; x: string; }\n\
         class D { y: string; get y(): string { return \"s\"; } }\n\
         class E { constructor(public z: number, public w: string, public w: number) {} }\n\
         type T = { b: string; b: number };\n",
    );
    let names = names_with_2300(&mut c, file);
    assert_eq!(
        names,
        vec![
            "a", "a", "x", "x", "y", "y", "w", "w", "b", "b",
        ]
        .into_iter()
        .map(String::from)
        .collect::<Vec<_>>(),
        "every duplicate member pair must report TS2300 (got {names:?})"
    );
}

/// Pin 2 — a single well-formed property never reports TS2300 (the state
/// machine records kind 1 and stops).
#[test]
fn single_property_no_duplicate_pin() {
    let (mut c, file) = check("class C { x: number; constructor(public z: number) {} }\n");
    let names = names_with_2300(&mut c, file);
    assert!(names.is_empty(), "no duplicate members, no TS2300 (got {names:?})");
}

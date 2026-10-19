//! FCS63 pins (full-corpus check sweep, micro-tranche 62 — the binding-
//! pattern-initializer + computed-name families, worked directly by the
//! orchestrator): the tranche's roots —
//!
//!  * `is_invalid_computed_property_name` (expressions.rs) read the
//!    ComputedPropertyName NODE's own kind instead of its EXPRESSION's,
//!    so the `[x in Y]`-in-a-type-literal / class-like / interface guard
//!    (Go checker.go:27330) was always false and every `[x in Y]` member
//!    re-checked its expression (the TS2464 + TS2304 + TS2693 trio
//!    rust-extra, x12 in mappedTypeProperties).
//!
//!  * `pad_object_literal_type` (typeofsymbol.rs) seeded the padded
//!    member table EMPTY instead of with the initializer type's existing
//!    properties (Go checker.go:17225), so `{ x, y = 0 } = { x: 0 }`
//!    typed the parameter as `{ y?: number }` instead of
//!    `{ x?: number; y?: number }` (the TS2353/TS2339 x-on-`{ y?: number }`
//!    extras at f5's call sites).
//!
//!  * `pad_tuple_type` (typeofsymbol.rs) was the permissive no-padding
//!    arm; implemented Go checker.go:17251 (the variable-flags/arity
//!    guard; elementTypes/elementInfos cloned from the target and padded
//!    with anyType-or-binding-element types as ElementFlagsOptional;
//!    reportImplicitAny for defaulted-less non-omitted elements;
//!    createTupleTypeEx with the target's readonly), so
//!    `[x, y = 0] = [0]` typed the parameter as `[number, number]`
//!    instead of `[number]` (the TS2345 extras at g4/g5's call sites).
//!
//! ("FCS tranche 62" PORT notes in-tree; pins named fcs63 by census epoch.)

use tsc_ast::{NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::compileroptions::ResolutionModeNone;
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
            implied_node_format: ResolutionModeNone,
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

fn check_with(src: &str, options: CompilerOptions) -> (Checker, tsc_ast::NodeId) {
    let mut sf = parse_source_file(Default::default(), src, ScriptKind::TS);
    let file = sf.as_node_id();
    let bind = bind_source_file(&mut sf).expect("bind produced a result");
    let binder_symbol_count = bind.symbol_count as u32;
    let program = TestProgram {
        options,
        name: RootedFilePath::from("/x/fcs63_unit.ts".to_string()),
        source_file: sf,
        bind,
        binder_symbol_count,
        checker_symbols: Vec::new(),
    };
    let mut checker = new_checker(Box::new(program));
    checker.check_source_file(file, true);
    (checker, file)
}

fn check_strict(src: &str) -> (Checker, tsc_ast::NodeId) {
    check_with(
        src,
        CompilerOptions {
            target: ScriptTarget::ESNext,
            strict: Tristate::TSTrue,
            ..CompilerOptions::default()
        },
    )
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

// ── CLUSTER A — the `[x in Y]` computed-name guard family ──

/// The mappedTypeProperties Before/I/C/D shape: `[placeType in PlaceType]`
/// members of a type literal / class / interface are silently invalid
/// (errorType, NO diagnostics — the isInvalidComputedPropertyName guard),
/// not re-checked as computed-name expressions (no TS2464/TS2304/TS2693).
#[test]
fn fcs63_in_computed_name_members_silent_pin() {
    let (mut c, file) = check_strict(
        "type PlaceType = 'openSky' | 'roofed' | 'garage';\ntype Before = {\n    model: 'hour' | 'day';\n    [placeType in PlaceType]: void;\n}\ninterface I {\n    [P in PlaceType]: any\n}\nclass C {\n    [P in PlaceType]: any\n}\n",
    );
    let ds = diags(&mut c, file);
    assert!(
        codes(&ds, 2464).is_empty() && codes(&ds, 2304).is_empty() && codes(&ds, 2693).is_empty(),
        "the in-style computed names in type literals / interface / class are silently invalid: {ds:?}"
    );
}

// ── CLUSTER B — the binding-pattern-initializer padding family ──

/// The destructuringWithLiteralInitializers f5 shape: the parameter
/// `{ x, y = 0 } = { x: 0 }` types as the initializer's members PLUS the
/// pattern's missing defaulted member — `arg?: { x?: number; y?: number }`
/// — so `f5({ x: 1 })` checks with NO TS2353/TS2339 on `x`.
#[test]
fn fcs63_object_pattern_initializer_padded_pin() {
    let (mut c, file) = check_strict(
        "function f5({ x, y = 0 } = { x: 0 }) { }\nf5();\nf5({ x: 1 });\nf5({ x: 1, y: 1 });\n",
    );
    let ds = diags(&mut c, file);
    assert!(
        codes(&ds, 2353).is_empty() && codes(&ds, 2339).is_empty(),
        "the object-pattern initializer's type keeps the initializer's members: {ds:?}"
    );
}

/// The destructuringWithLiteralInitializers g4/g5 shape: the parameters
/// `[x, y = 0] = [0]` and `[x = 0, y = 0] = []` type as the initializer's
/// tuple PADDED to the pattern's arity — `arg?: [number, number]` — so
/// the `g4([1, 1])` / `g5([1, 1])` calls check with NO TS2345.
#[test]
fn fcs63_array_pattern_initializer_padded_pin() {
    let (mut c, file) = check_strict(
        "function g4([x, y = 0] = [0]) { }\ng4();\ng4([1, 1]);\nfunction g5([x = 0, y = 0] = []) { }\ng5();\ng5([1, 1]);\n",
    );
    let ds = diags(&mut c, file);
    assert!(
        codes(&ds, 2345).is_empty(),
        "the array-pattern initializer's tuple pads out to the pattern's arity: {ds:?}"
    );
}

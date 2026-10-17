//! FCS42 pins (full-corpus check sweep, tranche 42): the top-3 census
//! clusters of the fresh t41-final-dump census — TS2790 rust-missing
//! x25/6rec (the delete-operator optional-operand family), TS2364
//! rust-missing x23/1rec (the compound-assignment left-hand-side
//! family, compoundAssignmentLHSIsValue.ts), TS2536 rust-extra x28/12rec
//! ("Type '0' cannot be used to index type '1'", the remapping
//! mapped-type as-clause family).
//! ("FCS tranche 42" PORT notes in-tree.)
//!
//! THREE roots, all Go-exact:
//!
//! ROOT A — TS2790: `checkPropertyAccessExpressionOrQualifiedName` (Go
//! checker.go:11610) writes the resolved-symbol link
//! `c.symbolNodeLinks.Get(node).resolvedSymbol = prop` for every
//! resolved property. The W-era port omitted the write, so
//! `getResolvedSymbolOrNil` answered nil for EVERY property access —
//! `checkDeleteExpression` (checker.go:11056) then skipped
//! `checkDeleteExpressionMustBeOptional` entirely (TS2790 never fired
//! for `delete x.y` / `delete x?.y` shapes; only element accesses fired
//! via index.rs's own link write).
//!
//! ROOT B — TS2364: the `+` arm of checkBinaryLikeExpression gates its
//! early return on `resultType != nil &&
//! !c.checkForDisallowedESSymbolOperand(...)` (Go checker.go:12690) —
//! return early only when the symbol check REPORTED an error. The port
//! had the polarity INVERTED (`&&` instead of `!...`), so every `+=`
//! with a passing symbol check returned before reaching
//! `checkAssignmentOperator` — all `+=` TS2364s (this/null/literal
//! left-hand sides) were missing corpus-wide.
//!
//! ROOT C — TS2536: `checkIndexedAccessIndexType` (Go
//! checker.go:8433-8441) reads the mapped type's OWN index type
//! (`getIndexTypeForMappedType`) when the object type is a generic
//! mapped type with a REMAPPING as-clause, else `getIndexTypeEx`. The
//! port always took `getIndexTypeEx` (the TODO(port M6 S7) seam), so
//! `{ [K in keyof T as ...]: ... }` object types answered `keyof T` and
//! every remapped key failed the assignability probe — TS2536 fired
//! rust-extra on the mapped as-clause family.

use tsc_ast::{NodeStore, SourceFile, SourceFileParseOptions, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::options_generated::{CompilerOptions, ModuleKind, ScriptTarget};
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
    emit_format: ModuleKind,
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
        self.emit_format
    }
}

/// The check twin's unit parse (the fcs41 idiom).
fn check_unit(
    src: &str,
    name: &str,
    options: CompilerOptions,
    emit_format: ModuleKind,
) -> (Checker, tsc_ast::NodeId) {
    let file_name = RootedFilePath::from(name);
    let mut sf = parse_source_file(
        SourceFileParseOptions {
            file_name,
            ..Default::default()
        },
        src,
        ScriptKind::TS,
    );
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
        emit_format,
    };
    let mut checker = new_checker(Box::new(program));
    checker.check_source_file(file, true);
    (checker, file)
}

/// The (code, args) list of the checker's WHOLE semantic collection.
fn diags(c: &mut Checker) -> Vec<(i32, Vec<String>)> {
    c.semantic_diagnostics()
        .iter()
        .map(|d| (d.code(), d.message_args().to_vec()))
        .collect()
}

fn count(ds: &[(i32, Vec<String>)], code: i32) -> usize {
    ds.iter().filter(|(c, _)| *c == code).count()
}

// ── ROOT A — TS2790 (the delete-operator optional-operand family) ──

/// The deleteChain.ts shape (ROOT A): `delete o1?.b` on a non-optional
/// property under strictNullChecks reports exactly one TS2790 — the
/// resolved-symbol link now answers `b`, so
/// checkDeleteExpressionMustBeOptional runs and `string` has no
/// undefined fact. The pre-tranche port fired ZERO (the link was never
/// written, so getResolvedSymbolOrNil answered nil and the whole
/// optional-operand branch was skipped).
#[test]
fn fcs42_delete_optional_chain_reports_2790_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::CommonJS,
        strict: Tristate::TSTrue,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "declare const o1: undefined | { b: string };\ndelete o1?.b;\n",
        "/x/deleteChain.ts",
        options,
        ModuleKind::CommonJS,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 2790), 1, "expected one TS2790, got: {ds:?}");
}

/// The control (ROOT A): `delete p.b` where `b` is OPTIONAL (its type
/// includes undefined) reports NO TS2790 — hasTypeFacts(string |
/// undefined, IsUndefined) holds.
#[test]
fn fcs42_delete_optional_property_no_2790_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::CommonJS,
        strict: Tristate::TSTrue,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "declare const p: { b?: string };\ndelete p.b;\n",
        "/x/deleteOptional.ts",
        options,
        ModuleKind::CommonJS,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 2790), 0, "unexpected TS2790, got: {ds:?}");
}

// ── ROOT B — TS2364 (the compound-assignment left-hand-side family) ──

/// The compoundAssignmentLHSIsValue.ts shape (ROOT B): `null += value`
/// (a `+=` whose left-hand side is not a variable or property access)
/// reports exactly one TS2364 — the `+` arm now reaches
/// checkAssignmentOperator. The pre-tranche port returned early at the
/// inverted symbol-operand gate and fired ZERO `+=` TS2364s.
#[test]
fn fcs42_plus_equals_non_reference_lhs_2364_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::CommonJS,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "var value: any;\nnull += value;\n",
        "/x/compoundAssignmentLHSIsValue.ts",
        options,
        ModuleKind::CommonJS,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 2364), 1, "expected one TS2364, got: {ds:?}");
}

/// The control (ROOT B): an identifier left-hand side IS a reference —
/// `x += 1` reports NO TS2364.
#[test]
fn fcs42_plus_equals_identifier_lhs_no_2364_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::CommonJS,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "var x: number;\nx += 1;\n",
        "/x/compoundAssignmentLHSIsReference.ts",
        options,
        ModuleKind::CommonJS,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 2364), 0, "unexpected TS2364, got: {ds:?}");
}

// ── ROOT C — TS2536 (the remapping mapped-type as-clause family) ──

/// The mappedTypeAsClauses.ts / templateLiteralTypes1.ts shape (ROOT C):
/// an element access `x[k]` inside the generic function resolves to a
/// deferred indexed access over a generic mapped type with a REMAPPING
/// as-clause — `Getters<T>[k]`. checkIndexedAccessIndexType reads the
/// mapped type's OWN index type (the renamed `get${...}` keys), so `k`
/// (constrained to the renamed shape) IS assignable: NO TS2536. The
/// pre-tranche port answered `keyof T` (the unconditioned getIndexTypeEx
/// seam + the identity getNormalizedType stub), so every remapped key
/// failed the probe and TS2536 fired rust-extra.
#[test]
fn fcs42_remap_mapped_type_key_no_2536_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::CommonJS,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "type Getters<T> = { [P in keyof T & string as `get${P}`]: () => T[P] };\nfunction f<T>(x: Getters<T>, k: `get${string & keyof T}`) { return x[k]; }\n",
        "/x/mappedTypeAsClauses.ts",
        options,
        ModuleKind::CommonJS,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 2536), 0, "unexpected TS2536, got: {ds:?}");
}

//! FCS44 pins (full-corpus check sweep, tranche 44): the top-3 coherent
//! clusters of the fresh t43-final-dump census — TS7053 rust-MISSING
//! x42/13rec (the indexSignatures1 template/union index family), TS1361
//! rust-MISSING x18/7rec (the externalModules/typeOnly "imported using
//! import type" family), TS1470 rust-MISSING x16/8rec (the
//! nodeModulesImportMeta import.meta CommonJS-output family).
//! ("FCS tranche 44" PORT notes in-tree.)
//!
//! THREE roots, all Go-exact:
//!
//! ROOT A — TS7053: Go's noImplicitAny index ladder ends with an
//! UNCONDITIONAL `NewDiagnosticChainForNode(diagnostic, accessExpression,
//! Element_implicitly_has_an_any_type_because_expression_of_type_0_can_t_
//! be_used_to_index_type_1, ...)` (checker.go:27713): when the switch
//! above matched none of its arms (a template-literal / literal-union /
//! `keyof`-shaped index type — no EnumLiteral, UniqueESSymbol,
//! StringLiteral/NumberLiteral, or plain Number/String flags), the
//! chain's nil-inner worker (utilities.go:31: `chain != nil` → chain,
//! else a plain diagnostic) still emits the 7053 HEAD ALONE. The port
//! gated the emission on `diagnostic.is_some()`, dropping exactly those
//! (all 42 corpus instances rode a nil-arm index type), and passed
//! `originalObjectType` where Go passes the apparent `objectType`.
//!
//! ROOT B — TS1361: Go's `onSuccessfullyResolvedSymbol` type-only-value
//! arm (checker.go:1902) gates on `!ast.IsValidTypeOnlyAliasUseSite(
//! errorLocation)` — the real predicate (ast/utilities.go:3200:
//! Ambient/JSDoc flags, IsPartOfTypeQuery,
//! isIdentifierInNonEmittingHeritageClause,
//! isPartOfPossiblyValidTypeOrAbstractComputedPropertyName,
//! `!(IsExpressionNode || isShorthandPropertyNameUseSite)`). The port
//! stubbed it permissive-true (the TODO(port M6 S6 remainder) seam),
//! keeping the "X cannot be used as a value because it was imported
//! using import type" arm dormant for every real value use site.
//!
//! ROOT C — TS1470: Go's `checkImportMetaProperty` (checker.go:11018)
//! reads `c.program.GetSourceFileMetaData(
//! GetSourceFileOfNode(node).PathKey()).ImpliedNodeFormat` and reports
//! TS1470 whenever it is not ESNext (module node16–nodenext). The port
//! had the TODO(port M6 S6) permissive skip; the read now goes through
//! the Program trait's `source_file_implied_node_format` seam.

use tsc_ast::{NodeStore, SourceFile, SourceFileParseOptions, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::options_generated::{CompilerOptions, ModuleKind, ScriptTarget};
use tsc_core::scriptkind::ScriptKind;
use tsc_core::tristate::Tristate;
use tsc_parser::parse_source_file;
use tsc_tspath::RootedFilePath;

// ── The single-file Program scaffold (the fcs43 idiom, + the source-file
// metadata's implied-node-format read the T44 ROOT C pin needs) ──

struct TestProgram {
    options: CompilerOptions,
    name: RootedFilePath,
    source_file: SourceFile,
    bind: BindResult,
    binder_symbol_count: u32,
    checker_symbols: Vec<Symbol>,
    emit_format: ModuleKind,
    implied_node_format: ModuleKind,
    /// Go: Program.GetSourceFileMetaData(path).ImpliedNodeFormat — the
    /// ROOT C read (the metadata's implied format for the unit).
    source_implied_node_format: ModuleKind,
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
    fn get_implied_node_format_for_emit(&self, _file: tsc_ast::NodeId) -> ModuleKind {
        self.implied_node_format
    }
    fn source_file_implied_node_format(&self, _file: tsc_ast::NodeId) -> ModuleKind {
        self.source_implied_node_format
    }
}

/// The check twin's unit parse (the fcs43 idiom, + the metadata implied
/// format the T44 clusters read).
#[allow(clippy::too_many_arguments)]
fn check_unit(
    src: &str,
    name: &str,
    options: CompilerOptions,
    emit_format: ModuleKind,
    implied_node_format: ModuleKind,
    source_implied_node_format: ModuleKind,
    script_kind: ScriptKind,
) -> (Checker, tsc_ast::NodeId) {
    let file_name = RootedFilePath::from(name);
    let mut sf = parse_source_file(
        SourceFileParseOptions {
            file_name,
            ..Default::default()
        },
        src,
        script_kind,
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
        implied_node_format,
        source_implied_node_format,
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

// ── ROOT A — TS7053 (the nil-arm index-type family) ──

/// The indexSignatures1/numericStringLiteralTypes shape (ROOT A): an
/// element access whose index type is a TEMPLATE-LITERAL type (matches
/// none of Go's switch arms — checker.go:27696-27709) on an object with
/// no index signature reports exactly one TS7053: Go's
/// `NewDiagnosticChainForNode(nil, ...)` (utilities.go:31) still emits
/// the 7053 head alone. The pre-tranche port gated on
/// `diagnostic.is_some()` and reported nothing.
#[test]
fn fcs44_template_index_nil_chain_7053_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::CommonJS,
        no_implicit_any: Tristate::TSTrue,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "declare let obj: { b: number };\ndeclare const k: `a${string}`;\nobj[k];\n",
        "/x/templateIndex.ts",
        options,
        ModuleKind::CommonJS,
        tsc_core::compileroptions::ResolutionModeCommonJS,
        tsc_core::compileroptions::ResolutionModeCommonJS,
        ScriptKind::TS,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 7053), 1, "expected one TS7053, got: {ds:?}");
}

/// The no-fire control (ROOT A): the same template-literal index against
/// a type WITH a matching template-pattern index signature resolves
/// through `getApplicableIndexInfo` and reports NO TS7053.
#[test]
fn fcs44_template_index_signature_no_7053_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::CommonJS,
        no_implicit_any: Tristate::TSTrue,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "declare let dom: { [x: `a${string}`]: number };\ndeclare const k: `a${string}`;\ndom[k];\n",
        "/x/templateIndexSig.ts",
        options,
        ModuleKind::CommonJS,
        tsc_core::compileroptions::ResolutionModeCommonJS,
        tsc_core::compileroptions::ResolutionModeCommonJS,
        ScriptKind::TS,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 7053), 0, "unexpected TS7053, got: {ds:?}");
}

/// The arm-matched regression control (ROOT A): a plain `string` index
/// on a type with no index signature matches the fourth switch arm
/// (`flags & (Number|String)`), so the chain carries the
/// `No_index_signature_with_a_parameter_of_type_0_was_found_on_type_1`
/// inner — TS7053 fires exactly once (the pre-tranche port already
/// fired this; it must keep firing).
#[test]
fn fcs44_string_index_still_7053_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::CommonJS,
        no_implicit_any: Tristate::TSTrue,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "declare let obj: { b: number };\ndeclare const k: string;\nobj[k];\n",
        "/x/stringIndex.ts",
        options,
        ModuleKind::CommonJS,
        tsc_core::compileroptions::ResolutionModeCommonJS,
        tsc_core::compileroptions::ResolutionModeCommonJS,
        ScriptKind::TS,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 7053), 1, "expected one TS7053, got: {ds:?}");
}

// ── ROOT B — TS1361 (the typeOnly-import value-use family) ──

/// The externalModules/typeOnly/importClause_namedImports.ts /d.ts shape
/// (ROOT B): a `import type { A } from ...` alias used as a VALUE
/// (`new A()`) at a non-type use site reports exactly one TS1361 — the
/// alias symbol carries Alias|Type (no Value) flags, the use site is a
/// plain expression (IsValidTypeOnlyAliasUseSite false on every leg),
/// and `getTypeOnlyAliasDeclarationEx` finds the type-only import
/// specifier. The pre-tranche port's permissive-true
/// `is_valid_type_only_alias_use_site` stub kept the arm dormant.
/// (The ambient module keeps the alias resolution in-memory.)
#[test]
fn fcs44_type_only_import_value_use_1361_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::CommonJS,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "declare module \"abc\" { export class A {} }\nimport type { A } from \"abc\";\nnew A();\n",
        "/x/typeOnlyValue.ts",
        options,
        ModuleKind::CommonJS,
        tsc_core::compileroptions::ResolutionModeCommonJS,
        tsc_core::compileroptions::ResolutionModeCommonJS,
        ScriptKind::TS,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 1361), 1, "expected one TS1361, got: {ds:?}");
}

/// The type-use control (ROOT B — the enums.ts /b.ts `declare let a: A`
/// arm): a type-only import used ONLY in a type position reports NO
/// TS1361 (`IsValidTypeOnlyAliasUseSite` answers true through
/// IsExpressionNode's non-expression legs).
#[test]
fn fcs44_type_only_import_type_use_no_1361_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::CommonJS,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "declare module \"abc\" { export class A {} }\nimport type { A } from \"abc\";\ndeclare let a: A;\na;\n",
        "/x/typeOnlyType.ts",
        options,
        ModuleKind::CommonJS,
        tsc_core::compileroptions::ResolutionModeCommonJS,
        tsc_core::compileroptions::ResolutionModeCommonJS,
        ScriptKind::TS,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 1361), 0, "unexpected TS1361, got: {ds:?}");
}

/// The plain-import control (ROOT B): a NON-type import used as a value
/// resolves to a symbol carrying Value flags — the
/// `result.Flags&Value == 0` leg fails and NO TS1361 fires.
#[test]
fn fcs44_value_import_no_1361_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::CommonJS,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "declare module \"abc\" { export class A {} }\nimport { A } from \"abc\";\nnew A();\n",
        "/x/valueImport.ts",
        options,
        ModuleKind::CommonJS,
        tsc_core::compileroptions::ResolutionModeCommonJS,
        tsc_core::compileroptions::ResolutionModeCommonJS,
        ScriptKind::TS,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 1361), 0, "unexpected TS1361, got: {ds:?}");
}

// ── ROOT C — TS1470 (the nodeModulesImportMeta import.meta family) ──

/// The nodeModulesImportMeta.ts subfolder/index.ts shape (ROOT C):
/// `import.meta` under module node16 in a file whose metadata implies
/// CommonJS reports exactly one TS1470
/// (`GetSourceFileMetaData(...).ImpliedNodeFormat != ESNext`). The
/// pre-tranche port had the permissive TODO(port M6 S6) skip.
#[test]
fn fcs44_import_meta_node16_cjs_1470_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2022,
        module: ModuleKind::Node16,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "const x = import.meta.url;\nexport { x };\n",
        "/x/subfolderIndex.ts",
        options,
        ModuleKind::Node16,
        tsc_core::compileroptions::ResolutionModeCommonJS,
        tsc_core::compileroptions::ResolutionModeCommonJS,
        ScriptKind::TS,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 1470), 1, "expected one TS1470, got: {ds:?}");
}

/// The ESM control (ROOT C — the nodeModulesImportMeta.ts root
/// index.ts, a "type": "module" package file): `import.meta` under
/// module node16 with an ESNext-implied metadata reports NO TS1470.
#[test]
fn fcs44_import_meta_node16_esm_no_1470_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2022,
        module: ModuleKind::NodeNext,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "const x = import.meta.url;\nexport { x };\n",
        "/x/index.ts",
        options,
        ModuleKind::NodeNext,
        tsc_core::compileroptions::ResolutionModeESM,
        tsc_core::compileroptions::ResolutionModeESM,
        ScriptKind::TS,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 1470), 0, "unexpected TS1470, got: {ds:?}");
    assert_eq!(count(&ds, 1343), 0, "unexpected TS1343, got: {ds:?}");
}

/// The pre-node16 regression control (ROOT C — the es2019 importMeta.ts
/// shape): under module es2015, `import.meta` keeps reporting the
/// module-option error TS1343 (NOT TS1470) — the second arm of
/// `checkImportMetaProperty` (checker.go:11023) is untouched.
#[test]
fn fcs44_import_meta_es2015_still_1343_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::ES2015,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "const x = import.meta.url;\nexport { x };\n",
        "/x/importMeta.ts",
        options,
        ModuleKind::ES2015,
        tsc_core::compileroptions::ResolutionModeCommonJS,
        tsc_core::compileroptions::ResolutionModeCommonJS,
        ScriptKind::TS,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 1470), 0, "unexpected TS1470, got: {ds:?}");
    assert_eq!(count(&ds, 1343), 1, "expected one TS1343, got: {ds:?}");
}

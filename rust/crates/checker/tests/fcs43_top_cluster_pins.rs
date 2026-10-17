//! FCS43 pins (full-corpus check sweep, tranche 43): the top-3 coherent
//! clusters of the fresh t42-final-dump census — TS2323 rust-extra x26/5rec
//! (the salsa moduleExportAlias computed-name family, queued since T11),
//! TS1203 rust-extra x25/15rec (the export= forbid-guard's per-file
//! implied-format family), TS1049/TS1054 rust-extra x20/4rec (the
//! thisTypeInAccessors accessor-`this`-parameter family).
//! ("FCS tranche 43" PORT notes in-tree.)
//!
//! THREE roots, all Go-exact:
//!
//! ROOT A — TS1049/TS1054: Go's `IsThisParameter`
//! (ast/utilities.go:1371) is TEXT-based — a parameter whose name is an
//! Identifier with text `"this"` (the parser builds `this` parameter
//! names as Identifier nodes, never `ThisKeyword`-kind names). The port
//! checked for a `ThisKeyword`-kind name, so the shim answered false for
//! EVERY real `this` parameter; `getAccessorThisParameter` (Go
//! checker.go:20335) then answered nil for `get x(this: Foo)` /
//! `set x(this: Foo, v)`, `doesAccessorHaveCorrectParameterCount`
//! (grammarchecks.go:1341) fell through to the plain arity check, and
//! TS1054/TS1049 fired rust-extra on every accessor declared with a
//! `this` parameter (thisTypeInAccessors.ts,
//! thisTypeInAccessorsNegative.ts).
//!
//! ROOT B — TS1203: Go's forbid-export= guard (checker.go:5860) reads
//! `c.program.GetImpliedNodeFormatForEmit(sourceFile)` — the PER-FILE
//! implied format (GetImpliedNodeFormatForEmitWorker over the file
//! name, the emit module kind, and the source-file metadata). The port
//! approximated it with the CONFIGURED module kind (the TODO(port M6
//! S?) seam), so every node16/nodenext file in a CommonJS package
//! (implied format CommonJS) and every non-ESNext-mode declaration file
//! (implied format None) misclassified and TS1203 fired rust-extra
//! (nodeModulesExportAssignments, verbatimModuleSyntaxRestrictionsESM,
//! bundlerSyntaxRestrictions, rewriteRelativeImportExtensions cjsErrors).
//!
//! ROOT C — TS2323: Go's `checkExternalModuleExports` export-* conflict
//! loop (checker.go:5916) skips a multi-declaration symbol when EVERY
//! declaration is a `GetAssignmentDeclarationKind ==
//! JSDeclarationKindExportsProperty` assignment (`exports.x = ...` /
//! `module.exports.x = ...`). The port hardcoded the
//! `TODO(port M6 S4)` permissive-false seam, so the salsa
//! moduleExportAlias family (symbols declared twice through repeated
//! `exports = module.exports = ... = {}` re-aliasing blocks) fired the
//! extra "Cannot redeclare exported variable" TS2323s.

use tsc_ast::{NodeStore, SourceFile, SourceFileParseOptions, Symbol, SymbolId};
use tsc_binder::{bind_source_file, BindResult};
use tsc_checker::{new_checker, Checker, Program};
use tsc_core::options_generated::{CompilerOptions, ModuleKind, ScriptTarget};
use tsc_core::scriptkind::ScriptKind;
use tsc_core::tristate::Tristate;
use tsc_parser::parse_source_file;
use tsc_tspath::RootedFilePath;

// ── The single-file Program scaffold (the fcs42 idiom) ──

struct TestProgram {
    options: CompilerOptions,
    name: RootedFilePath,
    source_file: SourceFile,
    bind: BindResult,
    binder_symbol_count: u32,
    checker_symbols: Vec<Symbol>,
    emit_format: ModuleKind,
    /// Go: Program.GetImpliedNodeFormatForEmit — the per-file implied
    /// format (the ROOT B read; the SkipModuleResolution worker answer
    /// for the synthetic unit).
    implied_node_format: ModuleKind,
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
}

/// The check twin's unit parse (the fcs42 idiom, + the script kind and the
/// implied-format inputs the T43 clusters read).
#[allow(clippy::too_many_arguments)]
fn check_unit(
    src: &str,
    name: &str,
    options: CompilerOptions,
    emit_format: ModuleKind,
    implied_node_format: ModuleKind,
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

// ── ROOT A — TS1049/TS1054 (the accessor `this`-parameter family) ──

/// The thisTypeInAccessors.ts shape (ROOT A): a get accessor declared
/// with a single `this` parameter and a set accessor declared with a
/// `this` parameter plus one value parameter report NO TS1054/TS1049 —
/// `getAccessorThisParameter` (Go checker.go:20335) now resolves the
/// Identifier-`"this"` parameter name, so
/// `doesAccessorHaveCorrectParameterCount` holds. The pre-tranche port
/// mis-saw the `this` parameter (ThisKeyword-kind name check) and fired
/// the accessor-arity grammar errors on both accessors.
#[test]
fn fcs43_accessor_this_params_no_1049_1054_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::CommonJS,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "interface Foo { n: number; x: number }\nconst o = {\n    n: 12,\n    get x(this: Foo): number { return this.n; },\n    set x(this: Foo, n: number) { this.n = n; },\n};\n",
        "/x/thisTypeInAccessors.ts",
        options,
        ModuleKind::CommonJS,
        tsc_core::compileroptions::ResolutionModeCommonJS,
        ScriptKind::TS,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 1054), 0, "unexpected TS1054, got: {ds:?}");
    assert_eq!(count(&ds, 1049), 0, "unexpected TS1049, got: {ds:?}");
}

/// The control (ROOT A): accessors whose parameter count is wrong even
/// WITH the `this` parameter still report the arity grammar errors — a
/// get accessor with `this` plus a value parameter fires TS1054, a set
/// accessor with `this` plus two value parameters fires TS1049
/// (`getAccessorThisParameter` answers nil whenever the accessor's
/// arity is incorrect — Go checker.go:20336's `1`/`2` gate).
#[test]
fn fcs43_accessor_bad_arity_still_1049_1054_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::CommonJS,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "interface Foo { n: number }\nclass C {\n    n = 0;\n    get x(this: Foo, v: number) { return this.n; }\n    set y(this: Foo, a: number, b: number) { }\n};\n",
        "/x/thisTypeInAccessorsBadArity.ts",
        options,
        ModuleKind::CommonJS,
        tsc_core::compileroptions::ResolutionModeCommonJS,
        ScriptKind::TS,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 1054), 1, "expected one TS1054, got: {ds:?}");
    assert_eq!(count(&ds, 1049), 1, "expected one TS1049, got: {ds:?}");
}

// ── ROOT B — TS1203 (the export= per-file implied-format family) ──

/// The nodeModulesExportAssignments.ts shape (ROOT B): `export =` in a
/// non-ambient .ts file under module node16 whose package implies
/// CommonJS reports NO TS1203 — Go's guard reads the per-file implied
/// format (CommonJS here), not the configured module kind. The
/// pre-tranche port answered with the configured node16 module kind and
/// fired.
#[test]
fn fcs43_export_equals_node16_commonjs_no_1203_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::Node16,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "declare const config: { a: number };\nexport = config;\n",
        "/x/index.ts",
        options,
        ModuleKind::Node16,
        tsc_core::compileroptions::ResolutionModeCommonJS,
        ScriptKind::TS,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 1203), 0, "unexpected TS1203, got: {ds:?}");
}

/// The verbatimModuleSyntaxRestrictionsESM.ts shape (ROOT B): `export =`
/// in an AMBIENT declaration file under module esnext reports NO TS1203
/// — the per-file implied format for the .d.ts is None (not ESNext), so
/// Go's ambient arm (`impliedNodeFormatForEmit == ModuleKindESNext`)
/// does not fire. The pre-tranche port compared the configured module
/// kind (esnext) and fired.
#[test]
fn fcs43_export_equals_ambient_dts_no_1203_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::ESNext,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "declare const config: { a: number };\nexport = config;\n",
        "/x/ambient.d.ts",
        options,
        ModuleKind::ESNext,
        tsc_core::compileroptions::ResolutionModeNone,
        ScriptKind::TS,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 1203), 0, "unexpected TS1203, got: {ds:?}");
}

/// The control (ROOT B — bundlerSyntaxRestrictions.ts main.ts): a
/// non-ambient .ts `export =` under module esnext still reports exactly
/// one TS1203 — the per-file implied format is None (the emit-module
/// fallback does not apply; GetImpliedNodeFormatForEmit answers
/// ResolutionModeNone, not CommonJS), so Go's non-ambient arm
/// (`impliedNodeFormatForEmit != ModuleKindCommonJS`) fires.
#[test]
fn fcs43_export_equals_esnext_ts_1203_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::ESNext,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "declare const config: { a: number };\nexport = config;\n",
        "/x/main.ts",
        options,
        ModuleKind::ESNext,
        tsc_core::compileroptions::ResolutionModeNone,
        ScriptKind::TS,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 1203), 1, "expected one TS1203, got: {ds:?}");
}

// ── ROOT C — TS2323 (the moduleExportAlias ExportsProperty family) ──

/// The moduleExportAlias.ts shape (ROOT C): in a CommonJS .js module,
/// a symbol declared twice through `exports.x = ...` assignments (the
/// test's repeated re-aliasing blocks) reports NO TS2323 — every
/// declaration classifies as `JSDeclarationKindExportsProperty`, so Go's
/// `checkExternalModuleExports` Every() guard (checker.go:5917) skips
/// the export-* conflict error. The pre-tranche port hardcoded the
/// permissive-false seam and fired on each declaration.
#[test]
fn fcs43_module_export_alias_no_2323_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES2015,
        module: ModuleKind::CommonJS,
        allow_js: Tristate::TSTrue,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "exports.func11 = function () { };\nexports.func12 = function () { };\nexports.func11 = function () { };\nexports.func12 = function () { };\n",
        "/x/moduleExportAlias.js",
        options,
        ModuleKind::CommonJS,
        tsc_core::compileroptions::ResolutionModeCommonJS,
        ScriptKind::JS,
    );
    let ds = diags(&mut c);
    assert_eq!(count(&ds, 2323), 0, "unexpected TS2323, got: {ds:?}");
}

/// The control (ROOT C — multipleDefaultExports04.ts shape): two
/// `export default function f` declarations put TWO non-ExportsProperty
/// declarations on the `default` export symbol — the Every() guard does
/// not suppress the TS2323 export-* conflict, which fires on both
/// declarations (Go's per-declaration loop, checker.go:5917).
#[test]
fn fcs43_mixed_declaration_still_2323_pin() {
    let options = CompilerOptions {
        target: ScriptTarget::ES5,
        module: ModuleKind::CommonJS,
        ..CompilerOptions::default()
    };
    let (mut c, _) = check_unit(
        "export default function f() {\n}\n\nexport default function f() {\n}\n",
        "/x/multipleDefaultExports04.ts",
        options,
        ModuleKind::CommonJS,
        tsc_core::compileroptions::ResolutionModeCommonJS,
        ScriptKind::TS,
    );
    let ds = diags(&mut c);
    assert_eq!(
        count(&ds, 2323),
        2,
        "expected the two duplicate-default TS2323 conflicts, got: {ds:?}"
    );
}

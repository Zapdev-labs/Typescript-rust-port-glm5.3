// Port of tsc/internal/testrunner/compiler_runner.go @
// ec47d33c23e464a17cdf2475632cba629bee8763 — the compiler/conformance
// baseline runner, at the DEPTH CONTROLLED stage (SPEC §8 M6).
//
// The Go runner drives the whole compiler; the port drives the pipeline
// as deep as it exists (the M5 program layer + the W3 checker wave):
// per-case tsconfig construction → program build (parse) → bind →
// check (the per-unit checker seam — `semantic_diagnostics_of_unit`) →
// diagnostics collection at the wired depth → `.errors.txt` baseline
// render → verdict vs baselines/reference. The per-case NDJSON record
// (case / baseline / verdict / errors_count / shape_hash) is the
// conformance TWIN's contract: the Go driver (rust/bench/go-driver)
// mirrors this module byte-for-byte — `diff(1)` over the two outputs is
// the verdict.
//
// DEPTH CONTROL — [`Stage`]: `Parse` (program build only), `Bind` (+
// `bind_source_files`, the default), `Check` (+ the checker crate's
// spine per in-test unit — Go program.go's `GetSemanticDiagnostics`
// counterpart, driving `check_source_file` through the ported checker;
// the single-file seam is PORT-noted in CONFORMANCE.md).
//
// PORT notes (Go → Rust):
//   - `harnessutil.CompileFiles` (cached host, tracer, output recorder,
//     build-info reader) is superseded at this depth by the M5 program-twin
//     construction: units seeded into a map FS at their rooted paths, the
//     per-case (synthetic or in-test) tsconfig written into the same FS,
//     `get_parsed_command_line_of_config_file_path` → `new_compiler_host`
//     → `new_program` — exactly the shape rust/crates/compiler's
//     program_test.rs established.
//   - Case options marshal into the synthetic tsconfig through the
//     generated table (options_generated.rs); the in-test tsconfig.json
//     route (Go's `ParseJsonSourceFileConfigFileContent` + the
//     `tsConfig.ParsedConfig.FileNames` root split) uses the written config
//     verbatim and splits roots via the parsed config's own `file_names`
//     (the same list the Go harness splits on).
//   - verifyContentMapper / verifyJavaScriptOutput / verifySourceMapOutput /
//     verifySourceMapRecord / verifyTypesAndSymbols (the `.js`/`.map`/
//     `.types`/`.symbols` arms) / verifyModuleResolution /
//     verifyUnionOrdering / verifyParentPointers are checker- or
//     emit-scope and not ported — they land with their waves; the climb
//     tracker (rust/CONFORMANCE.md) counts the `.types`/`.symbols`
//     reference trees as the future denominator.
//   - `skippedEmitTests` guards emit output order (not ported — emit wave).
//   - The runner's local-baseline cleanup (`cleanUpLocal`) is driver-side:
//     pass `local_root` to write local baselines through the ported
//     golden-file machinery (tsc_testutil::baseline) on demand.
//   - Panic containment: Go's `RecoverAndFail` fails the subtest; the port
//     emits the record with `errors_count: -1` (the documented panic
//     sentinel — the record shape stays the twin's five-field contract;
//     both drivers emit the same sentinel, the panic message itself is
//     driver-specific and deliberately NOT part of the record).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tsc_compiler::{
    FileDiagnostic, ProgramConfig, ProgramFactories, ProgramHosts, ProgramOptions, new_compiler_host,
    new_program,
};
use tsc_testutil::harness::{
    NamedTestConfiguration, TestConfiguration, enumerate_files, get_config_name_from_file_name,
    get_file_based_test_configurations, is_compiler_baseline_file,
};
use tsc_testutil::tsbaseline::{self, ErrorBaselineDiagnostic};
use tsc_tsoptions::tsconfigparsing::ExtendedConfigCache;
use tsc_tsoptions::get_parsed_command_line_of_config_file_path;
use tsc_tspath::{
    CaseSensitivity, RootedDirectoryPath, RootedFilePath, get_base_file_name, rooted_directory_path_from_normalized,
    to_rooted_directory_path, to_rooted_file_path,
};
use tsc_vfs::Fs;

use crate::options_generated::{compiler_vary_by, marshal_compiler_options};
use crate::test_case_parser::{
    TestUnit, extract_compiler_settings, parse_test_files_and_symlinks,
};

// ──────────────────────────────────────────────────────────────────────
// The W3 check stage: the per-file checker seam
// ──────────────────────────────────────────────────────────────────────

use tsc_ast::{NodeId, NodeStore, SourceFile, Symbol, SymbolId};
use tsc_binder::{BindResult, bind_source_file};
use tsc_checker::{Program, new_checker};
use tsc_core::options_generated::{CompilerOptions, ModuleKind};
use tsc_core::scriptkind::ScriptKind;
use tsc_core::tristate::Tristate;
use tsc_parser::parse_source_file;

/// Go: `srcFolder = tspath.RootedDirectoryPathFromNormalized("/.src")`.
pub fn src_folder() -> RootedDirectoryPath {
    rooted_directory_path_from_normalized("/.src")
}

/// The single-file `Program` the check stage hands the checker — mirrors
/// rust/crates/checker's own test seam (tests/w3_flow.rs's TestProgram):
/// the parsed unit + its bind result (the checker reads the binder's flow
/// arenas through `flow_arena`), the §5.5 symbol-store split (binder ids
/// below `binder_symbol_count`, checker-created ids above), and the case's
/// marshaled CompilerOptions.
struct CheckerUnitProgram {
    options: CompilerOptions,
    /// The unit's rooted name — the §5.1 parse kept the SourceFile anonymous
    /// (no file name), so the W3 module-format seams read it from here.
    name: RootedFilePath,
    source_file: SourceFile,
    bind: BindResult,
    binder_symbol_count: u32,
    checker_symbols: Vec<Symbol>,
    /// The per-unit resolution twin's answers (FCS tranche 7): the unit's
    /// entry of the 1-file map-FS program's resolvedModules (Go-driver
    /// conformance.go confSemanticDiagnosticsOfUnit's twin program — the
    /// file loader resolved every import of the unit against the unit-only
    /// FS; node16 .js→.ts substitution resolves "./index.js" to the unit
    /// itself).
    resolutions: tsc_module::ModeAwareCache<Option<std::sync::Arc<tsc_module::ResolvedModule>>>,
    /// Go: program.SourceFileMayBeEmitted(unit, false) — the twin program's
    /// own computation (the rewriteRelativeImportExtensions arm reads it).
    may_be_emitted: bool,
}

impl Program for CheckerUnitProgram {
    fn options(&self) -> &CompilerOptions {
        &self.options
    }
    fn source_files(&self) -> &[SourceFile] {
        std::slice::from_ref(&self.source_file)
    }
    // The unit is bound before the adapter is built (the bind result must
    // be kept for `flow_arena`); NewChecker's BindSourceFiles call is the
    // no-op — the same shape the checker's own tests use.
    fn bind_source_files(&mut self) {}
    // M6 W5: the JSX react-jsx auto-import seam — mirrors the Go program's
    // jsxRuntimeImportSpecifiers map (the file loader computes the
    // specifier per JS/TSX file via GetJSXImplicitImportBase +
    // GetJSXRuntimeImport; the twin program has exactly one file).
    fn get_jsx_runtime_import_specifier(&self, file: NodeId) -> Option<String> {
        if file != self.source_file.as_node_id() {
            return None;
        }
        if tsc_compiler::ast_shims::is_source_file_js_by_file(&self.source_file)
            || self.source_file.payload().script_kind == ScriptKind::TSX
        {
            let specifier = tsc_compiler::ast_shims::get_jsx_runtime_import(
                &tsc_compiler::ast_shims::get_jsx_implicit_import_base(
                    &self.options,
                    &self.source_file,
                ),
                &self.options,
            );
            if !specifier.is_empty() {
                return Some(specifier);
            }
        }
        None
    }
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
    // ── The W3 grammar seams (Go program.go GetEmitModuleFormatOfFile /
    // GetSourceFileMetaData).ImpliedNodeFormat) ──
    //
    // The unit program carries no module-resolution data (the single-file
    // checker never resolves), so Go's fileLoader metadata arm is the
    // SkipModuleResolution shape: implied format from the file name alone,
    // no package.json "type" (the driver twin's FS map holds only the unit —
    // the same read fails there too).
    fn get_emit_module_format_of_file(&self, _file: tsc_ast::NodeId) -> ModuleKind {
        let implied_node_format =
            tsc_compiler::ast_shims::get_implied_node_format_for_file(&self.name, "");
        tsc_compiler::ast_shims::get_emit_module_format_of_file_worker(
            &self.name,
            &self.options,
            &tsc_ast::SourceFileMetaData {
                package_json_type: String::new(),
                package_json_directory: Default::default(),
                implied_node_format,
            },
        )
    }
    fn source_file_implied_node_format(&self, _file: tsc_ast::NodeId) -> ModuleKind {
        tsc_compiler::ast_shims::get_implied_node_format_for_file(&self.name, "")
    }
    // ── The FCS-tranche-6 module-resolution-mode seams (Go: file.FileName(),
    // p.sourceFileMetaDatas[...] and program.FileExists — the per-unit twin's
    // answers: the name and meta the adapter already computes for the
    // module-format seams; the map-FS twin holds only the unit, so
    // FileExists is exact-name equality). ──
    fn source_file_name(&self, _file: tsc_ast::NodeId) -> String {
        self.name.as_string().to_string()
    }
    fn source_file_meta_data(&self, _file: tsc_ast::NodeId) -> tsc_ast::SourceFileMetaData {
        let implied_node_format =
            tsc_compiler::ast_shims::get_implied_node_format_for_file(&self.name, "");
        tsc_ast::SourceFileMetaData {
            package_json_type: String::new(),
            package_json_directory: Default::default(),
            implied_node_format,
        }
    }
    fn file_exists(&self, path: &str) -> bool {
        path == self.name.as_string()
    }
    // ── The FCS-tranche-7 resolved-module seams (Go: program.GetResolvedModule /
    // GetSourceFileForResolvedModule / SourceFileMayBeEmitted — the per-unit
    // twin program's exact answers; see semantic_diagnostics_of_unit). ──
    fn get_resolved_module(
        &self,
        file: tsc_ast::NodeId,
        module_reference: &str,
        mode: tsc_core::compileroptions::ResolutionMode,
    ) -> Option<std::sync::Arc<tsc_module::ResolvedModule>> {
        if file != self.source_file.as_node_id() {
            return None;
        }
        // Go: `if resolved, ok := resolutions[ModeAwareCacheKey{...}]; ok {
        // return resolved }` — nil on miss. The loader always stores
        // `Some` (a recorded failure is Go's `&module.ResolvedModule{}`),
        // so the flattened Option only misses on a cache miss.
        self.resolutions
            .get(&tsc_module::ModeAwareCacheKey {
                name: module_reference.to_string(),
                mode,
            })
            .cloned()
            .flatten()
    }
    fn get_source_file_for_resolved_module(
        &self,
        resolved: &tsc_module::ResolvedModule,
    ) -> Option<tsc_ast::NodeId> {
        // Go: p.getSourceFileWithRedirect(resolvedFileName, resolvedPath) —
        // the map-FS twin holds only the unit, so every successful
        // resolution resolves to it (name equality is the twin's answer).
        if resolved.resolved_file_name == self.name {
            Some(self.source_file.as_node_id())
        } else {
            None
        }
    }
    fn source_file_may_be_emitted(&self, file: tsc_ast::NodeId) -> bool {
        if file == self.source_file.as_node_id() {
            self.may_be_emitted
        } else {
            false
        }
    }
}

/// The unit's semantic diagnostics — Go program.go `GetSemanticDiagnostics`'s
/// per-file read, driven through the full ported spine
/// (`check_source_file(checkUnused=false)`), over the unit's own parse+bind
/// (the bytes are the program's bytes; the §5.1 single-file checker is the
/// W3 seam — PORT-noted in CONFORMANCE.md). Panics inside the checker stay
/// contained by the case-level catch_unwind (the -1 sentinel record).
fn semantic_diagnostics_of_unit(
    name: &str,
    content: &str,
    options: &CompilerOptions,
    parsed: &tsc_tsoptions::parsedcommandline::ParsedCommandLine,
) -> Vec<tsc_ast::Diagnostic> {
    // PORT (M6 W3 t7): the Go check twin (go-bench conformance's
    // confSemanticDiagnosticsOfUnit) builds a map-FS program, and the vfs
    // read (tsc/internal/vfs/internal's Common.ReadFile → decodeBytes)
    // strips a leading UTF-8 BOM before the parser sees the text — every
    // check-stage position is relative to the BOM-less text. The parse/bind
    // stages above already route through the program's vfs; this seam parses
    // the unit bytes directly, so the same strip applies here (the 2703
    // spans of the BOM'd await_unary cases). The UTF-16 arm of decodeBytes
    // is unreachable here: content is a Rust &str (already valid UTF-8).
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    // PORT(FCS tranche 6): Go's check twin (go-driver conformance.go
    // confSemanticDiagnosticsOfUnit) switches on core.GetScriptKindFromFileName
    // and returns nil for ScriptKindJSON AND ScriptKindUnknown — extensionless
    // and unsupported-extension units (bundlerImportTsExtensions' /project/e
    // and /project/e.txt) are never checked, so the machinery TS2318s
    // (getGlobalType reportErrors) and garbage semantic errors never fire for
    // them. The seam previously used ensure_script_kind_from_file_name
    // (Unknown forced to TS) and checked those units — 532 rust-extra TS2318s
    // on the corpus. Units that survive the switch have a real script kind by
    // construction, so the parse below sees the identical kind Go's
    // per-unit program would.
    let script_kind = match tsc_compiler::get_script_kind_from_file_name(&RootedFilePath::from(name)) {
        ScriptKind::JSON | ScriptKind::Unknown => return Vec::new(),
        kind => kind,
    };
    // PORT(FCS tranche 7): Go's check twin (go-driver conformance.go
    // confSemanticDiagnosticsOfUnit) builds a 1-file map-FS program — the
    // REAL file loader/module resolver runs against the unit-only FS (the
    // map holds only the unit; the default-library path is "/" — the
    // no-libs sentinel), filling program.resolvedModules with the unit's
    // resolution answers: node16 .js→.ts (and .mjs→.mts) extension
    // substitution resolves "./index.js" to the unit itself; CJS
    // directory/index resolution answers require("./") and
    // require("./index"). The Rust seam hands those answers to the checker
    // through the Program trait's FCS-tranche-7 resolved-module surface —
    // mirroring the Go twin's construction exactly (the case's parsed
    // config with the file list swapped for the unit; no bind —
    // ModuleAugmentations is parse-set, parser/references.go, and the
    // loader reads no bind data).
    let unit_entries = [(name.to_string(), content.to_string())];
    let unit_fs = tsc_bundled::wrap_fs(tsc_vfs::vfstest::from_map(
        unit_entries,
        CaseSensitivity::CaseSensitive,
    ));
    let unit_host = new_compiler_host(
        Arc::<dyn Fs>::clone(&unit_fs),
        rooted_directory_path_from_normalized("/"),
        nil_extended_config_cache(),
        None,
    );
    let unit_parsed = parsed.with_file_names(vec![RootedFilePath::from(name)]);
    let unit_program = new_program(ProgramOptions {
        config: ProgramConfig {
            config: std::sync::Arc::new(unit_parsed),
            ..Default::default()
        },
        hosts: ProgramHosts {
            host: Some(unit_host),
        },
        factories: ProgramFactories::default(),
    });
    // Go: `files := program.GetSourceFiles(); if len(files) == 0 { return
    // nil }` — the belt to the same braces (a real script kind by the
    // switch above always yields the unit). The unit's resolutions entry +
    // SourceFileMayBeEmitted are the twin reads the checker seam needs.
    let Some(unit_file) = unit_program
        .get_source_files()
        .iter()
        .find(|file| file.file_name().as_string() == name)
    else {
        return Vec::new();
    };
    let resolutions = unit_program
        .get_resolved_modules()
        .get(unit_file.path_key())
        .cloned()
        .unwrap_or_default();
    let may_be_emitted = unit_program.source_file_may_be_emitted(unit_file, false);
    let mut source_file = parse_source_file(Default::default(), content, script_kind);
    let Some(bind) = bind_source_file(&mut source_file) else {
        return Vec::new();
    };
    let binder_symbol_count = bind.symbol_count as u32;
    let file = source_file.as_node_id();
    let mut checker = new_checker(Box::new(CheckerUnitProgram {
        options: options.clone(),
        name: RootedFilePath::from(name),
        source_file,
        bind,
        binder_symbol_count,
        checker_symbols: Vec::new(),
        resolutions,
        may_be_emitted,
    }));
    // Go's exported GetDiagnostics (checker.go:14222+14232) computes
    // checkUnused from the noUnused* options — the regular (non-suggestion)
    // collection read the driver twins. The port's seam previously passed
    // false unconditionally, diverging on every noUnused* case record.
    // PORT: mirrors Go's `NoUnusedLocals.IsTrue() || NoUnusedParameters.IsTrue()`
    // (the `collection == &c.suggestionDiagnostics` arm is the suggestion read,
    // which the driver does not make).
    let check_unused = options.no_unused_locals == Tristate::TSTrue
        || options.no_unused_parameters == Tristate::TSTrue;
    checker.check_source_file(file, check_unused);
    checker.semantic_diagnostics()
}

// ──────────────────────────────────────────────────────────────────────
// Depth control
// ──────────────────────────────────────────────────────────────────────

/// The pipeline depth the runner drives (SPEC §8 M6's depth control).
///
/// `Check` is wired through the W3 checker wave (rust/crates/checker's
/// spine: `check_source_file` + the flow engine) via the per-file checker
/// seam (`semantic_diagnostics_of_unit`) — Go's program-layer entry points
/// (`GetSemanticDiagnostics` in tsc/internal/compiler/program.go) are the
/// shape the M5 program merge will grow; the port drives the same spine
/// per in-test unit today (PORT-noted in CONFORMANCE.md).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Stage {
    /// Program construction only (parse).
    Parse,
    /// Program construction + `bind_source_files`.
    #[default]
    Bind,
    /// Bind + the per-unit semantic layer (the checker spine).
    Check,
}

impl Stage {
    /// The flag string (both drivers share the grammar).
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Parse => "parse",
            Stage::Bind => "bind",
            Stage::Check => "check",
        }
    }

    /// Parses the flag string (unknown values are errors).
    pub fn parse(value: &str) -> Result<Stage, String> {
        match value {
            "parse" => Ok(Stage::Parse),
            "bind" => Ok(Stage::Bind),
            "check" => Ok(Stage::Check),
            other => Err(format!("unknown stage {other:?} (parse|bind|check)")),
        }
    }
}

// ──────────────────────────────────────────────────────────────────────
// The verdict / record contract
// ──────────────────────────────────────────────────────────────────────

/// The per-case verdict (the twin's contract vocabulary).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The rendered `.errors.txt` content equals the reference (or both
    /// sides expect no baseline: no diagnostics, no reference file).
    Match,
    /// The reference exists but the rendered content differs — or the case
    /// panicked (the `errors_count: -1` sentinel).
    Mismatch,
    /// The runner produces content but the reference has none — a new
    /// baseline would be created (`baseline-accept` territory).
    MissingBaseline,
}

impl Verdict {
    fn as_str(self) -> &'static str {
        match self {
            Verdict::Match => "match",
            Verdict::Mismatch => "mismatch",
            Verdict::MissingBaseline => "missing-baseline",
        }
    }
}

/// One per-case NDJSON record (the twin's five-field contract; field order
/// is the contract).
#[derive(Clone, Debug)]
pub struct CaseRecord {
    /// The case path relative to the cases root (posix).
    pub case: String,
    /// The baseline path relative to the baselines root:
    /// `<suite>/<configuredName>.errors.txt`.
    pub baseline: String,
    pub verdict: Verdict,
    /// The diagnostics count at the wired depth; `-1` is the panic
    /// sentinel (PORT-noted in the module header).
    pub errors_count: i64,
    /// FNV-1a 64 (lowercase 16-hex) over the record's fold lines (see
    /// [`shape_hash`]).
    pub shape_hash: String,
    /// The fold lines themselves (the shape_hash input) — NOT part of the
    /// NDJSON contract; the `--dump-filter` read on the bench-harness seam
    /// (and the go-driver twin's) prints them to STDERR for the W3
    /// check-stage divergence bisect. `None` on the panic sentinel.
    pub dump_lines: Option<Vec<String>>,
}

impl CaseRecord {
    /// The NDJSON line (field order is the contract; `json_escape` rules
    /// shared with the Go driver).
    pub fn to_ndjson(&self) -> String {
        let mut out = String::new();
        out.push_str("{\"case\":\"");
        crate::options_generated::json_escape(&self.case, &mut out);
        out.push_str("\",\"baseline\":\"");
        crate::options_generated::json_escape(&self.baseline, &mut out);
        out.push_str("\",\"verdict\":\"");
        out.push_str(self.verdict.as_str());
        out.push_str("\",\"errors_count\":");
        out.push_str(&self.errors_count.to_string());
        out.push_str(",\"shape_hash\":\"");
        out.push_str(&self.shape_hash);
        out.push_str("\"}");
        out
    }
}

/// The runner's configuration (the twin's flag grammar).
#[derive(Clone, Debug)]
pub struct ConformanceConfig {
    /// The cases tree (Go: `basePath = "tests/cases/<suite>"` under the
    /// test-data root; the driver resolves it absolutely).
    pub cases_root: PathBuf,
    /// The baselines/reference root.
    pub baselines_root: PathBuf,
    /// The suite subfolder ("conformance" | "compiler").
    pub suite: String,
    /// `--limit N` — take the first N cases after the skip/filter passes
    /// (0 = all).
    pub limit: usize,
    /// `--filter pattern` — the `-test.run` pattern ("" = all).
    pub filter: String,
    /// The wired depth.
    pub stage: Stage,
    /// When set, write local baselines through the ported golden-file
    /// machinery on divergence (Go's local-baseline behavior).
    pub local_root: Option<PathBuf>,
}

impl Default for ConformanceConfig {
    fn default() -> Self {
        ConformanceConfig {
            cases_root: PathBuf::new(),
            baselines_root: PathBuf::new(),
            suite: "conformance".to_string(),
            limit: 0,
            filter: String::new(),
            stage: Stage::Bind,
            local_root: None,
        }
    }
}

// ──────────────────────────────────────────────────────────────────────
// Case enumeration
// ──────────────────────────────────────────────────────────────────────

/// Go: `var skippedTests` (verbatim — tests that depended on
/// typescript.d.ts in built, or options that have been removed entirely).
pub const SKIPPED_TESTS: &[&str] = &[
    // Tests that depended on typescript.d.ts in built.
    "APILibCheck.ts",
    "APISample_Watch.ts",
    "APISample_WatchWithDefaults.ts",
    "APISample_WatchWithOwnWatchHost.ts",
    "APISample_compile.ts",
    "APISample_jsdoc.ts",
    "APISample_linter.ts",
    "APISample_parseConfig.ts",
    "APISample_transform.ts",
    "APISample_watcher.ts",
    // These tests contain options that have been completely removed, so fail to parse.
    "preserveUnusedImports.ts",
    "noCrashWithVerbatimModuleSyntaxAndImportsNotUsedAsValues.ts",
    "verbatimModuleSyntaxCompat.ts",
    "verbatimModuleSyntaxCompat2.ts",
    "verbatimModuleSyntaxCompat3.ts",
    "verbatimModuleSyntaxCompat4.ts",
    "preserveValueImports.ts",
    "preserveValueImports_importsNotUsedAsValues.ts",
    "preserveValueImports_errors.ts",
    "preserveValueImports_mixedImports.ts",
    "preserveValueImports_module.ts",
    "importsNotUsedAsValues_error.ts",
    "alwaysStrictNoImplicitUseStrict.ts",
    "nonPrimitiveIndexingWithForInSupressError.ts",
    "parameterInitializerBeforeDestructuringEmit.ts",
    "mappedTypeUnionConstraintInferences.ts",
    "lateBoundConstraintTypeChecksCorrectly.ts",
    "keyofDoesntContainSymbols.ts",
    "noStrictGenericChecks.ts",
    "noImplicitUseStrict_umd.ts",
    "noImplicitUseStrict_system.ts",
    "noImplicitUseStrict_es6.ts",
    "noImplicitUseStrict_commonjs.ts",
    "noImplicitAnyIndexingSuppressed.ts",
    "excessPropertyErrorsSuppressed.ts",
    "moduleNoneDynamicImport.ts",
    "moduleNoneErrors.ts",
    "noErrorUsingImportExportModuleAugmentationInDeclarationFile1.ts",
    "noErrorUsingImportExportModuleAugmentationInDeclarationFile2.ts",
    "noErrorUsingImportExportModuleAugmentationInDeclarationFile3.ts",
    "requireOfJsonFileWithModuleEmitNone.ts",
    "requireOfJsonFileWithModuleNodeResolutionEmitNone.ts",
];

/// Go: `CompilerBaselineRunner.EnumerateTestFiles` — the recursive
/// `\.tsx?$` enumeration, then the skip list and the `-test.run` filter,
/// then `--limit`. The final list is sorted by full path (the deterministic
/// total order both drivers share).
pub fn enumerate_case_files(config: &ConformanceConfig) -> Result<Vec<String>, String> {
    let files = enumerate_files(&config.cases_root, true, is_compiler_baseline_file)?;
    let filter = crate::compiler_test_filter::compiler_test_file_filter(&config.filter);
    let mut selected: Vec<String> = Vec::new();
    for file in files {
        let basename = get_base_file_name(&file).into_owned();
        if SKIPPED_TESTS.contains(&basename.as_str()) {
            continue;
        }
        if !filter(&basename) {
            continue;
        }
        selected.push(file);
        if config.limit > 0 && selected.len() >= config.limit {
            break;
        }
    }
    Ok(selected)
}

// ──────────────────────────────────────────────────────────────────────
// The run loop
// ──────────────────────────────────────────────────────────────────────

/// Runs the suite at the wired depth, emitting one record per
/// (case, configuration). Returns the record count.
pub fn run_conformance(config: &ConformanceConfig, mut emit: impl FnMut(&CaseRecord)) -> Result<usize, String> {
    let files = enumerate_case_files(config)?;
    let mut count = 0usize;
    for file in files {
        for record in run_case_file(config, &file) {
            emit(&record);
            count += 1;
        }
    }
    Ok(count)
}

/// Go: `CompilerBaselineRunner.runTest` — one case file, every
/// configuration (Go's `t.Run` subtests collapse onto one record each).
fn run_case_file(config: &ConformanceConfig, file: &str) -> Vec<CaseRecord> {
    let content = match std::fs::read(file) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(err) => panic!("Could not read test file: {file}: {err}"),
    };
    let basename = get_base_file_name(file).into_owned();
    let settings = extract_compiler_settings(&content);
    let mut configurations = get_file_based_test_configurations(&settings, &compiler_vary_by);
    // Go's configuration order is map-random; the twin sorts by the
    // configuration's name (the baseline name's parenthetical) — the
    // deterministic shared order (PORT-noted in CONFORMANCE.md).
    configurations.sort_by(|a, b| a.name.cmp(&b.name));
    if configurations.is_empty() {
        configurations.push(NamedTestConfiguration {
            name: String::new(),
            config: TestConfiguration::new(),
        });
    }

    let payload = parse_test_files_and_symlinks(&content, file);
    let units: Vec<TestUnit> = payload.0;
    let symlinks: BTreeMap<String, String> = payload.1;
    let case = case_relative(config, file);

    let mut records = Vec::with_capacity(configurations.len());
    for named in &configurations {
        records.push(run_case_configuration(config, &case, &basename, &units, &symlinks, named, &content));
    }
    records
}

/// The case path relative to the cases root, posix separators.
fn case_relative(config: &ConformanceConfig, file: &str) -> String {
    let root = config.cases_root.to_string_lossy();
    let trimmed = file
        .strip_prefix(root.as_ref())
        .map(|rest| rest.trim_start_matches('/').to_string())
        .unwrap_or_else(|| file.to_string());
    trimmed.replace('\\', "/")
}

/// A panic payload downcast to its message (`&str` or `String`; other
/// payloads render as "unknown panic").
fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

/// Go: `runSingleConfigTest` + `newCompilerTest` — the per-configuration
/// pipeline, with the panic containment (the `-1` sentinel record).
fn run_case_configuration(
    config: &ConformanceConfig,
    case: &str,
    basename: &str,
    units: &[TestUnit],
    symlinks: &BTreeMap<String, String>,
    named: &NamedTestConfiguration,
    _content: &str,
) -> CaseRecord {
    let baseline_name = baseline_file_name(basename, &named.name);
    let baseline = format!("{}/{}", config.suite, baseline_name);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run_configuration_pipeline(config, case, &baseline_name, units, symlinks, &named.config)
    }));
    match result {
        Ok(outcome) => CaseRecord {
            case: case.to_string(),
            baseline,
            verdict: outcome.verdict,
            errors_count: outcome.errors_count,
            shape_hash: outcome.shape_hash,
            dump_lines: Some(outcome.lines),
        },
        Err(payload) => {
            // The panic message goes to STDERR only (stdout stays the pure
            // NDJSON contract; the message itself is driver-specific).
            eprintln!(
                "conformance: panic on case {case} (baseline {baseline}): {}",
                panic_message(payload)
            );
            CaseRecord {
                case: case.to_string(),
                baseline,
                verdict: Verdict::Mismatch,
                errors_count: -1,
                shape_hash: String::new(),
                dump_lines: None,
            }
        }
    }
}

/// Go: `newCompilerTest`'s configuredName: `base(ext)(desc)ext`.
fn configured_name(basename: &str, configuration_name: &str) -> String {
    if configuration_name.is_empty() {
        return basename.to_string();
    }
    let ext = any_extension(basename);
    let extensionless = &basename[..basename.len() - ext.len()];
    format!("{extensionless}({configuration_name}){ext}")
}

/// Go: `tspath.GetAnyExtensionFromPath(basename, nil, CaseSensitive)` —
/// the last `.xxx` suffix ("" when none).
fn any_extension(name: &str) -> &str {
    match name.rfind('.') {
        Some(index) => &name[index..],
        None => "",
    }
}

/// The `<suite>/<configuredName>.errors.txt` baseline file name (Go:
/// `tsExtension.ReplaceAllString(baselinePath, ".errors.txt")`).
fn baseline_file_name(basename: &str, configuration_name: &str) -> String {
    let configured = configured_name(basename, configuration_name);
    tsbaseline::replace_ts_extension(&configured, ".errors.txt")
}

/// The per-configuration observable outcome.
struct PipelineOutcome {
    verdict: Verdict,
    errors_count: i64,
    shape_hash: String,
    /// The fold lines (the shape_hash input) — the `--dump-filter` bisect
    /// read (CaseRecord::dump_lines).
    lines: Vec<String>,
}

/// The full depth-controlled pipeline (Go: `newCompilerTest` +
/// `verifyDiagnostics`'s baseline selection + the twin's verdict mapping).
fn run_configuration_pipeline(
    config: &ConformanceConfig,
    case: &str,
    baseline_name: &str,
    units: &[TestUnit],
    symlinks: &BTreeMap<String, String>,
    harness_config: &TestConfiguration,
) -> PipelineOutcome {
    // Go: currentDirectory = srcFolder, or the @currentDirectory directive
    // rooted against it.
    let current_directory = match harness_config.get("currentdirectory") {
        Some(raw) if !raw.is_empty() => to_rooted_directory_path(raw, &src_folder()),
        _ => src_folder(),
    };

    // Rooted unit names (Go: ToRootedFilePath(unit.name, currentDirectory)).
    let rooted_names: Vec<String> = units
        .iter()
        .map(|unit| to_rooted_file_path(&unit.name, &current_directory).as_string().to_string())
        .collect();

    // The in-test tsconfig unit (Go: GetConfigNameFromFileName loop).
    let ts_config_unit: Option<usize> = units
        .iter()
        .position(|unit| !get_config_name_from_file_name(&unit.name).is_empty());

    // Roots vs other files (Go newCompilerTest's split):
    //   - a tsconfig unit: split by the parsed config's own file list
    //     (Go: tsConfig.ParsedConfig.FileNames)
    //   - otherwise: the require/triple-slash-reference heuristic over the
    //     last unit (Go's exact rule)
    let (root_indexes, other_indexes): (Vec<usize>, Vec<usize>);
    let no_implicit_references = harness_config
        .get("noimplicitreferences")
        .is_some_and(|value| !value.is_empty());
    let last = units.len() - 1;
    let references_last = no_implicit_references
        || units[last].content.contains("require(")
        || contains_reference_path(&units[last].content);
    if references_last {
        root_indexes = vec![last];
        other_indexes = (0..last).collect();
    } else {
        root_indexes = (0..units.len()).collect();
        other_indexes = Vec::new();
    }

    // The config route: the in-test tsconfig verbatim, or the synthetic
    // per-case tsconfig (options + the root files, minus .json/.tsbuildinfo
    // units — Go CompileFilesEx's programFileNames filter).
    let config_path: RootedFilePath;
    let mut config_text = String::new();
    match ts_config_unit {
        Some(index) => {
            config_path = RootedFilePath::from(rooted_names[index].as_str());
            config_text.push_str(&units[index].content);
        }
        None => {
            config_path = to_rooted_file_path("__conformance__.tsconfig.json", &current_directory);
            config_text.push_str("{\"compilerOptions\":{");
            marshal_compiler_options(harness_config, &mut config_text);
            config_text.push_str("},\"files\":[");
            let mut first = true;
            for &index in &root_indexes {
                let ext = any_extension(&units[index].name);
                if ext == ".json" || ext == ".tsbuildinfo" {
                    continue;
                }
                if !first {
                    config_text.push(',');
                }
                first = false;
                config_text.push('"');
                crate::options_generated::json_escape(&rooted_names[index], &mut config_text);
                config_text.push('"');
            }
            config_text.push_str("]}");
        }
    }

    // The map FS: every unit at its rooted path + the config + symlinks
    // seeded as file copies of their target unit (Go: vfstest.Symlink; the
    // copy is the depth-controlled simplification — a resolved target's
    // bytes are identical, and both drivers share the rule; targets outside
    // the unit set are dropped — PORT-noted in CONFORMANCE.md).
    let mut entries: Vec<(String, String)> = Vec::with_capacity(units.len() + 1 + symlinks.len());
    for (index, unit) in units.iter().enumerate() {
        entries.push((rooted_names[index].clone(), unit.content.clone()));
    }
    entries.push((config_path.as_string().to_string(), config_text.clone()));
    for (link, target) in symlinks {
        let link_path = to_rooted_file_path(link, &current_directory);
        let target_path = to_rooted_file_path(target, &current_directory);
        if let Some((_, content)) = entries.iter().find(|(name, _)| name == target_path.as_string()) {
            entries.push((link_path.as_string().to_string(), content.clone()));
        }
    }
    // The in-test tsconfig unit seeds both its unit entry and the config
    // entry at the same path — the Go twin's map assignment dedupes
    // silently (last write wins); the Vec folds through the same rule so
    // from_map never sees duplicates.
    let deduped: std::collections::BTreeMap<String, String> = entries.into_iter().collect();
    let entries: Vec<(String, String)> = deduped.into_iter().collect();
    let fs = tsc_bundled::wrap_fs(tsc_vfs::vfstest::from_map(entries, CaseSensitivity::CaseSensitive));

    // The M5 program-twin construction.
    let parsed = get_parsed_command_line_of_config_file_path(&config_path, fs.as_ref(), None);
    // The W3 check stage: the checker reads the same CompilerOptions the
    // program was built from (Go: the checker reads
    // `program.CompilerOptions()`; the parsed copy predates the Arc move).
    let checker_options = parsed.compiler_options.options.clone();
    // PORT(FCS tranche 7): the per-unit resolution twins read the case's
    // parsed config (Go's confSemanticDiagnosticsOfUnit passes it through
    // WithFileNames) — keep the Arc alongside the program's own.
    let parsed_arc = Arc::new(parsed);
    let host = new_compiler_host(
        Arc::<dyn Fs>::clone(&fs),
        tsc_bundled::lib_path(),
        nil_extended_config_cache(),
        None,
    );
    let mut program = new_program(ProgramOptions {
        config: ProgramConfig {
            config: Arc::clone(&parsed_arc),
            ..Default::default()
        },
        hosts: ProgramHosts { host: Some(host) },
        factories: ProgramFactories::default(),
    });
    if config.stage != Stage::Parse {
        program.bind_source_files();
    }

    // Roots vs others for a tsconfig unit: the parsed config's own file list
    // (the same list Go's harness splits on).
    let (root_indexes, other_indexes) = match ts_config_unit {
        Some(_) => {
            let file_names: Vec<String> = program
                .get_source_files()
                .iter()
                .map(|file| file.file_name().as_string().to_string())
                .collect();
            let listed = |index: usize| file_names.contains(&rooted_names[index]);
            let mut roots = Vec::new();
            let mut others = Vec::new();
            for index in 0..units.len() {
                if index == ts_config_unit.expect("checked above") {
                    continue;
                }
                if listed(index) {
                    roots.push(index);
                } else {
                    others.push(index);
                }
            }
            (roots, others)
        }
        None => (root_indexes, other_indexes),
    };

    // ── Diagnostics at the wired depth ──
    // The tsconfig-parse errors, the per-file parse diagnostics (the raw
    // parser + JSDoc set — the program twin's rule: Go's additional
    // JS-extras and checker-scope arms land with their waves), and the
    // program diagnostics (option verification + include-processor
    // globals).
    let mut views: Vec<ErrorBaselineDiagnostic<'_>> = Vec::new();
    let mut owned: Vec<(String, String, tsc_ast::Diagnostic)> = Vec::new();
    for file_diagnostic in program.get_config_file_parsing_diagnostics() {
        owned.push(file_diagnostic_view(file_diagnostic, &program, &config_path, &config_text));
    }
    for file in program.get_source_files() {
        for d in file.diagnostics() {
            owned.push((
                file.file_name().as_string().to_string(),
                file.text().to_string(),
                d.clone(),
            ));
        }
        for d in file.js_diagnostics() {
            owned.push((
                file.file_name().as_string().to_string(),
                file.text().to_string(),
                d.clone(),
            ));
        }
    }
    for file_diagnostic in program.get_program_diagnostics() {
        owned.push(file_diagnostic_view(file_diagnostic, &program, &config_path, &config_text));
    }
    // ── The W3 check stage: the semantic layer ──
    // Go: the harness reads `GetSemanticDiagnostics(file)` for every file in
    // the program (the compiler result's Diagnostics). The port's checker
    // is single-file by construction (the §5.1 NodeId space; the M5 program
    // merge is the seam that swaps this for a program-wide checker), so the
    // stage drives the per-file pipeline over each unit's own bytes
    // (identical content → identical locations), in Go's input-file order:
    // the in-test tsconfig unit is the config (not checked — Go parses it
    // as JSON), and .json/.tsbuildinfo units are not TS sources (Go's
    // checker skips them too).
    if config.stage == Stage::Check {
        for (index, unit) in units.iter().enumerate() {
            if Some(index) == ts_config_unit {
                continue;
            }
            let ext = any_extension(&units[index].name);
            if ext == ".json" || ext == ".tsbuildinfo" {
                continue;
            }
            let diags = semantic_diagnostics_of_unit(&rooted_names[index], &unit.content, &checker_options, &parsed_arc);
            if !diags.is_empty() {
                let text = unit.content.clone();
                for d in diags {
                    // PORT(FCS, 2026-10-14): Go's fileless diagnostics
                    // (NewCompilerDiagnostic — UndefinedTextRange {-1,-1},
                    // nil File) are GLOBALS: the baseline renders them in
                    // the global-error block with no location
                    // (error_baseline.go's `error.File() == nil` arm). The
                    // per-unit seam lumps them in with the unit's
                    // diagnostics, which rendered a bogus "file(1,0):"
                    // location — and, once spans went Go-exact (-1,-1),
                    // panicked the baseline's column slice. pos < 0 is
                    // exactly Go's nil-File shape here.
                    if d.pos() >= 0 {
                        owned.push((rooted_names[index].clone(), text.clone(), d));
                    } else {
                        owned.push((String::new(), String::new(), d));
                    }
                }
            }
        }
    }
    let errors_count = owned.len() as i64;
    for (name, text, diagnostic) in &owned {
        views.push(ErrorBaselineDiagnostic {
            file_name: name,
            file_text: text,
            diagnostic,
        });
    }

    // The input files in Go's order: tsconfig files, to-be-compiled, others
    // (Go verifyDiagnostics' Concatenate).
    let mut input_files: Vec<(&str, &str)> = Vec::with_capacity(units.len());
    if let Some(index) = ts_config_unit {
        input_files.push((rooted_names[index].as_str(), units[index].content.as_str()));
    }
    for &index in &root_indexes {
        input_files.push((rooted_names[index].as_str(), units[index].content.as_str()));
    }
    for &index in &other_indexes {
        input_files.push((rooted_names[index].as_str(), units[index].content.as_str()));
    }

    // The .errors.txt content (Go: DoErrorBaseline's selection).
    let view_refs: Vec<&ErrorBaselineDiagnostic<'_>> = views.iter().collect();
    let content = tsbaseline::error_baseline_content(&input_files, &view_refs);

    // ── The verdict (Go writeComparison's semantics, mapped onto the twin) ──
    let reference = config.baselines_root.join(&config.suite).join(baseline_name);
    let verdict = verdict_for(&content, &reference);

    // ── The local-baseline write (on demand — the ported golden-file
    //    machinery; Go's writeComparison side effect) ──
    if let Some(local_root) = &config.local_root {
        let paths = tsc_testutil::baseline::BaselinePaths {
            local_root: local_root.clone(),
            reference_root: config.baselines_root.clone(),
        };
        let _ = tsc_testutil::baseline::run_baseline(
            &paths,
            baseline_name,
            &content,
            &tsc_testutil::baseline::BaselineOptions {
                subfolder: config.suite.clone(),
            },
        );
    }

    // ── The shape_hash fold lines (the twin's recipe; the Go driver
    //    mirrors byte-for-byte): case, baseline, errors, then one
    //    diag line per diagnostic in the render sort order. ──
    let mut sorted_views: Vec<&ErrorBaselineDiagnostic<'_>> = views.iter().collect();
    sorted_views.sort_by(|a, b| tsbaseline::compare_error_baseline_diagnostics(a, b));
    let mut lines = vec![
        format!("case={case}\n"),
        format!("baseline={}/{}\n", config.suite, baseline_name),
        format!("errors={errors_count}\n"),
    ];
    for view in &sorted_views {
        let d = view.diagnostic;
        lines.push(format!(
            "diag={}|{}|{}|{}|{}|{}\n",
            view.file_name,
            d.message_key(),
            d.code(),
            d.pos(),
            d.end(),
            d.message_args().join("\u{1}")
        ));
    }
    PipelineOutcome {
        verdict,
        errors_count,
        shape_hash: shape_hash(&lines),
        lines,
    }
}

/// Go: `referencesRegex = regexp.MustCompile("reference\\spath")` —
/// "reference" + one whitespace + "path".
fn contains_reference_path(content: &str) -> bool {
    let bytes = content.as_bytes();
    const NEEDLE: &[u8] = b"reference";
    for index in 0..bytes.len().saturating_sub(NEEDLE.len()) {
        if &bytes[index..index + NEEDLE.len()] == NEEDLE {
            let after = &bytes[index + NEEDLE.len()..];
            if !after.is_empty() && (after[0] as char).is_whitespace() && after[1..].starts_with(b"path") {
                return true;
            }
        }
    }
    false
}

/// The `(file name, file text)` a `FileDiagnostic` renders against (the
/// program's copy when the file was pulled in, the config text for config
/// parse errors, "" for globals — Go resolves through the source-file
/// pointer graph).
fn file_diagnostic_text(
    file_diagnostic: &FileDiagnostic,
    program: &tsc_compiler::Program,
    config_path: &RootedFilePath,
    config_text: &str,
) -> (String, String) {
    match &file_diagnostic.file_name {
        Some(name) => {
            for file in program.get_source_files() {
                if file.file_name().as_string() == name.as_string() {
                    return (name.as_string().to_string(), file.text().to_string());
                }
            }
            if name.as_string() == config_path.as_string() {
                return (name.as_string().to_string(), config_text.to_string());
            }
            (name.as_string().to_string(), String::new())
        }
        None => (String::new(), String::new()),
    }
}

/// The `(file name, file text, diagnostic)` triple a `FileDiagnostic`
/// renders against — `file_diagnostic_text`'s pairing plus the FCS
/// tranche-4 config-span localization (see `config_option_value_loc`).
fn file_diagnostic_view(
    file_diagnostic: FileDiagnostic,
    program: &tsc_compiler::Program,
    config_path: &RootedFilePath,
    config_text: &str,
) -> (String, String, tsc_ast::Diagnostic) {
    let (name, text) = file_diagnostic_text(&file_diagnostic, program, config_path, config_text);
    let mut diagnostic = file_diagnostic.diagnostic;
    // PORT(FCS, tranche 4): Go's config-route option diagnostics carry the
    // failing property assignment's INITIALIZER loc — tsconfigparsing.go's
    // `onPropertySet` calls `convertJsonOption(option, value, basePath,
    // propertyAssignment, propertyAssignment.Initializer, sourceFile)`, and
    // both the Compiler_option_0_requires_a_value_of_type_1 tail and the
    // convertJsonOptionOfEnumType Argument_for_0_option_must_be arm route
    // through CreateDiagnosticForNodeInSourceFileOrCompilerDiagnostic with
    // that node. The tsoptions port dropped the AST params from the
    // validate/enum helpers ("command-line path only" wave), so those
    // diagnostics arrive fileless with UndefinedTextRange (-1,-1) yet
    // paired to the config path by the ParsedCommandLine concat — a span
    // the baseline's column slice cannot render (the 26-case
    // tsbaseline.rs:273 panic family, full-corpus sweeps). Re-derive Go's
    // loc at this seam from the config AST: the config text is
    // byte-identical to the one Go parsed, so the option's value-node loc
    // is Go's valueExpression loc exactly. The option is the diagnostic's
    // first message arg (Argument_for_0_option_must_be names it
    // `--`-prefixed — Go's `"--" + opt.Name`). No matching property means
    // Go's node was nil — CreateDiagnosticForNodeInSourceFileOrCompiler
    // Diagnostic's own fileless arm — and the diagnostic renders in the
    // global-error block.
    if !name.is_empty() && name == config_path.as_string() && diagnostic.pos() < 0 {
        let option_name = diagnostic
            .message_args()
            .first()
            .map(|name| name.trim_start_matches('-').to_string());
        match option_name
            .as_deref()
            .and_then(|option| config_option_value_loc(config_text, option))
        {
            Some(loc) => diagnostic.set_location(loc),
            None => return (String::new(), String::new(), diagnostic),
        }
    }
    (name, text, diagnostic)
}

/// Go's `valueExpression` loc for the named compiler option in the config
/// text — the property assignment's initializer under `compilerOptions`
/// first, then at the root object (Go's notifier walk visits the
/// `compilerOptions` properties before the root's own). The first
/// name-matching property decides (Go's map lookup takes the parsed
/// object's value).
fn config_option_value_loc(
    config_text: &str,
    option_name: &str,
) -> Option<tsc_core::text::TextRange> {
    if option_name.is_empty() {
        return None;
    }
    // The same JSON parse the config route runs (BOM-free text; the JSON
    // arm of the unit seam strips the BOM before the config is written).
    let source_file = parse_source_file(Default::default(), config_text, ScriptKind::JSON);
    let root = source_file
        .payload()
        .statements
        .as_ref()?
        .nodes
        .first()
        .copied()?;
    let root_expression = tsc_tsoptions::ast_shims::expression_of(&source_file, root)?;
    if !tsc_ast::is_object_literal_expression(source_file.node(root_expression)) {
        return None;
    }
    let mut containers = vec![root_expression];
    if let Some(compiler_options) =
        object_literal_property_value(&source_file, root_expression, "compilerOptions")
    {
        if tsc_ast::is_object_literal_expression(source_file.node(compiler_options)) {
            containers.insert(0, compiler_options);
        }
    }
    for container in containers {
        if let Some(value) = object_literal_property_value(&source_file, container, option_name) {
            return Some(source_file.node(value).loc);
        }
    }
    None
}

/// The value (initializer) node of the FIRST property assignment named
/// `key` in the object literal — Go's `ElementOptions.Get`/notifier walk
/// resolves the parsed property; computed/named non-assignments are
/// skipped (the config walk's own rule).
fn object_literal_property_value(
    source_file: &SourceFile,
    object_literal: NodeId,
    key: &str,
) -> Option<NodeId> {
    let data = source_file
        .node(object_literal)
        .as_object_literal_expression()?
        .clone();
    let properties = data.properties?;
    for property in properties.nodes.iter() {
        let node = source_file.node(*property);
        if !tsc_ast::is_property_assignment(node) {
            continue;
        }
        let name = node.name().expect("PropertyAssignment name");
        if tsc_tsoptions::ast_shims::try_get_text_of_property_name(source_file, name).as_deref()
            == Some(key)
        {
            return Some(node.as_property_assignment().expect("checked above").initializer);
        }
    }
    None
}

/// The verdict mapping (Go `writeComparison` semantics):
///   - content == NoContent: match iff no reference exists (Go's
///     NoContent/missing-reference pass; a present reference is the
///     `.delete` divergence — mismatch at this depth)
///   - content real: missing reference → missing-baseline (the "new
///     baseline created" branch); equal → match; else mismatch
fn verdict_for(content: &str, reference: &Path) -> Verdict {
    if content == tsc_testutil::baseline::NO_CONTENT {
        return if reference.exists() {
            Verdict::Mismatch
        } else {
            Verdict::Match
        };
    }
    if !reference.exists() {
        return Verdict::MissingBaseline;
    }
    match std::fs::read(reference) {
        Ok(bytes) if String::from_utf8_lossy(&bytes) == content => Verdict::Match,
        _ => Verdict::Mismatch,
    }
}

/// The always-miss extended-config cache (Go's nil cache).
fn nil_extended_config_cache() -> ExtendedConfigCache {
    Arc::new(|_: &str| -> Option<tsc_tsoptions::CompilerOptionsValue> { None })
}

/// FNV-1a 64 over the fold lines, lowercase 16-hex (the program twin's
/// recipe).
fn shape_hash(lines: &[String]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for line in lines {
        for b in line.as_bytes() {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// W3T7 PIN (the 2703 spans of the BOM'd await_unary cases): Go's check
    /// twin reads the unit through the map-FS program, and the vfs read
    /// (tsc/internal/vfs/internal's decodeBytes) strips a leading UTF-8 BOM
    /// before the parser sees the text — every check-stage position is
    /// relative to the BOM-LESS text. The seam parses the unit bytes
    /// directly, so it applies the same strip. The pinned span (35..43 of
    /// the BOM-less text) is the Go-twin-verified value — the t3 slice run
    /// collapsed all four BOM'd await_unary records byte-for-byte with this
    /// strip in place.
    #[test]
    fn w3t7_check_seam_strips_utf8_bom() {
        let content = "\u{feff}async function bar1() {\n    delete await 42;\n}\n";
        // PORT(FCS tranche 7): the seam reads the case's parsed config for the
        // per-unit resolution twin — the test's config carries the rooted base
        // directory the twin's module resolver requires (the runner's real
        // configs parse from the case's tsconfig).
        let parsed = tsc_tsoptions::parsedcommandline::ParsedCommandLine::new(
            RootedFilePath::default(),
            src_folder(),
            CaseSensitivity::CaseSensitive,
        );
        let diags = semantic_diagnostics_of_unit(
            "/.src/bomprobe.ts",
            content,
            &CompilerOptions::default(),
            &parsed,
        );
        let d = diags
            .iter()
            .find(|d| d.code() == 2703)
            .expect("the delete operand is an await expression, not a reference");
        assert_eq!(
            (d.pos() as usize, d.end() as usize),
            (35, 43),
            "the span is relative to the BOM-less text; got {:?}",
            diags.iter().map(|d| (d.code(), d.pos(), d.end())).collect::<Vec<_>>()
        );
    }

    #[test]
    fn configured_name_matches_go() {
        assert_eq!(configured_name("foo.ts", ""), "foo.ts");
        assert_eq!(configured_name("foo.ts", "target=es2020"), "foo(target=es2020).ts");
        assert_eq!(configured_name("foo.tsx", "alwaysstrict=true"), "foo(alwaysstrict=true).tsx");
        // Go's GetAnyExtensionFromPath keeps the LAST extension
        assert_eq!(any_extension("foo.d.ts"), ".ts");
        assert_eq!(any_extension("foo"), "");
    }

    #[test]
    fn baseline_file_names_match_the_reference_tree() {
        assert_eq!(baseline_file_name("ArrowFunction1.ts", ""), "ArrowFunction1.errors.txt");
        assert_eq!(
            baseline_file_name("abstractProperty.ts", "target=es2015"),
            "abstractProperty(target=es2015).errors.txt"
        );
        assert_eq!(baseline_file_name("foo.tsx", "alwaysstrict=true"), "foo(alwaysstrict=true).errors.txt");
    }

    #[test]
    fn reference_path_regex_matches_go() {
        assert!(contains_reference_path("/// <reference path=\"./foo.ts\" />"));
        assert!(contains_reference_path("/// <reference\tpath='./foo.ts' />"));
        assert!(!contains_reference_path("/// <referencepath='./foo.ts' />"));
        // Go: referencesRegex = `reference\spath` — a plain "reference path" matches
        assert!(contains_reference_path("reference path"));
    }

    #[test]
    fn stage_flags() {
        assert_eq!(Stage::parse("parse"), Ok(Stage::Parse));
        assert_eq!(Stage::parse("bind"), Ok(Stage::Bind));
        assert_eq!(Stage::parse("check"), Ok(Stage::Check));
        assert!(Stage::parse("other").is_err());
        assert_eq!(Stage::Bind.as_str(), "bind");
        assert_eq!(Stage::Check.as_str(), "check");
    }

    #[test]
    fn shape_hash_matches_the_program_twin_recipe() {
        // published FNV-1a 64 vector ("foobar")
        let mut h: u64 = 0xcbf29ce484222325;
        for b in b"foobar" {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        assert_eq!(h, 0x85944171f73967e8);
        assert_eq!(shape_hash(&[]), format!("{:016x}", 0xcbf29ce484222325u64));
    }

    #[test]
    fn skipped_tests_table_is_go_verbatim() {
        assert_eq!(SKIPPED_TESTS.len(), 42);
        assert!(SKIPPED_TESTS.contains(&"APILibCheck.ts"));
        assert!(SKIPPED_TESTS.contains(&"requireOfJsonFileWithModuleNodeResolutionEmitNone.ts"));
    }
}

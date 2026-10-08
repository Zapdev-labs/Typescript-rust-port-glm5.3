// Port of tsc/internal/testrunner/compiler_runner.go @
// ec47d33c23e464a17cdf2475632cba629bee8763 — the compiler/conformance
// baseline runner, at the DEPTH CONTROLLED stage (SPEC §8 M6).
//
// The Go runner drives the whole compiler; the port drives the pipeline
// only as deep as it exists TODAY (the M5 program layer): per-case tsconfig
// construction → program build (parse) → bind → diagnostics collection at
// the wired depth → `.errors.txt` baseline render → verdict vs
// baselines/reference. The per-case NDJSON record (case / baseline /
// verdict / errors_count / shape_hash) is the conformance TWIN's contract:
// the Go driver (rust/bench/go-driver) mirrors this module byte-for-byte —
// `diff(1)` over the two outputs is the verdict.
//
// DEPTH CONTROL — [`Stage`]: `Parse` (program build only), `Bind` (+
// `bind_source_files`, the default), `Check` (explicitly UNWIRED — no fake
// output; the plug point is Go program.go's check entry points
// `GetSemanticDiagnostics` / `GetSemanticDiagnosticsForIncremental`, whose
// Rust counterpart lands with the checker wave in rust/crates/checker via
// the compiler crate's CheckerPool).
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

/// Go: `srcFolder = tspath.RootedDirectoryPathFromNormalized("/.src")`.
pub fn src_folder() -> RootedDirectoryPath {
    rooted_directory_path_from_normalized("/.src")
}

// ──────────────────────────────────────────────────────────────────────
// Depth control
// ──────────────────────────────────────────────────────────────────────

/// The pipeline depth the runner drives (SPEC §8 M6's depth control).
///
/// `Check` is the explicit unwired boundary: asking for it is an error (no
/// fake output). The plug point is the Go program layer's check entry points
/// (`GetSemanticDiagnostics` / `GetSemanticDiagnosticsForIncremental` in
/// tsc/internal/compiler/program.go) — the Rust counterpart lands with the
/// checker wave (rust/crates/checker) through the compiler crate's
/// CheckerPool.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Stage {
    /// Program construction only (parse).
    Parse,
    /// Program construction + `bind_source_files` — today's wired depth.
    #[default]
    Bind,
    /// The check stage — NOT WIRED YET (see the type docs).
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
            "check" => Err(CHECK_NOT_WIRED.to_string()),
            other => Err(format!("unknown stage {other:?} (parse|bind)")),
        }
    }
}

const CHECK_NOT_WIRED: &str = "the check stage is not wired yet: the checker wave (rust/crates/checker) \
plugs in at the compiler layer's check entry points (Go program.go GetSemanticDiagnostics / \
GetSemanticDiagnosticsForIncremental); no fake check output";

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
/// (case, configuration). Returns the record count, or the depth-control
/// error for [`Stage::Check`].
pub fn run_conformance(config: &ConformanceConfig, mut emit: impl FnMut(&CaseRecord)) -> Result<usize, String> {
    if config.stage == Stage::Check {
        return Err(CHECK_NOT_WIRED.to_string());
    }
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
    let host = new_compiler_host(
        Arc::<dyn Fs>::clone(&fs),
        tsc_bundled::lib_path(),
        nil_extended_config_cache(),
        None,
    );
    let mut program = new_program(ProgramOptions {
        config: ProgramConfig {
            config: Arc::new(parsed),
            ..Default::default()
        },
        hosts: ProgramHosts { host: Some(host) },
        factories: ProgramFactories::default(),
    });
    if config.stage == Stage::Bind {
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
        let (name, text) = file_diagnostic_text(&file_diagnostic, &program, &config_path, &config_text);
        owned.push((name, text, file_diagnostic.diagnostic));
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
        let (name, text) = file_diagnostic_text(&file_diagnostic, &program, &config_path, &config_text);
        owned.push((name, text, file_diagnostic.diagnostic));
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
        assert!(Stage::parse("check").unwrap_err().contains("check stage is not wired"));
        assert!(Stage::parse("other").is_err());
        assert_eq!(Stage::Bind.as_str(), "bind");
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

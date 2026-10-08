// Port of tsc/internal/tsoptions/parsedcommandline.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT notes (M4 scope):
//   - The watch/parse-status machinery (`ParsedCommandLineWatcher`,
//     `changedFiles` invalidation, `ChangeKind`) and the build-info cache
//     accessors stay unported until the watch/build waves need them; the
//     struct carries the fields the --showConfig surface consumes.
//   - Go's lazy caches (`getFileNames...`) are built eagerly here where
//     cheap, or left as the raw parsed values (file enumeration is not part
//     of this wave — see tsconfigparsing.rs PORT notes).

use tsc_ast::Diagnostic;
use tsc_core::options_generated::{BuildOptions, CompilerOptions, TypeAcquisition};
use tsc_tspath::{CaseSensitivity, PathKey, PathPattern, RootedDirectoryPath, RootedFilePath, RootedPath};

use crate::tsconfigparsing::{ConfigFileSpecs, ParsedCompilerOptions, ProjectReference, TsConfigSourceFile};

/// Go: `type ParsedCommandLine struct` (the fields the M4 surface consumes).
#[derive(Clone, Debug, Default)]
pub struct ParsedCommandLine {
    /// Go: `options *parsedCompilerOptions`.
    pub compiler_options: ParsedCompilerOptions,
    /// Go: `typeAcquisition *core.TypeAcquisition`.
    pub type_acquisition: TypeAcquisition,
    /// Go: `buildOptions *core.BuildOptions`.
    pub build_options: BuildOptions,
    /// Go: `fileNames []tspath.RootedFilePath` — the glob-enumerated + literal
    /// file list (getFileNamesFromConfigSpecs; ported with the showconfig
    /// parity twin — see tsconfigparsing.rs PORT notes).
    pub(crate) file_names: Vec<RootedFilePath>,
    /// Go: `baseDirectory tspath.RootedDirectoryPath`.
    pub base_directory: RootedDirectoryPath,
    /// Go: `caseSensitive tspath.CaseSensitivity`.
    pub case_sensitivity: CaseSensitivity,
    /// Go: `projectReferences []*core.ProjectReference`.
    pub project_references: Vec<ProjectReference>,
    /// Go: `compileOnSave *bool`.
    pub compile_on_save: Option<bool>,
    /// Go: `configFileName tspath.RootedFilePath`.
    pub config_file_name: RootedFilePath,
    /// Go: `configFileSpecs.validatedIncludeSpecs` — post-${configDir}
    /// substitution; `None` mirrors Go's nil slice (include absent).
    pub(crate) validated_include_specs: Option<Vec<PathPattern>>,
    /// Go: `configFileSpecs.validatedExcludeSpecs` (`None` = absent).
    pub(crate) validated_exclude_specs: Option<Vec<PathPattern>>,
    /// Go: `configFileSpecs.validatedFilesSpec` (post-substitution).
    pub(crate) validated_files_specs: Vec<tsc_tspath::FileSpec>,
    /// Go: `configFileSpecs.validatedFileNames` — the files-list entries
    /// resolved against the config directory (literal files are included
    /// verbatim with no existence check, exactly as in Go).
    pub(crate) validated_file_names: Vec<RootedFilePath>,
    /// Go: `configFileSpecs.validatedFilesSpecBeforeSubstitution` — the
    /// `files` entries as written (pre-${configDir}); the matched-file-spec
    /// include reasons report these (M6 config-node wave).
    pub(crate) validated_files_specs_before_substitution: Vec<tsc_tspath::FileSpec>,
    /// Go: `configFileSpecs.validatedIncludeSpecsBeforeSubstitution` — the
    /// include entries as written (pre-${configDir}).
    pub(crate) validated_include_specs_before_substitution: Vec<PathPattern>,
    /// Go: `configFileSpecs.fileSpecByPath` — each files-list entry's path
    /// → the spec as written (first occurrence wins, Go's `if !exists`).
    pub(crate) file_spec_by_path: rustc_hash::FxHashMap<PathKey, tsc_tspath::FileSpec>,
    /// Go: `ConfigFile *TsConfigSourceFile` — the config file parsed as a
    /// JSON SourceFile; `None` mirrors Go's nil ConfigFile (config-node
    /// diagnostics degrade to compiler diagnostics, Go's nil arm). PORT:
    /// populated on the `GetParsedCommandLineOfConfigFilePath` route only
    /// (the runner/program seam); direct `ParseJsonConfigFileContent`
    /// callers keep `None` until the full AST-config conversion wave lands
    /// (tsconfigparsing.rs PORT notes).
    pub config_file: Option<std::sync::Arc<TsConfigSourceFile>>,
    /// Go: `configFileSpecs.includeSpecs`/`excludeSpecs` — the raw spec
    /// values as merged into the config (extends rewrite included), kept
    /// for the No-inputs diagnostic's JSON arguments.
    pub(crate) raw_include_spec_values: Vec<String>,
    pub(crate) raw_exclude_spec_values: Vec<String>,
    /// Go: `configFileSpecs.isDefaultIncludeSpec`.
    pub(crate) is_default_include_spec: bool,
    /// Go: `errors []*ast.Diagnostic`.
    pub errors: Vec<Diagnostic>,
    /// Go: `raw any` — the option map the command-line parser produced
    /// (tsconfig parsing leaves it zero).
    pub raw: tsc_collections::OrderedMap<String, crate::commandlineoption::CompilerOptionsValue>,
    /// Go: `rootFileNamesForDiagnostics []string` — the un-rooted file name
    /// strings as they appeared on the command line.
    pub root_file_names_for_diagnostics: Vec<String>,
    /// Go: `wildcardDirectories` (see wildcarddirectories.rs; empty in M4).
    pub wildcard_directories: Vec<(RootedPath, crate::wildcarddirectories::WildcardDirectoryWatcher)>,
}

impl ParsedCommandLine {
    /// The Go zero-ParsedCommandLine shape, with the directories wired.
    pub fn new(
        config_file_name: RootedFilePath,
        base_directory: RootedDirectoryPath,
        case_sensitivity: CaseSensitivity,
    ) -> Self {
        ParsedCommandLine {
            compiler_options: ParsedCompilerOptions::new(CompilerOptions::default()),
            base_directory,
            case_sensitivity,
            config_file_name,
            ..ParsedCommandLine::default()
        }
    }

    /// Go: `func (c *ParsedCommandLine) CompilerOptions() *core.CompilerOptions`.
    pub fn compiler_options(&self) -> &CompilerOptions {
        &self.compiler_options.options
    }

    /// Go: `func (c *ParsedCommandLine) FileNames()`.
    pub fn file_names(&self) -> &[RootedFilePath] {
        &self.file_names
    }

    /// Go: `func (c *ParsedCommandLine) ConfigName() tspath.RootedFilePath` —
    /// the config source file's name ("" without a config file).
    pub fn config_name(&self) -> RootedFilePath {
        match &self.config_file {
            Some(config_file) => config_file.source_file.file_name().clone(),
            None => RootedFilePath::default(),
        }
    }

    /// Go: `func (p *ParsedCommandLine) GetMatchedFileSpec(fileName) string` —
    /// the `files`-list spec that matched `file_name` (the spec AS WRITTEN —
    /// the pre-${configDir} substitution form), "" when the file is not a
    /// files-list entry.
    pub fn get_matched_file_spec(&self, file_name: &RootedFilePath) -> tsc_tspath::FileSpec {
        self.file_spec_by_path
            .get(&self.case_sensitivity.path_key(&file_name.as_path()))
            .cloned()
            .unwrap_or_default()
    }

    /// Go: `func (p *ParsedCommandLine) GetMatchedIncludeSpec(fileName) (string, bool)` —
    /// the include pattern that matched `file_name` plus whether it is the
    /// default `["**/*"]` spec; the returned pattern is the AS-WRITTEN
    /// (pre-${configDir}) form, exactly Go's
    /// `validatedIncludeSpecsBeforeSubstitution[index]`.
    pub fn get_matched_include_spec(&self, file_name: &RootedFilePath) -> (PathPattern, bool) {
        if self.validated_include_specs.is_empty() {
            return (PathPattern::default(), false);
        }
        if self.is_default_include_spec {
            return (self.validated_include_specs[0].clone(), true);
        }
        // Go: `specs.getMatchedIncludeSpec(fileName, p.baseDirectory, p.caseSensitivity)` —
        // per-spec SpecMatcher; the matched index reads the
        // before-substitution list.
        for (index, spec) in self.validated_include_specs.iter().enumerate() {
            let include_matcher = tsc_module::new_spec_matcher(
                &[spec.as_string().to_owned()],
                self.base_directory(),
                tsc_module::Usage::Files,
                self.case_sensitivity,
            );
            // Go: `includeMatcher != nil && includeMatcher.MatchFileName(fileName)`.
            if include_matcher.is_some_and(|matcher| matcher.match_file_name(file_name)) {
                return (self.validated_include_specs_before_substitution[index].clone(), false);
            }
        }
        (PathPattern::default(), false)
    }

    /// Go: `func (c *ParsedCommandLine) ProjectReferences()`.
    pub fn project_references(&self) -> &[ProjectReference] {
        &self.project_references
    }

    /// Go: `func (c *ParsedCommandLine) getConfigFileSpecs() *configFileSpecs`.
    ///
    /// The worker-computed validated specs (see tsconfigparsing.rs); the
    /// include list is `["**/*"]` exactly when the default include was
    /// applied (Go: `isDefaultIncludeSpec`).
    pub fn config_file_specs(&self) -> Option<ConfigFileSpecs> {
        Some(ConfigFileSpecs {
            validated_include_specs: self
                .validated_include_specs
                .clone()
                .unwrap_or_default(),
            validated_exclude_specs: self
                .validated_exclude_specs
                .clone()
                .unwrap_or_default(),
            validated_files_specs: self.validated_files_specs.clone(),
            validated_file_names: self.validated_file_names.clone(),
            validated_files_specs_before_substitution: self
                .validated_files_specs_before_substitution
                .clone(),
            validated_include_specs_before_substitution: self
                .validated_include_specs_before_substitution
                .clone(),
            file_spec_by_path: self.file_spec_by_path.clone(),
            is_default_include_spec: self.is_default_include_spec,
        })
    }

    /// Go: `func (c *ParsedCommandLine) BaseDirectory()`.
    pub fn base_directory(&self) -> &RootedDirectoryPath {
        &self.base_directory
    }
}

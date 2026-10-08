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
use tsc_tspath::{CaseSensitivity, PathPattern, RootedDirectoryPath, RootedFilePath, RootedPath};

use crate::tsconfigparsing::{ConfigFileSpecs, ParsedCompilerOptions, ProjectReference};

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
            is_default_include_spec: self.is_default_include_spec,
        })
    }

    /// Go: `func (c *ParsedCommandLine) BaseDirectory()`.
    pub fn base_directory(&self) -> &RootedDirectoryPath {
        &self.base_directory
    }
}

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

use crate::tsconfigparsing::{ConfigFileSpecs, ParsedCompilerOptions, ProjectReference, DEFAULT_INCLUDE_SPEC};

/// Go: `type ParsedCommandLine struct` (the fields the M4 surface consumes).
#[derive(Clone, Debug, Default)]
pub struct ParsedCommandLine {
    /// Go: `options *parsedCompilerOptions`.
    pub compiler_options: ParsedCompilerOptions,
    /// Go: `typeAcquisition *core.TypeAcquisition`.
    pub type_acquisition: TypeAcquisition,
    /// Go: `buildOptions *core.BuildOptions`.
    pub build_options: BuildOptions,
    /// Go: `fileNames []tspath.RootedFilePath` — empty in the M4 wave (file
    /// enumeration is the glob/vfs wave; see tsconfigparsing.rs PORT notes).
    file_names: Vec<RootedFilePath>,
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
    /// The raw validated include/exclude/files specs (Go: `configFileSpecs`
    /// is built during parse; the port defers to `config_file_specs()`).
    pub(crate) raw_include_specs: Option<Vec<PathPattern>>,
    pub(crate) raw_exclude_specs: Option<Vec<PathPattern>>,
    pub(crate) raw_files_specs: Vec<PathPattern>,
    /// Go: `errors []*ast.Diagnostic`.
    pub errors: Vec<Diagnostic>,
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
    /// Builds the validated specs view: include defaults to `["**/*"]`
    /// exactly as in Go's parse worker when neither files nor include are
    /// present.
    pub fn config_file_specs(&self) -> Option<ConfigFileSpecs> {
        let has_files = !self.raw_files_specs.is_empty();
        let (validated_include_specs, is_default_include_spec) = match &self.raw_include_specs {
            Some(specs) => (specs.clone(), false),
            None if !has_files => (vec![PathPattern::from(DEFAULT_INCLUDE_SPEC)], true),
            None => (Vec::new(), false),
        };
        Some(ConfigFileSpecs {
            validated_include_specs,
            validated_exclude_specs: self
                .raw_exclude_specs
                .clone()
                .unwrap_or_default(),
            validated_files_specs: Vec::new(),
            is_default_include_spec,
        })
    }

    /// Go: `func (c *ParsedCommandLine) BaseDirectory()`.
    pub fn base_directory(&self) -> &RootedDirectoryPath {
        &self.base_directory
    }
}

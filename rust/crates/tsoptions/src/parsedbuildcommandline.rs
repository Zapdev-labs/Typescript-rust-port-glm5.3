// Port of tsc/internal/tsoptions/parsedbuildcommandline.go @ ec47d33c23e464a17cdf2475632cba629bee8763

use std::sync::OnceLock;

use tsc_ast::Diagnostic;
use tsc_collections::OrderedMap;
use tsc_core::options_generated::{BuildOptions, CompilerOptions};
use tsc_tspath::RootedDirectoryPath;

use crate::commandlineoption::CompilerOptionsValue;

/// Go: `type ParsedBuildCommandLine struct`.
pub struct ParsedBuildCommandLine {
    /// Go: `BuildOptions *core.BuildOptions`.
    pub build_options: BuildOptions,
    /// Go: `CompilerOptions *core.CompilerOptions`.
    pub compiler_options: CompilerOptions,
    /// Go: `Projects []string`.
    pub projects: Vec<String>,
    /// Go: `Errors []*ast.Diagnostic`.
    pub errors: Vec<Diagnostic>,
    /// Go: `Raw any` — the parsed option map (closed value model).
    pub raw: OrderedMap<String, CompilerOptionsValue>,
    /// Go: `currentDirectory tspath.RootedDirectoryPath`.
    pub current_directory: RootedDirectoryPath,

    /// Go: `resolvedProjectPaths` + `sync.Once`.
    resolved_project_paths: OnceLock<Vec<tsc_tspath::RootedFilePath>>,
    /// Go: `locale locale.Locale` + `sync.Once`.
    locale: OnceLock<tsc_locale::Locale>,
}

impl ParsedBuildCommandLine {
    /// The `&ParsedBuildCommandLine{...}` constructor shape (the two `Once`
    /// caches start cold, exactly as Go's `sync.Once` fields do).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        build_options: BuildOptions,
        compiler_options: CompilerOptions,
        projects: Vec<String>,
        errors: Vec<Diagnostic>,
        raw: OrderedMap<String, CompilerOptionsValue>,
        current_directory: RootedDirectoryPath,
    ) -> Self {
        ParsedBuildCommandLine {
            build_options,
            compiler_options,
            projects,
            errors,
            raw,
            current_directory,
            resolved_project_paths: OnceLock::new(),
            locale: OnceLock::new(),
        }
    }

    /// Go: `func (p *ParsedBuildCommandLine) ResolvedProjectPaths() []tspath.RootedFilePath`.
    ///
    /// PORT: Go resolves each project via
    /// `core.ResolveConfigFileNameOfProjectReference` (which knows the
    /// tsconfig.json/*.json fallback naming); the core helper is not in the
    /// Rust core crate yet, so the path is rooted against the current
    /// directory the same way. Swap on the core wave (PORTING-NOTES).
    pub fn resolved_project_paths(&self) -> &[tsc_tspath::RootedFilePath] {
        self.resolved_project_paths.get_or_init(|| {
            self.projects
                .iter()
                .map(|project| self.current_directory.resolve_file(project))
                .collect()
        })
    }

    /// Go: `func (p *ParsedBuildCommandLine) Locale() locale.Locale`.
    ///
    /// PORT: Go returns `locale.Locale` by value; the port returns the
    /// cached slot's reference (Locale is not Copy in the tsc-locale port).
    pub fn locale(&self) -> &tsc_locale::Locale {
        self.locale
            .get_or_init(|| tsc_locale::parse(&self.compiler_options.locale).0)
    }
}

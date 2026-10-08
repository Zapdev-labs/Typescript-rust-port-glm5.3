// Ported from tsc/internal/compiler/emitHost.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT notes:
//   - Go's `EmitHost` embeds `printer.EmitHost` + `declarations.DeclarationEmitHost`
//     (the M7 emit machinery: printer, sourcemap, transformers, the
//     declarations transform). Those crates are not ported, so the trait
//     keeps only the program-level surface the emit pipeline reads; the
//     printer/declarations halves (`NewEmitResolver`, `GetOutputPathsFor`,
//     `GetSymlinkCache`, the `EmitTextWriter` construction) land with the M7
//     emit wave (PORTING-NOTES).
//   - Go's `newEmitHost(ctx, program, file)` returns `(host, done)` where
//     `done` releases the checker acquired by `GetTypeCheckerForFile`. The
//     checker crate is a parallel wave (see checkerpool.rs), so the port
//     returns the host alone (there is nothing to release yet).
//   - NOTE (Go): EmitHost operations must be thread-safe.

use std::sync::Arc;

use tsc_ast::{FileReference, SourceFile};
use tsc_core::compileroptions::ResolutionMode;
use tsc_core::options_generated::CompilerOptions;
use tsc_module::ResolvedModule;
use tsc_tspath::{CaseSensitivity, PathKey, RootedDirectoryPath, RootedFilePath};

use crate::project_reference_file_mapper::SourceOutputAndProjectReference;

/// Go: `type EmitHost interface` — the program-level surface (the
/// printer/declarations halves are the M7 boundary; see the file header).
pub trait EmitHost {
    /// Go: `Options() *core.CompilerOptions`.
    fn options(&self) -> &CompilerOptions;
    /// Go: `SourceFiles() []*ast.SourceFile`.
    fn source_files(&self) -> &[SourceFile];
    /// Go: `CaseSensitivity() tspath.CaseSensitivity`.
    fn case_sensitivity(&self) -> CaseSensitivity;
    /// Go: `BaseDirectory() tspath.RootedDirectoryPath`.
    fn base_directory(&self) -> RootedDirectoryPath;
    /// Go: `CommonSourceDirectory() tspath.RootedDirectoryPath`.
    fn common_source_directory(&self) -> RootedDirectoryPath;
    /// Go: `IsEmitBlocked(file tspath.RootedFilePath) bool`.
    fn is_emit_blocked(&self, file: &RootedFilePath) -> bool;
    /// Go: `SourceFileMayBeEmitted(file *ast.SourceFile, forceDtsEmit bool) bool`.
    fn source_file_may_be_emitted(&self, file: &SourceFile, force_dts_emit: bool) -> bool;
    /// Go: `GetModeForUsageLocation(file, moduleSpecifier) core.ResolutionMode`.
    fn get_mode_for_usage_location(&self, file: &SourceFile, module_specifier: tsc_ast::NodeId) -> ResolutionMode;
    /// Go: `GetResolvedModuleFromModuleSpecifier(file, moduleSpecifier) *module.ResolvedModule`.
    fn get_resolved_module_from_module_specifier(
        &self,
        file: &SourceFile,
        module_specifier: tsc_ast::NodeId,
    ) -> Option<Arc<ResolvedModule>>;
    /// Go: `GetDefaultResolutionModeForFile(file) core.ResolutionMode`.
    fn get_default_resolution_mode_for_file(&self, file: &SourceFile) -> ResolutionMode;
    /// Go: `GetEmitModuleFormatOfFile(file) core.ModuleKind`.
    fn get_emit_module_format_of_file(&self, file: &SourceFile) -> tsc_core::compileroptions::ModuleKind;
    /// Go: `FileExists(path tspath.RootedFilePath) bool`.
    fn file_exists(&self, path: &RootedFilePath) -> bool;
    /// Go: `GetGlobalTypingsCacheLocation() tspath.RootedDirectoryPath`.
    fn get_global_typings_cache_location(&self) -> RootedDirectoryPath;
    /// Go: `GetNearestAncestorDirectoryWithPackageJson(dirname)`.
    fn get_nearest_ancestor_directory_with_package_json(
        &self,
        dirname: &RootedDirectoryPath,
    ) -> RootedDirectoryPath;
    /// Go: `GetSourceOfProjectReferenceIfOutputIncluded(file)`.
    fn get_source_of_project_reference_if_output_included(
        &self,
        file: &SourceFile,
    ) -> RootedFilePath;
    /// Go: `GetProjectReferenceFromSource(path tspath.PathKey)`.
    fn get_project_reference_from_source(
        &self,
        path: &PathKey,
    ) -> Option<Arc<SourceOutputAndProjectReference>>;
    /// Go: `GetRedirectTargets(path tspath.PathKey) []tspath.RootedFilePath`.
    fn get_redirect_targets(&self, path: &PathKey) -> &[RootedFilePath];
    /// Go: `GetSourceFileFromReference(origin, ref) *ast.SourceFile`.
    fn get_source_file_from_reference<'a>(
        &'a self,
        origin: &'a SourceFile,
        r#ref: &'a FileReference,
    ) -> Option<&'a SourceFile>;
    /// Go: `IsSourceFileFromExternalLibrary(file) bool`.
    fn is_source_file_from_external_library(&self, file: &SourceFile) -> bool;
    /// Go: `WriteFile(fileName, text) error` — `host.program.Host().FS().WriteFile`.
    fn write_file(&self, file_name: &RootedFilePath, text: &str) -> std::io::Result<()>;
}

/// Go: `type emitHost struct`.
pub struct EmitHostImpl<'a> {
    /// Go: `program *Program` + `newEmitResolver func(...)` (the resolver
    /// half is the M7 printer boundary — see the file header).
    pub program: &'a crate::program::Program,
}

/// Go: `func newEmitHost(ctx, program, file) (*emitHost, func())`.
///
/// PORT: the checker-release `done` function is dropped (the checker crate
/// is a parallel wave — see the file header).
pub fn new_emit_host<'a>(
    program: &'a crate::program::Program,
    _file: &SourceFile,
) -> EmitHostImpl<'a> {
    EmitHostImpl { program }
}

impl EmitHost for EmitHostImpl<'_> {
    fn options(&self) -> &CompilerOptions {
        self.program.options()
    }

    fn source_files(&self) -> &[SourceFile] {
        self.program.source_files()
    }

    fn case_sensitivity(&self) -> CaseSensitivity {
        self.program.case_sensitivity
    }

    fn base_directory(&self) -> RootedDirectoryPath {
        self.program.base_directory().clone()
    }

    fn common_source_directory(&self) -> RootedDirectoryPath {
        self.program.common_source_directory()
    }

    fn is_emit_blocked(&self, file: &RootedFilePath) -> bool {
        self.program.is_emit_blocked(file)
    }

    fn source_file_may_be_emitted(&self, file: &SourceFile, force_dts_emit: bool) -> bool {
        // Go: sourceFileMayBeEmitted(file, host, forceDtsEmit, false).
        crate::emitter::source_file_may_be_emitted(
            file,
            self.program,
            force_dts_emit,
            false,
        )
    }

    fn get_mode_for_usage_location(
        &self,
        file: &SourceFile,
        module_specifier: tsc_ast::NodeId,
    ) -> ResolutionMode {
        self.program.get_mode_for_usage_location(file, module_specifier)
    }

    fn get_resolved_module_from_module_specifier(
        &self,
        file: &SourceFile,
        module_specifier: tsc_ast::NodeId,
    ) -> Option<Arc<ResolvedModule>> {
        self.program
            .get_resolved_module_from_module_specifier(file, module_specifier)
            .cloned()
    }

    fn get_default_resolution_mode_for_file(&self, file: &SourceFile) -> ResolutionMode {
        self.program.get_default_resolution_mode_for_file(file)
    }

    fn get_emit_module_format_of_file(
        &self,
        file: &SourceFile,
    ) -> tsc_core::compileroptions::ModuleKind {
        self.program.get_emit_module_format_of_file(file)
    }

    fn file_exists(&self, path: &RootedFilePath) -> bool {
        self.program.host().fs().file_exists(path)
    }

    fn get_global_typings_cache_location(&self) -> RootedDirectoryPath {
        self.program.get_global_typings_cache_location().clone()
    }

    fn get_nearest_ancestor_directory_with_package_json(
        &self,
        dirname: &RootedDirectoryPath,
    ) -> RootedDirectoryPath {
        self.program
            .get_nearest_ancestor_directory_with_package_json(dirname)
    }

    fn get_source_of_project_reference_if_output_included(
        &self,
        file: &SourceFile,
    ) -> RootedFilePath {
        self.program
            .get_source_of_project_reference_if_output_included(file.path_key(), file.file_name())
    }

    fn get_project_reference_from_source(
        &self,
        path: &PathKey,
    ) -> Option<Arc<SourceOutputAndProjectReference>> {
        self.program.get_project_reference_from_source(path)
    }

    fn get_redirect_targets(&self, path: &PathKey) -> &[RootedFilePath] {
        self.program.get_redirect_targets(path)
    }

    fn get_source_file_from_reference<'a>(
        &'a self,
        origin: &'a SourceFile,
        r#ref: &'a FileReference,
    ) -> Option<&'a SourceFile> {
        self.program.get_source_file_from_reference(origin, r#ref)
    }

    fn is_source_file_from_external_library(&self, file: &SourceFile) -> bool {
        self.program.is_source_file_from_external_library(file)
    }

    fn write_file(&self, file_name: &RootedFilePath, text: &str) -> std::io::Result<()> {
        self.program.host().fs().write_file(file_name, text)
    }
}

// Ported from tsc/internal/compiler/emitter.go @ ec47d33c23e464a17cdf2475632cba629bee8763
// — the program-level bits only:
//   EmitOnly, SourceFileMayBeEmittedHost, sourceFileMayBeEmitted,
//   getSourceFilesToEmit, isSourceFileNotJson.
//
// The emit machinery itself is the M7 boundary (see lib.rs): `emitter`,
// `emit()`, `emitJSFile`, `emitDeclarationFile`, `printSourceFile`,
// `getScriptTransformers`/`getDeclarationTransformers` (printer,
// sourcemap, transformers, declarations crates), `getDeclarationDiagnostics`,
// and `Program.Emit` are not ported in M5. The `EmitOptions`/`EmitResult`
// result shapes live in program.rs (Go: program.go) so the boundary types
// exist for the orchestrator surface.

use std::sync::Arc;

use tsc_ast::SourceFile;
use tsc_core::options_generated::CompilerOptions;
use tsc_tspath::{CaseSensitivity, PathKey, RootedDirectoryPath};

use crate::project_reference_file_mapper::SourceOutputAndProjectReference;

/// Go: `type EmitOnly byte` + the iota consts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum EmitOnly {
    /// Go: `EmitAll EmitOnly = iota`.
    #[default]
    All = 0,
    /// Go: `EmitOnlyJs`.
    OnlyJs = 1,
    /// Go: `EmitOnlyDts`.
    OnlyDts = 2,
    /// Go: `EmitOnlyBuilderSignature`.
    OnlyBuilderSignature = 3,
}

/// Go: `type SourceFileMayBeEmittedHost interface`.
pub trait SourceFileMayBeEmittedHost {
    /// Go: `Options() *core.CompilerOptions`.
    fn options(&self) -> &CompilerOptions;
    /// Go: `GetProjectReferenceFromSource(path tspath.PathKey) *tsoptions.SourceOutputAndProjectReference`.
    ///
    /// PORT: Go's nil-able struct pointer becomes the crate's
    /// `Arc<SourceOutputAndProjectReference>` (the tsoptions shape is
    /// crate-local — see project_reference_file_mapper.rs).
    fn get_project_reference_from_source(
        &self,
        path: &PathKey,
    ) -> Option<Arc<SourceOutputAndProjectReference>>;
    /// Go: `IsSourceFileFromExternalLibrary(file *ast.SourceFile) bool`.
    fn is_source_file_from_external_library(&self, file: &SourceFile) -> bool;
    /// Go: `BaseDirectory() tspath.RootedDirectoryPath`.
    fn base_directory(&self) -> RootedDirectoryPath;
    /// Go: `CaseSensitivity() tspath.CaseSensitivity`.
    fn case_sensitivity(&self) -> CaseSensitivity;
    /// Go: `SourceFiles() []*ast.SourceFile`.
    fn source_files(&self) -> &[SourceFile];
}

/// Go: `func sourceFileMayBeEmitted(sourceFile, host, forceDtsEmit, forceJsEmit) bool`.
pub fn source_file_may_be_emitted(
    source_file: &SourceFile,
    host: &dyn SourceFileMayBeEmittedHost,
    force_dts_emit: bool,
    force_js_emit: bool,
) -> bool {
    // TODO: move this to outputpaths?
    let options = host.options();
    // Js files are emitted only if option is enabled
    if !force_js_emit
        && options.no_emit_for_js_files.is_true()
        && crate::ast_shims::is_source_file_js_by_file(source_file)
    {
        return false;
    }

    // Declaration files are not emitted
    if source_file.payload().is_declaration_file {
        return false;
    }

    // Runtime output for content-mapped files is owned by the external content mapper or build tool. Only
    // include them in the emit set when their transformed TypeScript can produce declarations.
    //
    // PORT: the content-mapper seam is empty in this wave (Go's zero-config
    // case: `sourceFile.ContentMapper()` is ""), so the check degrades to
    // `"" != "" && ...` (always false) — reintroduce it with the
    // contentmapper wave (PORTING-NOTES).
    if !force_dts_emit && !options.get_emit_declarations() {
        // (the content-mapper arm — see above)
    }

    // Source file from node_modules are not emitted
    if host.is_source_file_from_external_library(source_file) {
        return false;
    }

    // forcing dts emit => file needs to be emitted
    if force_dts_emit || force_js_emit {
        return true;
    }

    // Check other conditions for file emit
    // Source files from referenced projects are not emitted
    if host
        .get_project_reference_from_source(source_file.path_key())
        .is_some()
    {
        return false;
    }

    // Any non json file should be emitted
    if !crate::ast_shims::is_json_source_file(source_file) {
        return true;
    }

    // Json file is not emitted if outDir is not specified
    if options.out_dir.as_string().is_empty() {
        return false;
    }

    // Otherwise, if rootDir is specified or a config file exists, we know the common source directory and can check if the file would be emitted in the same location
    if !options.root_dir.as_string().is_empty()
        || !options.config_file_path.as_string().is_empty()
    {
        let common_dir = tsc_outputpaths::get_common_source_directory(
            options,
            &Vec::new,
            &host.base_directory(),
            host.case_sensitivity(),
            None,
        );
        let output_path = tsc_outputpaths::get_source_file_name_in_new_dir(
            source_file.file_name(),
            &options.out_dir,
            &common_dir,
            host.case_sensitivity(),
        );
        if host.case_sensitivity().compare_file_paths(source_file.file_name(), &output_path) == 0
        {
            return false;
        }
    }

    true
}

/// Go: `func getSourceFilesToEmit(host, targetSourceFiles, forceDtsEmit, forceJsEmit) []*ast.SourceFile`.
///
/// PORT: Go's `targetSourceFiles == nil` (all files) vs a non-nil empty list
/// (no files) collapses to `None` (all files) vs `Some(&[])` here.
pub fn get_source_files_to_emit<'a>(
    host: &'a dyn SourceFileMayBeEmittedHost,
    target_source_files: Option<&'a [&'a SourceFile]>,
    force_dts_emit: bool,
    force_js_emit: bool,
) -> Vec<&'a SourceFile> {
    let all: Vec<&SourceFile> = match target_source_files {
        None => host.source_files().iter().collect(),
        Some(target_source_files) => target_source_files.to_vec(),
    };
    all.into_iter()
        .filter(|source_file| {
            source_file_may_be_emitted(source_file, host, force_dts_emit, force_js_emit)
        })
        .collect()
}

/// Go: `func isSourceFileNotJson(file *ast.SourceFile) bool`.
pub fn is_source_file_not_json(file: &SourceFile) -> bool {
    !crate::ast_shims::is_json_source_file(file)
}

// Ported from tsc/internal/compiler/host.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT notes:
//   - Go interface values (`CompilerHost`) become object-safe traits used via
//     `Arc<dyn CompilerHost>` (the vfs crate's `Arc<dyn Fs>` precedent).
//   - `GetContentMappedSourceFiles` / `ContentMapperProject` depend on the
//     contentmapper Host machinery (`contentmapper.Project`,
//     `TransformAndParse`, `CheckSupplementalFileNameCollisions`), which is
//     not ported (the contentmapper crate header defers it). The trait keeps
//     the seam surface minimal: `content_mapper_extensions` reports the
//     configured extensions (empty in the Rust port until that wave lands)
//     and content-mapper paths degrade to plain loads — Go's zero-config
//     behavior. Reintroduce the two methods with that port (PORTING-NOTES).
//   - `NewCachedFSCompilerHost` is not ported: cachedvfs is not ported (see
//     crates/vfs/src/lib.rs). Callers wrap with `new_compiler_host` on the
//     fs they have; a caching layer slots in the same constructor when the
//     vfs wave adds it.

use std::sync::Arc;

use tsc_ast::{SourceFile, SourceFileParseOptions};
use tsc_core::scriptkind::ScriptKind;
use tsc_diagnostics::Message;
use tsc_tsoptions::ParsedCommandLine;
use tsc_tspath::{RootedDirectoryPath, RootedFilePath, RootedPath};
use tsc_vfs::Fs;

/// Go: `type CompilerHost interface`.
pub trait CompilerHost: Send + Sync {
    /// Go: `FS() vfs.FS`.
    fn fs(&self) -> Arc<dyn Fs>;

    /// Go: `DefaultLibraryPath() tspath.RootedDirectoryPath`.
    fn default_library_path(&self) -> RootedDirectoryPath;

    /// Go: `Trace(msg *diagnostics.Message, args ...any)`.
    fn trace(&self, message: &Message, args: &[String]);

    /// Go: `GetSourceFile(opts ast.SourceFileParseOptions) *ast.SourceFile`.
    fn get_source_file(&self, opts: &SourceFileParseOptions) -> Option<SourceFile>;

    /// Go: `GetResolvedProjectReference(fileName, path) *tsoptions.ParsedCommandLine`.
    fn get_resolved_project_reference(
        &self,
        file_name: &RootedFilePath,
        path: &tsc_tspath::PathKey,
    ) -> Option<Arc<ParsedCommandLine>>;
}

/// Go: `type compilerHost struct`.
pub struct CompilerHostImpl {
    fs: Arc<dyn Fs>,
    default_library_path: RootedDirectoryPath,
    extended_config_cache: tsc_tsoptions::ExtendedConfigCache,
    trace: TraceFn,
}

/// Go: the `trace func(msg *diagnostics.Message, args ...any)` field.
pub type TraceFn = Arc<dyn Fn(&Message, &[String]) + Send + Sync>;

/// Go: `func NewCompilerHost(fs, defaultLibraryPath, extendedConfigCache, trace, contentMapperProject) CompilerHost`.
///
/// PORT: `contentMapperProject` is dropped (see the file header) and a nil
/// `trace` degrades to the no-op Go installs.
pub fn new_compiler_host(
    fs: Arc<dyn Fs>,
    default_library_path: RootedDirectoryPath,
    extended_config_cache: tsc_tsoptions::ExtendedConfigCache,
    trace: Option<TraceFn>,
) -> Arc<dyn CompilerHost> {
    Arc::new(CompilerHostImpl {
        fs,
        default_library_path,
        extended_config_cache,
        trace: trace.unwrap_or_else(|| Arc::new(|_: &Message, _: &[String]| {})),
    })
}

impl CompilerHost for CompilerHostImpl {
    fn fs(&self) -> Arc<dyn Fs> {
        Arc::clone(&self.fs)
    }

    fn default_library_path(&self) -> RootedDirectoryPath {
        self.default_library_path.clone()
    }

    fn trace(&self, message: &Message, args: &[String]) {
        (self.trace)(message, args);
    }

    fn get_source_file(&self, opts: &SourceFileParseOptions) -> Option<SourceFile> {
        let text = self.fs.read_file(&opts.file_name)?;
        Some(tsc_parser::parse_source_file(
            opts.clone(),
            &text,
            crate::ast_shims::ensure_script_kind_from_file_name(&opts.file_name),
        ))
    }

    fn get_resolved_project_reference(
        &self,
        file_name: &RootedFilePath,
        _path: &tsc_tspath::PathKey,
    ) -> Option<Arc<ParsedCommandLine>> {
        // Go: GetParsedCommandLineOfConfigFilePath(fileName, path, nil, nil,
        // h.fs, h.extendedConfigCache). The Rust tsoptions surface is
        // (fileName, fs, resolveConfigHost); path/optionsRaw carry the
        // AST-config-path plumbing (not ported) and the extended-config
        // cache is consulted inside the same crate where it is installed.
        let _ = &self.extended_config_cache;
        Some(Arc::new(tsc_tsoptions::get_parsed_command_line_of_config_file_path(
            file_name,
            self.fs.as_ref(),
            None,
        )))
    }
}

/// Go: `type compilerResolutionHost struct` (fileloader.go) — the
/// module.ResolutionHost adapter over a CompilerHost + base directory.
pub(crate) struct CompilerResolutionHost {
    pub host: Arc<dyn CompilerHost>,
    pub base_directory: RootedDirectoryPath,
}

impl tsc_module::ResolutionHost for CompilerResolutionHost {
    fn fs(&self) -> Arc<dyn Fs> {
        self.host.fs()
    }

    fn get_current_directory(&self) -> RootedDirectoryPath {
        self.base_directory.clone()
    }
}

/// Go: `ast.NewHasFileName(fileName, path)` — the port's file-name carrier
/// for resolution surfaces (the Go helper wraps a name+path pair into an
/// `ast.HasFileName`).
#[derive(Clone, Debug)]
pub struct HasFileName {
    pub file_name: RootedFilePath,
    pub path: tsc_tspath::PathKey,
}

impl HasFileName {
    pub fn new(file_name: RootedFilePath, path: tsc_tspath::PathKey) -> Self {
        HasFileName { file_name, path }
    }

    /// Go: `tspath.RootedPath(fileName)` — the un-intended path form.
    pub fn as_rooted_path(&self) -> RootedPath {
        RootedPath::from(self.file_name.as_string())
    }
}

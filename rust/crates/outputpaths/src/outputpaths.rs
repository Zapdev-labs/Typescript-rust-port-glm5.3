// Ported from tsc/internal/outputpaths/outputpaths.go @ ec47d33c23e464a17cdf2475632cba629bee8763

use tsc_core::options_generated::{CompilerOptions, JsxEmit};
use tsc_tspath::{
    file_extension_is, file_extension_is_one_of, CaseSensitivity, RootedDirectoryPath,
    RootedFilePath,
};

/// Go: `type OutputPathsHost interface` — the surface output path computation
/// needs from the program (or a `*tsoptions.ParsedCommandLine`).
///
/// PORT (trait design): Go interface value → object-safe trait used as
/// `&dyn OutputPathsHost` (vfs crate `Arc<dyn Fs>` precedent).
pub trait OutputPathsHost {
    /// Go: `CommonSourceDirectory() tspath.RootedDirectoryPath`.
    fn common_source_directory(&self) -> RootedDirectoryPath;
    /// Go: `ContentMapperExtensions() []string`.
    fn content_mapper_extensions(&self) -> &[String];
    /// Go: `CaseSensitivity() tspath.CaseSensitivity`.
    fn case_sensitivity(&self) -> CaseSensitivity;
}

/// Go: `type OutputPaths struct` (private fields + the accessors the
/// declarations package requires).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OutputPaths {
    pub(crate) js_file_path: RootedFilePath,
    pub(crate) source_map_file_path: RootedFilePath,
    pub(crate) declaration_file_path: RootedFilePath,
    pub(crate) declaration_map_path: RootedFilePath,
}

impl OutputPaths {
    /// Go: `func (o *OutputPaths) DeclarationFilePath() tspath.RootedFilePath`
    /// (implements declarations.OutputPaths).
    pub fn declaration_file_path(&self) -> &RootedFilePath {
        &self.declaration_file_path
    }
    /// Go: `func (o *OutputPaths) JsFilePath() tspath.RootedFilePath`.
    pub fn js_file_path(&self) -> &RootedFilePath {
        &self.js_file_path
    }
    /// Go: `func (o *OutputPaths) SourceMapFilePath() tspath.RootedFilePath`.
    pub fn source_map_file_path(&self) -> &RootedFilePath {
        &self.source_map_file_path
    }
    /// Go: `func (o *OutputPaths) DeclarationMapPath() tspath.RootedFilePath`.
    pub fn declaration_map_path(&self) -> &RootedFilePath {
        &self.declaration_map_path
    }
}

/// Go: `type ForceEmitPaths struct`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ForceEmitPaths {
    pub dts: bool,
    pub js: bool,
    pub declaration_map: bool,
}

/// Go: the `*ast.SourceFile` parameter of `GetOutputPathsFor`/`ForEachEmittedFile`.
///
/// PORT: Go passes the `*ast.SourceFile` directly and reads
/// `FileName()`, `ast.IsJsonSourceFile(file)` and `file.ContentMapper()`.
/// The ast crate's content-mapper surface (ContentMapperSourceFileInfo) is
/// deferred (see the ast crate PORT notes), so the trait carries the three
/// reads explicitly; the impl for `tsc_ast::SourceFile` returns `""` for
/// `content_mapper` until that surface lands (no content-mapped file can
/// exist before then — TransformAndParse is likewise deferred).
pub trait OutputPathsSourceFile {
    fn file_name(&self) -> &RootedFilePath;
    /// Go: `ast.IsJsonSourceFile(file)` — `file.ScriptKind == core.ScriptKindJSON`.
    fn is_json_source_file(&self) -> bool;
    /// Go: `file.ContentMapper()` — the content mapper identity, or "".
    fn content_mapper(&self) -> &str;
}

impl OutputPathsSourceFile for tsc_ast::SourceFile {
    fn file_name(&self) -> &RootedFilePath {
        tsc_ast::SourceFile::file_name(self)
    }
    fn is_json_source_file(&self) -> bool {
        // Go: `file.ScriptKind == core.ScriptKindJSON`.
        self.payload().script_kind == tsc_core::scriptkind::ScriptKind::JSON
    }
    fn content_mapper(&self) -> &str {
        // PORT: see the trait docs — "" until the ast content-mapper surface
        // (ContentMapperSourceFileInfo) lands.
        ""
    }
}

/// Go: `func GetOutputPathsFor(sourceFile, options, host, force) *OutputPaths`.
pub fn get_output_paths_for<F: OutputPathsSourceFile>(
    source_file: &F,
    options: &CompilerOptions,
    host: &dyn OutputPathsHost,
    force: ForceEmitPaths,
) -> OutputPaths {
    let file_name = source_file.file_name().clone();
    let own_output_file_path = get_own_emit_output_file_path_for_file_name(
        &file_name,
        options,
        host,
        &get_output_extension_for_file_name(&file_name, options.jsx),
    );
    let is_json_file = source_file.is_json_source_file();
    // If json file emits to the same location skip writing it, if emitDeclarationOnly skip writing it
    let is_json_emitted_to_same_location = is_json_file
        && host.case_sensitivity().compare_file_paths(&file_name, &own_output_file_path) == 0;
    let mut paths = OutputPaths::default();
    if source_file.content_mapper().is_empty()
        && (force.js || options.emit_declaration_only != tsc_core::tristate::Tristate::TSTrue)
        && !is_json_emitted_to_same_location
    {
        paths.js_file_path = own_output_file_path.clone();
        if !is_json_file {
            paths.source_map_file_path = get_source_map_file_path(&paths.js_file_path, options);
        }
    }
    if force.dts || options.get_emit_declarations() && !is_json_file {
        paths.declaration_file_path =
            get_declaration_emit_output_file_path_for_file_name(&file_name, options, host);
        if options.get_are_declaration_maps_enabled()
            || force.declaration_map && options.declaration_map.is_true()
        {
            paths.declaration_map_path = paths.declaration_file_path.append_suffix(".map");
        }
    }
    paths
}

/// Go: `func ForEachEmittedFile(host, options, action, sourceFiles, forceDtsEmit) bool`.
pub fn for_each_emitted_file<F: OutputPathsSourceFile>(
    host: &dyn OutputPathsHost,
    options: &CompilerOptions,
    action: &mut dyn FnMut(&OutputPaths, &F) -> bool,
    source_files: &[F],
    force_dts_emit: bool,
) -> bool {
    for source_file in source_files {
        if action(
            &get_output_paths_for(source_file, options, host, ForceEmitPaths { dts: force_dts_emit, ..ForceEmitPaths::default() }),
            source_file,
        ) {
            return true;
        }
    }
    false
}

/// Go: `func GetOutputJSFileName(inputFileName, options, host) tspath.RootedFilePath`.
pub fn get_output_js_file_name(
    input_file_name: &RootedFilePath,
    options: &CompilerOptions,
    host: &dyn OutputPathsHost,
) -> RootedFilePath {
    if options.emit_declaration_only.is_true() || is_content_mapped_file_name(input_file_name, host)
    {
        return RootedFilePath::default();
    }
    let output_file_name = get_output_js_file_name_worker(input_file_name, options, host);
    if !output_file_name.extension_is(tsc_tspath::EXTENSION_JSON)
        || host.case_sensitivity().compare_file_paths(input_file_name, &output_file_name) != 0
    {
        return output_file_name;
    }

    RootedFilePath::default()
}

/// Go: `func isContentMappedFileName(fileName, host) bool`.
pub(crate) fn is_content_mapped_file_name(file_name: &RootedFilePath, host: &dyn OutputPathsHost) -> bool {
    // PORT: the host hands out owned `String`s; tspath takes `&[&str]`.
    let extensions: Vec<&str> = host
        .content_mapper_extensions()
        .iter()
        .map(String::as_str)
        .collect();
    !file_name
        .longest_extension(&extensions, host.case_sensitivity())
        .is_empty()
}

/// Go: `func GetOutputJSFileNameWorker(inputFileName, options, host) tspath.RootedFilePath`.
pub fn get_output_js_file_name_worker(
    input_file_name: &RootedFilePath,
    options: &CompilerOptions,
    host: &dyn OutputPathsHost,
) -> RootedFilePath {
    get_output_file_name_without_changing_extension(input_file_name, &options.out_dir, host)
        .change_extension(&get_output_extension_for_file_name(input_file_name, options.jsx))
}

/// Go: `func GetOutputDeclarationFileNameWorker(inputFileName, options, host) tspath.RootedFilePath`.
pub fn get_output_declaration_file_name_worker(
    input_file_name: &RootedFilePath,
    options: &CompilerOptions,
    host: &dyn OutputPathsHost,
) -> RootedFilePath {
    let dir = if options.declaration_dir.is_empty() {
        options.out_dir.clone()
    } else {
        options.declaration_dir.clone()
    };
    let path = get_output_file_name_without_changing_extension(input_file_name, &dir, host);
    change_to_declaration_extension(&path, host)
}

/// Go: `func GetOutputExtension(fileName string, jsx core.JsxEmit) string`.
pub fn get_output_extension(file_name: &str, jsx: JsxEmit) -> String {
    if file_extension_is(file_name, tsc_tspath::EXTENSION_JSON) {
        return tsc_tspath::EXTENSION_JSON.to_string();
    }
    if jsx == JsxEmit::Preserve
        && file_extension_is_one_of(file_name, &[tsc_tspath::EXTENSION_JSX, tsc_tspath::EXTENSION_TSX])
    {
        return tsc_tspath::EXTENSION_JSX.to_string();
    }
    if file_extension_is_one_of(
        file_name,
        &[tsc_tspath::EXTENSION_MTS, tsc_tspath::EXTENSION_MJS],
    ) {
        return tsc_tspath::EXTENSION_MJS.to_string();
    }
    if file_extension_is_one_of(
        file_name,
        &[tsc_tspath::EXTENSION_CTS, tsc_tspath::EXTENSION_CJS],
    ) {
        return tsc_tspath::EXTENSION_CJS.to_string();
    }
    tsc_tspath::EXTENSION_JS.to_string()
}

/// Go: `func GetOutputExtensionForFileName(fileName, jsx) string`.
pub fn get_output_extension_for_file_name(file_name: &RootedFilePath, jsx: JsxEmit) -> String {
    get_output_extension(file_name.as_string(), jsx)
}

/// Go: `func getDeclarationEmitOutputFilePathForFileName(file, options, host)`.
fn get_declaration_emit_output_file_path_for_file_name(
    file: &RootedFilePath,
    options: &CompilerOptions,
    host: &dyn OutputPathsHost,
) -> RootedFilePath {
    let mut output_dir = RootedDirectoryPath::default();
    if !options.declaration_dir.is_empty() {
        output_dir = options.declaration_dir.clone();
    } else if !options.out_dir.is_empty() {
        output_dir = options.out_dir.clone();
    }

    if !output_dir.is_empty() {
        return change_to_declaration_extension(
            &get_source_file_name_in_new_dir(
                file,
                &output_dir,
                &host.common_source_directory(),
                host.case_sensitivity(),
            ),
            host,
        );
    }
    change_to_declaration_extension(file, host)
}

/// Go: `func ChangeToDeclarationExtension(path, host) tspath.RootedFilePath`.
pub fn change_to_declaration_extension(
    path: &RootedFilePath,
    host: &dyn OutputPathsHost,
) -> RootedFilePath {
    // PORT: the host hands out owned `String`s; tspath takes `&[&str]`.
    let content_mapper_extensions: Vec<&str> = host
        .content_mapper_extensions()
        .iter()
        .map(String::as_str)
        .collect();
    let extension =
        path.longest_extension(&content_mapper_extensions, CaseSensitivity::CaseSensitive);
    if !extension.is_empty() {
        return path
            .remove_extension(extension)
            .append_suffix(&format!(".d{extension}.ts"));
    }
    let mut path_without_extension = path.remove_file_extension();
    if path_without_extension.as_string() == path.as_string() {
        // Go: `path.AnyExtension(nil, tspath.CaseSensitive)` — nil extensions
        // means "any extension"; the Rust port passes an empty slice for the
        // same meaning (see RootedFilePath::any_extension).
        let extension = path.any_extension(&[], CaseSensitivity::CaseSensitive);
        if !extension.is_empty() {
            path_without_extension = path.remove_extension(extension.as_ref());
        }
    }
    path_without_extension.append_suffix(&path.declaration_emit_extension())
}

/// Go: `func getOutputFileNameWithoutChangingExtension(inputFileName, outputDirectory, host)`.
fn get_output_file_name_without_changing_extension(
    input_file_name: &RootedFilePath,
    output_directory: &RootedDirectoryPath,
    host: &dyn OutputPathsHost,
) -> RootedFilePath {
    if !output_directory.is_empty() {
        if let Some(relative_path) = host
            .case_sensitivity()
            .relative_path_from_directory(&host.common_source_directory(), input_file_name)
        {
            return output_directory.resolve_relative_file(&relative_path);
        }
        return input_file_name.clone();
    }
    input_file_name.clone()
}

/// Go: `func GetSourceFileNameInNewDir(fileName, newDirPath, commonSourceDirectory, caseSensitivity)`.
pub fn get_source_file_name_in_new_dir(
    file_name: &RootedFilePath,
    new_dir_path: &RootedDirectoryPath,
    common_source_directory: &RootedDirectoryPath,
    case_sensitivity: CaseSensitivity,
) -> RootedFilePath {
    if file_name.as_path() == common_source_directory.as_path() {
        return file_name.clone();
    }
    if let Some(relative_path) =
        case_sensitivity.relative_file_path_from_directory(common_source_directory, file_name)
    {
        return new_dir_path.resolve_relative_file(&relative_path);
    }
    file_name.clone()
}

/// Go: `func getOwnEmitOutputFilePathForFileName(fileName, options, host, extension)`.
fn get_own_emit_output_file_path_for_file_name(
    file_name: &RootedFilePath,
    options: &CompilerOptions,
    host: &dyn OutputPathsHost,
    extension: &str,
) -> RootedFilePath {
    let emit_output_file_path = if !options.out_dir.is_empty() {
        get_source_file_name_in_new_dir(
            file_name,
            &options.out_dir,
            &host.common_source_directory(),
            host.case_sensitivity(),
        )
    } else {
        file_name.clone()
    };
    emit_output_file_path
        .remove_file_extension()
        .append_suffix(extension)
}

/// Go: `func GetSourceMapFilePath(jsFilePath, options) tspath.RootedFilePath`.
pub fn get_source_map_file_path(
    js_file_path: &RootedFilePath,
    options: &CompilerOptions,
) -> RootedFilePath {
    if options.source_map.is_true() && !options.inline_source_map.is_true() {
        return js_file_path.append_suffix(".map");
    }
    RootedFilePath::default()
}

/// Go: `func GetBuildInfoFileName(options, caseSensitivity) tspath.RootedFilePath`.
pub fn get_build_info_file_name(
    options: &CompilerOptions,
    case_sensitivity: CaseSensitivity,
) -> RootedFilePath {
    if !options.is_incremental() && !options.build.is_true() {
        return RootedFilePath::default();
    }
    if !options.ts_build_info_file.is_empty() {
        return options.ts_build_info_file.clone();
    }
    if options.config_file_path.is_empty() {
        return RootedFilePath::default();
    }
    let config_file_name = options.config_file_path.clone();
    let build_info_file_name: RootedFilePath;
    if !options.out_dir.is_empty() {
        if !options.root_dir.is_empty() {
            if let Some(relative_path) =
                case_sensitivity.relative_path_from_directory(&options.root_dir, &config_file_name)
            {
                build_info_file_name = options.out_dir.resolve_relative_file(&relative_path);
            } else {
                build_info_file_name = config_file_name.clone();
            }
        } else {
            build_info_file_name = options.out_dir.resolve_file(config_file_name.base_name());
        }
    } else {
        build_info_file_name = config_file_name.clone();
    }
    build_info_file_name
        .remove_file_extension()
        .append_suffix(tsc_tspath::EXTENSION_TS_BUILD_INFO)
}

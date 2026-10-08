// Ported from tsc/internal/module/resolver.go @ ec47d33c23e464a17cdf2475632cba629bee8763

use std::sync::Arc;

use tsc_ast::{Diagnostic, SourceFile};
use tsc_collections::{OrderedMap, Set};
use tsc_core::compileroptions::{
    CompilerOptions, ModuleKind, ModuleResolutionKind, ResolutionMode, ResolutionModeCommonJS,
    ResolutionModeESM, ResolutionModeNone,
};
use tsc_core::options_generated::ScriptTarget;
use tsc_core::pattern::{Pattern, find_best_pattern_match, try_parse_pattern};
use tsc_core::text::TextRange;
use tsc_core::tristate::Tristate;
use tsc_diagnostics::Message;
use tsc_packagejson::{
    ExportsOrImports, Expected, Fields, InfoCache, InfoCacheEntry, PackageDirectory, PackageJson,
    TypeValidatedField, VersionPaths, parse as packagejson_parse,
};
use tsc_tspath::{
    CaseSensitivity, EXTENSION_CJS, EXTENSION_CTS, EXTENSION_DCTS, EXTENSION_DMTS, EXTENSION_DTS,
    EXTENSION_JS, EXTENSION_JSON, EXTENSION_JSX, EXTENSION_MJS, EXTENSION_MTS, EXTENSION_TS,
    EXTENSION_TSX, FileNameStem, ModuleSpecifier, PathKey, RelativePath, RootedDirectoryPath,
    RootedFilePath, RootedPath, SUPPORTED_TS_EXTENSIONS_WITH_JSON_FLAT, change_full_extension,
    combine_paths, decode_dynamic_uri_path, decode_dynamic_uri_path_for_disk,
    dynamic_uri_path_to_module_specifier, encode_dynamic_directory_specifier,
    encode_dynamic_logical_module_specifier, encode_dynamic_module_specifier,
    encode_dynamic_relative_uri_directory_path, encode_dynamic_relative_uri_path,
    ensure_trailing_directory_separator, extension_is_one_of, extension_is_ts,
    file_extension_is_one_of, get_path_components, get_root_length,
    has_trailing_directory_separator, is_encoded_dynamic_file_name,
    is_external_module_name_relative, is_rooted_disk_path, path_is_absolute, path_is_relative,
    remove_trailing_directory_separator, resolve_path, rooted_directory_path_from_absolute,
    rooted_directory_path_from_path, rooted_file_path_from_path, try_get_extension_from_path,
};
use tsc_vfs::Fs;

use crate::cache::{
    ModuleResolutionCache, ModuleResolutionCacheKey, ParsedPatternsCache, ResolutionData,
    TypeRefDirectiveResolutionCache, TypeRefDirectiveResolutionCacheKey, get_redirect_config_name,
    new_resolution_data,
};
use crate::types::{
    Extensions, NodeResolutionFeatures, PackageId, ResolvedModule, ResolvedProjectReference,
    ResolvedTypeReferenceDirective, ResolutionHost, Resolver,
};
use crate::util::{
    InferredTypesContainingFile, get_types_package_name, is_applicable_versioned_types_key,
    mangle_scoped_package_name, node_module_package_root_for_file, parse_package_name,
    try_get_js_extension_for_file,
};
use crate::vfsmatch::{UNLIMITED_DEPTH, read_directory};

// ---------------------------------------------------------------------------
// Tracing
// ---------------------------------------------------------------------------

// DiagAndArgs pairs a trace message with its format arguments.
//
// PORT: Go stores `Args []any` (mixed types formatted with %v at print
// time); the port pre-stringifies args into `Vec<String>` (the same
// convention the packagejson crate pinned its trace fn args to). Every
// resolver trace arg is a string, path, or bool, and Go's %v formatting of
// those matches the `ToDiagArg` impls below.
#[derive(Clone, Debug)]
pub struct DiagAndArgs {
    pub message: &'static Message,
    pub args: Vec<String>,
}

// tracer collects the trace of a single resolution.
#[derive(Default)]
pub(crate) struct Tracer {
    traces: Vec<DiagAndArgs>,
}

impl Tracer {
    fn write(&mut self, message: &'static Message, args: Vec<String>) {
        self.traces.push(DiagAndArgs { message, args });
    }

    fn get_traces(&mut self) -> Vec<DiagAndArgs> {
        std::mem::take(&mut self.traces)
    }
}

// writeTrace is Go's nil-receiver-safe `(*tracer).write`: a trace through an
// absent tracer is a no-op.
pub(crate) fn write_trace(tracer: &mut Option<Tracer>, message: &'static Message, args: Vec<String>) {
    if let Some(tracer) = tracer {
        tracer.write(message, args);
    }
}

// ToDiagArg stringifies a trace argument exactly as Go's %v would print it.
pub(crate) trait ToDiagArg {
    fn to_diag_arg(&self) -> String;
}

impl ToDiagArg for str {
    fn to_diag_arg(&self) -> String {
        self.to_string()
    }
}

impl ToDiagArg for String {
    fn to_diag_arg(&self) -> String {
        self.clone()
    }
}

impl ToDiagArg for bool {
    fn to_diag_arg(&self) -> String {
        self.to_string()
    }
}

impl<T: ToDiagArg + ?Sized> ToDiagArg for &T {
    fn to_diag_arg(&self) -> String {
        (**self).to_diag_arg()
    }
}

impl ToDiagArg for RootedFilePath {
    fn to_diag_arg(&self) -> String {
        self.as_string().to_string()
    }
}

impl ToDiagArg for RootedDirectoryPath {
    fn to_diag_arg(&self) -> String {
        self.as_string().to_string()
    }
}

impl ToDiagArg for RootedPath {
    fn to_diag_arg(&self) -> String {
        self.as_string().to_string()
    }
}

impl ToDiagArg for RelativePath {
    fn to_diag_arg(&self) -> String {
        self.as_string().to_string()
    }
}

impl ToDiagArg for FileNameStem {
    fn to_diag_arg(&self) -> String {
        self.as_string().to_string()
    }
}

impl ToDiagArg for ModuleSpecifier {
    fn to_diag_arg(&self) -> String {
        self.as_string().to_string()
    }
}

impl ToDiagArg for PathKey {
    fn to_diag_arg(&self) -> String {
        self.as_string().to_string()
    }
}

impl ToDiagArg for PackageId {
    fn to_diag_arg(&self) -> String {
        self.to_string()
    }
}

impl ToDiagArg for PackageDirectory {
    fn to_diag_arg(&self) -> String {
        self.to_string()
    }
}

impl ToDiagArg for ResolutionCandidate {
    fn to_diag_arg(&self) -> String {
        self.as_string()
    }
}

impl ToDiagArg for Extensions {
    fn to_diag_arg(&self) -> String {
        self.to_string()
    }
}

pub(crate) fn diag_arg<T: ToDiagArg + ?Sized>(value: &T) -> String {
    value.to_diag_arg()
}

// trace! mirrors Go's `t.write(diagnostics.X, args...)` call shape:
// `trace!(self.tracer, DIAGNOSTIC_X, arg0, arg1)`.
macro_rules! trace {
    ($t:expr, $msg:expr $(, $arg:expr)* $(,)?) => {
        write_trace(&mut $t, &$msg, vec![$(diag_arg(&$arg)),*])
    };
}

// ---------------------------------------------------------------------------
// resolved
// ---------------------------------------------------------------------------

// resolved is an in-flight resolution result.
//
// PORT: Go models "keep searching" as a nil `*resolved` and "resolved to
// nothing" as a non-nil struct with an empty path; the port keeps the exact
// three-state shape as `Option<resolved>` (None = continueSearching) with
// `Some(Resolved{ path: "" })` for Go's `unresolved()` zero struct. The
// Go nil-receiver methods become a trait over `Option<Resolved>`.
#[derive(Clone, Debug, Default)]
pub(crate) struct Resolved {
    pub(crate) path: RootedFilePath,
    pub(crate) extension: String,
    pub(crate) package_id: PackageId,
    pub(crate) original_path: RootedFilePath,
    pub(crate) resolved_using_ts_extension: bool,
    pub(crate) resolved_using_extra_extensions: bool,
}

impl Resolved {
    fn is_resolved(&self) -> bool {
        !self.path.is_empty()
    }
}

// ResolvedOpt carries Go's nil-receiver `resolved` method set over
// `Option<Resolved>`.
pub(crate) trait ResolvedOpt {
    /// Go: `func (r *resolved) shouldContinueSearching() bool { return r == nil }`.
    fn should_continue_searching(&self) -> bool;
    /// Go: `func (r *resolved) isResolved() bool { return r != nil && r.path != "" }`.
    fn is_resolved(&self) -> bool;
}

impl ResolvedOpt for Option<Resolved> {
    fn should_continue_searching(&self) -> bool {
        self.is_none()
    }

    fn is_resolved(&self) -> bool {
        self.as_ref().is_some_and(|r| r.is_resolved())
    }
}

pub(crate) fn continue_searching() -> Option<Resolved> {
    None
}

pub(crate) fn unresolved() -> Option<Resolved> {
    Some(Resolved::default())
}

// ---------------------------------------------------------------------------
// resolutionCandidate
// ---------------------------------------------------------------------------

// resolutionCandidate is a rooted, normalized lookup location. Its FileName
// omits any trailing separator; directoryOnly preserves the resolver behavior
// that skips file lookup for candidates written with directory intent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ResolutionCandidate {
    pub(crate) path: RootedPath,
    /// Go: `directoryPath tspath.RootedDirectoryPath` with "" as the unset
    /// sentinel; the port uses Option.
    pub(crate) directory_path: Option<RootedDirectoryPath>,
    pub(crate) directory_only: bool,
}

fn resolution_candidate_from_normalized(path: &str) -> ResolutionCandidate {
    let mut directory_only = has_trailing_directory_separator(path);
    let mut path = path;
    if directory_only && path.len() > get_root_length(path) {
        path = remove_trailing_directory_separator(path);
    }
    ResolutionCandidate {
        path: RootedPath::from(path),
        directory_path: None,
        directory_only,
    }
}

fn resolve_resolution_candidate(path: &str, paths: &[&str]) -> ResolutionCandidate {
    resolution_candidate_from_normalized(&resolve_path(path, paths))
}

fn resolution_candidate_from_directory(directory: &RootedDirectoryPath) -> ResolutionCandidate {
    ResolutionCandidate {
        path: directory.as_path(),
        ..Default::default()
    }
}

fn resolution_candidate_from_file_name(file_name: &RootedFilePath) -> ResolutionCandidate {
    ResolutionCandidate {
        path: file_name.as_path(),
        ..Default::default()
    }
}

fn resolution_candidate_from_directory_path(
    directory: &RootedDirectoryPath,
    path: &str,
) -> ResolutionCandidate {
    let file_path = path_for_dynamic_resolution(directory, path, false);
    let directory_path = path_for_dynamic_resolution(directory, path, true);
    resolution_candidate_from_encoded_paths(
        directory,
        &file_path,
        &directory_path,
        has_trailing_directory_separator(path),
    )
}

fn resolution_candidate_from_dynamic_logical_path(
    directory: &RootedDirectoryPath,
    path: &str,
) -> ResolutionCandidate {
    let empty = path.is_empty();
    let mut directory_only = path.is_empty() || has_trailing_directory_separator(path);
    let mut path = path;
    if !path.is_empty() && directory_only {
        path = remove_trailing_directory_separator(path);
    }
    if empty {
        return ResolutionCandidate {
            path: directory.as_path(),
            directory_only: true,
            ..Default::default()
        };
    }
    resolution_candidate_from_encoded_paths(
        directory,
        &encode_dynamic_relative_uri_path(path),
        &encode_dynamic_relative_uri_directory_path(path),
        directory_only,
    )
}

fn resolution_candidate_from_disk_logical_path(
    directory: &RootedDirectoryPath,
    path: &str,
    directory_only: bool,
) -> ResolutionCandidate {
    let mut path = path;
    if !path.is_empty() && directory_only {
        path = remove_trailing_directory_separator(path);
    }
    if path.is_empty() {
        return ResolutionCandidate {
            path: directory.as_path(),
            directory_only,
            ..Default::default()
        };
    }
    ResolutionCandidate {
        path: directory
            .resolve_file_from_normalized_relative(path)
            .as_path(),
        directory_only,
        ..Default::default()
    }
}

fn resolution_candidate_from_encoded_paths(
    directory: &RootedDirectoryPath,
    file_path: &str,
    directory_path: &str,
    directory_only: bool,
) -> ResolutionCandidate {
    let resolved_directory_path = directory.resolve_directory(directory_path);
    if directory_only {
        return ResolutionCandidate {
            path: resolved_directory_path.as_path(),
            directory_only: true,
            ..Default::default()
        };
    }
    let mut candidate = ResolutionCandidate {
        path: directory.resolve_file(file_path).as_path(),
        ..Default::default()
    };
    if resolved_directory_path.as_path() != candidate.path {
        candidate.directory_path = Some(resolved_directory_path);
    }
    candidate
}

fn path_for_dynamic_resolution(
    directory: &RootedDirectoryPath,
    path: &str,
    directory_only: bool,
) -> String {
    if is_encoded_dynamic_file_name(directory.as_string()) && !path_is_absolute(path) {
        if directory_only {
            return encode_dynamic_directory_specifier(path).into_owned();
        }
        return encode_dynamic_module_specifier(path).into_owned();
    }
    path.to_string()
}

fn resolution_candidate_from_relative_path(
    directory: &RootedDirectoryPath,
    path: &RelativePath,
) -> ResolutionCandidate {
    let directory_only = path.is_empty() || path.has_trailing_directory_separator();
    let mut path = path;
    let stripped;
    if !path.is_empty() && directory_only {
        stripped = path.without_trailing_directory_separator();
        path = &stripped;
    }
    if path.is_empty() {
        return ResolutionCandidate {
            path: directory.as_path(),
            directory_only: true,
            ..Default::default()
        };
    }
    ResolutionCandidate {
        path: directory.resolve_relative_file(path).as_path(),
        directory_only,
        ..Default::default()
    }
}

impl ResolutionCandidate {
    fn as_string(&self) -> String {
        if self.directory_only {
            return ensure_trailing_directory_separator(self.path.as_string()).into_owned();
        }
        self.path.as_string().to_string()
    }

    fn as_directory_path(&self) -> RootedDirectoryPath {
        if let Some(directory_path) = &self.directory_path {
            return directory_path.clone();
        }
        rooted_directory_path_from_path(self.path.clone())
    }

    fn directory(&self) -> RootedDirectoryPath {
        self.path.directory()
    }

    fn has_trailing_directory_separator(&self) -> bool {
        self.directory_only
    }

    fn remove_file_extension(&self) -> FileNameStem {
        RootedFilePath::from(self.path.as_string()).remove_file_extension()
    }

    fn remove_extension(&self, extension: &str) -> FileNameStem {
        RootedFilePath::from(self.path.as_string()).remove_extension(extension)
    }

    fn resolve(&self, path: &str) -> ResolutionCandidate {
        resolution_candidate_from_directory_path(
            &rooted_directory_path_from_path(self.path.clone()),
            path,
        )
    }

    fn has_directory_prefix(&self, directory: &RootedDirectoryPath) -> bool {
        match self.path.relative_to(directory) {
            Some(relative) => !relative.is_empty() || self.directory_only,
            None => false,
        }
    }

    fn relative_to_directory(&self, directory: &RootedDirectoryPath) -> RelativePath {
        let relative = self
            .path
            .relative_to(directory)
            .expect("resolution candidate must be within prefix");
        if self.directory_only {
            return relative.with_trailing_directory_separator();
        }
        relative
    }

    fn split_extension(&self, extra_extensions: &[String]) -> (FileNameStem, String) {
        let file_name = RootedFilePath::from(self.path.as_string());
        let mut extension = file_name.extension().to_string();
        let mut extensionless = self.remove_file_extension();
        if extensionless.as_string() == self.path.as_string() {
            let extra_refs: Vec<&str> = extra_extensions.iter().map(|s| s.as_str()).collect();
            extension = file_name
                .longest_extension(&extra_refs, CaseSensitivity::CaseSensitive)
                .to_string();
            if extension.is_empty() {
                let path = self.path.as_string();
                let Some(dot) = path.rfind('.') else {
                    // Go: path[strings.LastIndex(path, "."):] panics with -1.
                    panic!("no extension in path {path:?}");
                };
                extension = path[dot..].to_string();
            }
            extensionless = self.remove_extension(&extension);
        }
        (extensionless, extension)
    }
}

// ---------------------------------------------------------------------------
// resolutionState
// ---------------------------------------------------------------------------

// resolutionKindSpecificLoader mirrors Go's loader closure type
// `func(extensions extensions, candidate resolutionCandidate) *resolved`.
//
// PORT: Go's loaders capture the `*resolutionState` itself, and a Rust
// closure cannot capture the `&mut ResolutionState` it would be passed back
// into (tryLoadModuleUsingPaths is a `&mut self` method). The loader closures
// therefore become an explicit loader-kind enum, dispatched by
// `ResolutionState::run_loader` — each arm is the corresponding Go closure
// body verbatim.
pub(crate) enum LoaderKind<'a> {
    // The tryLoadModuleUsingPathsIfEligible / tryLoadModuleUsingRootDirs
    // loaders: `r.nodeLoadModuleByRelativeName(extensions, candidate, true)`.
    NodeRelative,
    // The loadModuleFromSpecificNodeModulesDirectory loader.
    SpecificNodeModulesDirectory {
        rest: &'a str,
        package_info: Option<&'a Arc<InfoCacheEntry>>,
    },
    // The loadNodeModuleFromDirectoryWorker loader.
    DirectoryWorker {
        package_file: Option<&'a ResolutionCandidate>,
        package_info: Option<&'a Arc<InfoCacheEntry>>,
    },
}

// resolutionState is the state for one module resolution request.
pub(crate) struct ResolutionState<'a> {
    pub(crate) resolver: &'a DefaultResolver,
    pub(crate) tracer: Option<Tracer>,

    // request fields
    pub(crate) name: String,
    pub(crate) containing_directory: RootedDirectoryPath,
    pub(crate) containing_directory_has_trailing_separator: bool,
    pub(crate) is_config_lookup: bool,
    pub(crate) features: NodeResolutionFeatures,
    pub(crate) esm_mode: bool,
    pub(crate) conditions: Vec<String>,
    pub(crate) extensions: Extensions,
    pub(crate) compiler_options: Arc<CompilerOptions>,
    pub(crate) resolve_package_directory_only: bool,

    // state fields
    // candidateEndingIsFromConfig is set when the candidate file extension
    // originated from configuration (package.json fields, tsconfig.json paths
    // entries, or wildcard substitutions) rather than from the module specifier
    // written in source code. When true, resolvedUsingTsExtension is
    // suppressed so the checker does not attempt to extract a TS extension
    // from the original specifier.
    pub(crate) candidate_ending_is_from_config: bool,
    pub(crate) resolved_package_directory: bool,
    pub(crate) diagnostics: Vec<Arc<Diagnostic>>,
}

impl<'a> ResolutionState<'a> {
    // A state with only the fields Go's direct struct literals set
    // (`&resolutionState{resolver: r, compilerOptions: ...}`).
    pub(crate) fn new_minimal(resolver: &'a DefaultResolver) -> ResolutionState<'a> {
        ResolutionState {
            resolver,
            tracer: None,
            name: String::new(),
            containing_directory: RootedDirectoryPath::default(),
            containing_directory_has_trailing_separator: false,
            is_config_lookup: false,
            features: NodeResolutionFeatures::NodeResolutionFeaturesNone,
            esm_mode: false,
            conditions: Vec::new(),
            extensions: Extensions::default(),
            compiler_options: Arc::clone(resolver.resolution_data().compiler_options()),
            resolve_package_directory_only: false,
            candidate_ending_is_from_config: false,
            resolved_package_directory: false,
            diagnostics: Vec::new(),
        }
    }
}

// newResolutionState creates the state for one module resolution request.
#[allow(clippy::too_many_arguments)]
fn new_resolution_state<'a>(
    name: &str,
    containing_directory: &RootedDirectoryPath,
    is_type_reference_directive: bool,
    resolution_mode: ResolutionMode,
    compiler_options: &Arc<CompilerOptions>,
    redirected_reference: Option<&Arc<dyn ResolvedProjectReference>>,
    resolver: &'a DefaultResolver,
    trace_builder: Option<Tracer>,
) -> ResolutionState<'a> {
    let mut state = ResolutionState {
        name: name.to_string(),
        containing_directory: containing_directory.clone(),
        compiler_options: get_compiler_options_with_redirect(compiler_options, redirected_reference),
        resolver,
        tracer: trace_builder,
        containing_directory_has_trailing_separator: false,
        is_config_lookup: false,
        features: NodeResolutionFeatures::NodeResolutionFeaturesNone,
        esm_mode: false,
        conditions: Vec::new(),
        extensions: Extensions::default(),
        resolve_package_directory_only: false,
        candidate_ending_is_from_config: false,
        resolved_package_directory: false,
        diagnostics: Vec::new(),
    };

    if is_type_reference_directive {
        state.extensions = Extensions::extensionsDeclaration;
    } else if compiler_options.no_dts_resolution == Tristate::TSTrue {
        state.extensions = Extensions::extensionsImplementationFiles;
    } else {
        state.extensions = Extensions::extensionsTypeScript
            | Extensions::extensionsJavaScript
            | Extensions::extensionsDeclaration;
    }

    if !is_type_reference_directive && compiler_options.get_resolve_json_module() {
        state.extensions |= Extensions::extensionsJson;
    }

    match compiler_options.get_module_resolution_kind() {
        ModuleResolutionKind::Node16 => {
            state.features = NodeResolutionFeatures::NodeResolutionFeaturesNode16Default;
            state.esm_mode = resolution_mode == ModuleKind::ESNext;
            state.conditions = get_conditions(compiler_options, resolution_mode);
        }
        ModuleResolutionKind::NodeNext => {
            state.features = NodeResolutionFeatures::NodeResolutionFeaturesNodeNextDefault;
            state.esm_mode = resolution_mode == ModuleKind::ESNext;
            state.conditions = get_conditions(compiler_options, resolution_mode);
        }
        ModuleResolutionKind::Bundler => {
            state.features = get_node_resolution_features(compiler_options);
            state.conditions = get_conditions(compiler_options, resolution_mode);
        }
        _ => {}
    }
    state
}

impl ResolutionState<'_> {
    fn containing_directory_path(&self) -> String {
        if self.containing_directory_has_trailing_separator {
            return ensure_trailing_directory_separator(self.containing_directory.as_string())
                .into_owned();
        }
        self.containing_directory.as_string().to_string()
    }
}

pub fn get_compiler_options_with_redirect(
    compiler_options: &Arc<CompilerOptions>,
    redirected_reference: Option<&Arc<dyn ResolvedProjectReference>>,
) -> Arc<CompilerOptions> {
    let Some(redirected_reference) = redirected_reference else {
        return Arc::clone(compiler_options);
    };
    if let Some(options_from_redirect) = redirected_reference.compiler_options() {
        return Arc::new(options_from_redirect.clone());
    }
    Arc::clone(compiler_options)
}

// ---------------------------------------------------------------------------
// DefaultResolver
// ---------------------------------------------------------------------------

// DefaultResolver is the module resolver over a filesystem.
pub struct DefaultResolver {
    resolution_data: Arc<ResolutionData>,
    fs: Arc<dyn Fs>,
    base_directory: RootedDirectoryPath,
    // reportDiagnostic: DiagnosticReporter

    pub(crate) module_resolution_cache: ModuleResolutionCache,
    pub(crate) type_ref_directive_resolution_cache: TypeRefDirectiveResolutionCache,

    // Cached representations for `core.CompilerOptions.paths`, keyed by the
    // path mappings themselves. This does not handle other path patterns such
    // as `typesVersions`.
    pub(crate) parsed_patterns_for_paths: ParsedPatternsCache,
}

/// Go: `type ResolverOptions struct`.
pub struct ResolverOptions {
    pub host: Arc<dyn ResolutionHost>,
    pub compiler_options: Arc<CompilerOptions>,
    pub typings_location: RootedDirectoryPath,
    pub project_name: String,
    pub extra_extensions: Vec<String>,
    /// Go: `PackageJsonCache *packagejson.InfoCache` — None means create a
    /// fresh cache over the host's filesystem.
    pub package_json_cache: Option<InfoCache>,
}

impl DefaultResolver {
    pub(crate) fn resolution_data(&self) -> &Arc<ResolutionData> {
        &self.resolution_data
    }

    pub(crate) fn fs(&self) -> &Arc<dyn Fs> {
        &self.fs
    }

    pub(crate) fn compiler_options(&self) -> &Arc<CompilerOptions> {
        self.resolution_data.compiler_options()
    }

    /// A bare resolver for the in-package tests (Go tests write
    /// `&DefaultResolver{ResolutionData: ..., fs: ...}` via struct literals).
    pub(crate) fn bare(resolution_data: Arc<ResolutionData>, fs: Arc<dyn Fs>) -> DefaultResolver {
        DefaultResolver {
            resolution_data,
            fs,
            base_directory: RootedDirectoryPath::default(),
            module_resolution_cache: ModuleResolutionCache::default(),
            type_ref_directive_resolution_cache: TypeRefDirectiveResolutionCache::default(),
            parsed_patterns_for_paths: ParsedPatternsCache::default(),
        }
    }
}

impl Resolver for DefaultResolver {
    fn base_directory(&self) -> RootedDirectoryPath {
        self.base_directory()
    }

    fn resolve_module_name(
        &self,
        module_name: &str,
        containing_file: &RootedFilePath,
        resolution_mode: ResolutionMode,
        redirected_reference: Option<&Arc<dyn ResolvedProjectReference>>,
    ) -> crate::types::ResolveModuleNameResult {
        self.resolve_module_name(module_name, containing_file, resolution_mode, redirected_reference)
    }

    fn resolve_module_name_from_directory(
        &self,
        module_name: &str,
        containing_directory: &RootedDirectoryPath,
        resolution_mode: ResolutionMode,
    ) -> crate::types::ResolveModuleNameResult {
        self.resolve_module_name_from_directory(module_name, containing_directory, resolution_mode)
    }

    fn resolve_type_reference_directive(
        &self,
        type_reference_directive_name: &str,
        containing_file: &RootedFilePath,
        resolution_mode: ResolutionMode,
        redirected_reference: Option<&Arc<dyn ResolvedProjectReference>>,
    ) -> crate::types::ResolveTypeReferenceDirectiveResult {
        self.resolve_type_reference_directive(
            type_reference_directive_name,
            containing_file,
            resolution_mode,
            redirected_reference,
        )
    }

    fn get_resolution_data(&self) -> Arc<ResolutionData> {
        self.get_resolution_data()
    }
}

impl DefaultResolver {
    pub fn base_directory(&self) -> RootedDirectoryPath {
        self.base_directory.clone()
    }
}

/// Go: `func NewResolver(opts ResolverOptions) *DefaultResolver`.
pub fn new_resolver(opts: ResolverOptions) -> DefaultResolver {
    let data = new_resolution_data(&opts);
    data.new_resolver(opts.host)
}

impl DefaultResolver {
    pub fn get_resolution_data(&self) -> Arc<ResolutionData> {
        Arc::clone(&self.resolution_data)
    }
}

impl ResolutionData {
    /// Go: `func (d *ResolutionData) NewResolver(host ResolutionHost) *DefaultResolver`.
    pub fn new_resolver(self: &Arc<Self>, host: Arc<dyn ResolutionHost>) -> DefaultResolver {
        let fs = host.fs();
        let base_directory = host.get_current_directory();
        if base_directory.is_empty() {
            panic!("resolver must have a rooted base directory");
        }
        if self.package_json_info_cache().case_sensitivity() != fs.case_sensitivity() {
            panic!("package JSON cache and resolver must use the same case sensitivity");
        }
        DefaultResolver {
            resolution_data: Arc::clone(self),
            fs,
            base_directory,
            module_resolution_cache: ModuleResolutionCache::default(),
            type_ref_directive_resolution_cache: TypeRefDirectiveResolutionCache::default(),
            parsed_patterns_for_paths: ParsedPatternsCache::default(),
        }
    }
}

/// Go: `func NewResolverWithOptions(...)`.
#[allow(clippy::too_many_arguments)]
pub fn new_resolver_with_options(
    host: Arc<dyn ResolutionHost>,
    compiler_options: Arc<CompilerOptions>,
    typings_location: RootedDirectoryPath,
    project_name: String,
    mut opts: ResolverOptions,
) -> DefaultResolver {
    opts.host = host;
    opts.compiler_options = compiler_options;
    opts.typings_location = typings_location;
    opts.project_name = project_name;
    new_resolver(opts)
}

impl DefaultResolver {
    fn new_trace_builder(&self) -> Option<Tracer> {
        if self.resolution_data.compiler_options().trace_resolution == Tristate::TSTrue {
            return Some(Tracer::default());
        }
        None
    }

    /// Go: `func (r *DefaultResolver) GetPackageScopeForPath(directory) *packagejson.InfoCacheEntry`.
    pub fn get_package_scope_for_path(
        &self,
        directory: &RootedDirectoryPath,
    ) -> Option<Arc<InfoCacheEntry>> {
        let state = ResolutionState::new_minimal(self);
        state.get_package_scope_for_path(directory)
    }

    /// Go: `func (r *DefaultResolver) PackageJsonCacheEntries(f func(key, value) bool)`.
    pub fn package_json_cache_entries(
        &self,
        f: impl FnMut(PathKey, Arc<InfoCacheEntry>) -> bool,
    ) {
        self.resolution_data.package_json_cache_entries(f);
    }
}

fn trace_resolution_using_project_reference(
    tracer: &mut Option<Tracer>,
    redirected_reference: Option<&Arc<dyn ResolvedProjectReference>>,
) {
    if let Some(redirected_reference) = redirected_reference
        && redirected_reference.compiler_options().is_some()
    {
        write_trace(
            tracer,
            &tsc_diagnostics::USING_COMPILER_OPTIONS_OF_PROJECT_REFERENCE_REDIRECT_0,
            vec![diag_arg(redirected_reference.config_name())],
        );
    }
}

impl DefaultResolver {
    // Go: `func (r *DefaultResolver) ResolveTypeReferenceDirective(...)` —
    // returns `(*ResolvedTypeReferenceDirective, []DiagAndArgs)`.
    pub fn resolve_type_reference_directive(
        &self,
        type_reference_directive_name: &str,
        containing_file: &RootedFilePath,
        resolution_mode: ResolutionMode,
        redirected_reference: Option<&Arc<dyn ResolvedProjectReference>>,
    ) -> (Option<Arc<ResolvedTypeReferenceDirective>>, Vec<DiagAndArgs>) {
        let containing_directory = containing_file.directory();
        let mut trace_builder = self.new_trace_builder();

        let from_inferred_types_containing_file =
            containing_file.base_name() == InferredTypesContainingFile;

        let cache_key = TypeRefDirectiveResolutionCacheKey {
            containing_directory: containing_directory.clone(),
            type_reference_name: type_reference_directive_name.to_string(),
            resolution_mode,
            redirect_config_name: get_redirect_config_name(redirected_reference),
            from_inferred_types_containing_file,
        };

        if trace_builder.is_none() {
            if let Some(cached) = self.type_ref_directive_resolution_cache.get(&cache_key) {
                return (cached, Vec::new());
            }
        }

        let compiler_options = get_compiler_options_with_redirect(
            self.resolution_data.compiler_options(),
            redirected_reference,
        );

        let (type_roots, from_config) = compiler_options.get_effective_type_roots(&self.base_directory);
        if trace_builder.is_some() {
            trace!(
                trace_builder,
                tsc_diagnostics::RESOLVING_TYPE_REFERENCE_DIRECTIVE_0_CONTAINING_FILE_1_ROOT_DIRECTORY_2,
                type_reference_directive_name,
                containing_file,
                type_roots.iter().map(|root| root.as_string().to_string()).collect::<Vec<_>>().join(",")
            );
            trace_resolution_using_project_reference(&mut trace_builder, redirected_reference);
        }

        let mut state = new_resolution_state(
            type_reference_directive_name,
            &containing_directory,
            true, /*isTypeReferenceDirective*/
            resolution_mode,
            &compiler_options,
            redirected_reference,
            self,
            trace_builder,
        );
        // PORT: Go threads `traceBuilder` (a pointer) into both the state
        // and the outer scope; Rust can't share the &mut, so the state owns
        // the tracer and the outer trace calls go through the state before
        // the result is returned.
        let result = state.resolve_type_reference_directive(
            &type_roots,
            from_config,
            from_inferred_types_containing_file,
        );
        trace_type_reference_directive_result(&mut state.tracer, type_reference_directive_name, &result);
        let traces = state.tracer.as_mut().map(|t| t.get_traces()).unwrap_or_default();

        let result_arc = Arc::new(result);
        self.type_ref_directive_resolution_cache
            .set(cache_key, Some(Arc::clone(&result_arc)));

        (Some(result_arc), traces)
    }
}

impl DefaultResolver {
    // Go: `func (r *DefaultResolver) ResolveModuleName(...)`.
    pub fn resolve_module_name(
        &self,
        module_name: &str,
        containing_file: &RootedFilePath,
        resolution_mode: ResolutionMode,
        redirected_reference: Option<&Arc<dyn ResolvedProjectReference>>,
    ) -> (Option<Arc<ResolvedModule>>, Vec<DiagAndArgs>) {
        let (result, trace) = self.resolve_module_name_inner(
            module_name,
            containing_file.as_path().as_string(),
            &containing_file.directory(),
            resolution_mode,
            redirected_reference,
        );
        (Some(Arc::new(result)), trace)
    }

    // Go: `func (r *DefaultResolver) ResolveModuleNameFromDirectory(...)`.
    pub fn resolve_module_name_from_directory(
        &self,
        module_name: &str,
        containing_directory: &RootedDirectoryPath,
        resolution_mode: ResolutionMode,
    ) -> (Option<Arc<ResolvedModule>>, Vec<DiagAndArgs>) {
        let (result, trace) = self.resolve_module_name_inner(
            module_name,
            containing_directory.as_path().as_string(),
            containing_directory,
            resolution_mode,
            None,
        );
        (Some(Arc::new(result)), trace)
    }

    // Go: `func (r *DefaultResolver) resolveModuleName(...) (*ResolvedModule, []DiagAndArgs)`.
    fn resolve_module_name_inner(
        &self,
        module_name: &str,
        containing_file: &str,
        containing_directory: &RootedDirectoryPath,
        resolution_mode: ResolutionMode,
        redirected_reference: Option<&Arc<dyn ResolvedProjectReference>>,
    ) -> (ResolvedModule, Vec<DiagAndArgs>) {
        let mut trace_builder = self.new_trace_builder();

        let cache_key = ModuleResolutionCacheKey {
            containing_directory: containing_directory.clone(),
            module_name: module_name.to_string(),
            resolution_mode,
            redirect_config_name: get_redirect_config_name(redirected_reference),
        };

        if trace_builder.is_none() {
            if let Some(cached) = self.module_resolution_cache.get(&cache_key) {
                return (cached.unwrap_or_default(), Vec::new());
            }
        }

        let compiler_options = get_compiler_options_with_redirect(
            self.resolution_data.compiler_options(),
            redirected_reference,
        );
        if trace_builder.is_some() {
            trace!(
                trace_builder,
                tsc_diagnostics::RESOLVING_MODULE_0_FROM_1,
                module_name,
                containing_file
            );
            trace_resolution_using_project_reference(&mut trace_builder, redirected_reference);
        }

        let module_resolution = compiler_options.get_module_resolution_kind();
        if compiler_options.module_resolution != module_resolution {
            if trace_builder.is_some() {
                trace!(
                    trace_builder,
                    tsc_diagnostics::MODULE_RESOLUTION_KIND_IS_NOT_SPECIFIED_USING_0,
                    module_resolution.to_string()
                );
            }
        } else if trace_builder.is_some() {
            trace!(
                trace_builder,
                tsc_diagnostics::EXPLICITLY_SPECIFIED_MODULE_RESOLUTION_KIND_COLON_0,
                module_resolution.to_string()
            );
        }

        let mut state = new_resolution_state(
            module_name,
            containing_directory,
            false, /*isTypeReferenceDirective*/
            resolution_mode,
            &compiler_options,
            redirected_reference,
            self,
            trace_builder,
        );
        let mut result = match module_resolution {
            ModuleResolutionKind::Node16
            | ModuleResolutionKind::NodeNext
            | ModuleResolutionKind::Bundler => state.resolve_node_like(),
            _ => panic!("Unexpected moduleResolution: {}", module_resolution),
        };

        if state.tracer.is_some() {
            if ResolvedModule::is_resolved(Some(&result)) {
                if !result.package_id.name.is_empty() {
                    trace!(
                        state.tracer,
                        tsc_diagnostics::MODULE_NAME_0_WAS_SUCCESSFULLY_RESOLVED_TO_1_WITH_PACKAGE_ID_2,
                        module_name,
                        result.resolved_file_name,
                        result.package_id
                    );
                } else {
                    trace!(
                        state.tracer,
                        tsc_diagnostics::MODULE_NAME_0_WAS_SUCCESSFULLY_RESOLVED_TO_1,
                        module_name,
                        result.resolved_file_name
                    );
                }
            } else {
                trace!(
                    state.tracer,
                    tsc_diagnostics::MODULE_NAME_0_WAS_NOT_RESOLVED,
                    module_name
                );
            }
        }

        let final_result =
            self.try_resolve_from_typings_location(module_name, containing_directory, result, &mut state);
        let traces = state.tracer.as_mut().map(|t| t.get_traces()).unwrap_or_default();

        self.module_resolution_cache
            .set(cache_key, Some(Arc::new(final_result.clone())));

        (final_result, traces)
    }

    // Go: `func (r *DefaultResolver) ResolvePackageDirectory(...) *ResolvedModule`.
    pub fn resolve_package_directory(
        &self,
        module_name: &str,
        containing_file: &RootedFilePath,
        resolution_mode: ResolutionMode,
        redirected_reference: Option<&Arc<dyn ResolvedProjectReference>>,
    ) -> Option<Arc<ResolvedModule>> {
        let compiler_options = get_compiler_options_with_redirect(
            self.resolution_data.compiler_options(),
            redirected_reference,
        );
        let containing_directory = containing_file.directory();
        let mut state = new_resolution_state(
            module_name,
            &containing_directory,
            false, /*isTypeReferenceDirective*/
            resolution_mode,
            &compiler_options,
            redirected_reference,
            self,
            None,
        );
        state.resolve_package_directory_only = true;
        let result = state.load_module_from_nearest_node_modules_directory(false /*typesScopeOnly*/);
        if result.as_ref().is_some_and(|r| !r.path.is_empty()) {
            return Some(Arc::new(state.create_resolved_module_handling_symlink(result)));
        }
        None
    }

    // Go: `func (r *DefaultResolver) tryResolveFromTypingsLocation(...) *ResolvedModule`.
    fn try_resolve_from_typings_location(
        &self,
        module_name: &str,
        containing_directory: &RootedDirectoryPath,
        original_result: ResolvedModule,
        trace_builder_owner: &mut ResolutionState<'_>,
    ) -> ResolvedModule {
        let typings_location = self.resolution_data.typings_location();
        if typings_location.is_empty()
            || is_external_module_name_relative(module_name)
            || (!original_result.resolved_file_name.is_empty()
                && extension_is_one_of(
                    original_result.extension.as_str(),
                    &SUPPORTED_TS_EXTENSIONS_WITH_JSON_FLAT,
                ))
        {
            return original_result;
        }

        let compiler_options = self.resolution_data.compiler_options();
        let mut state = new_resolution_state(
            module_name,
            containing_directory,
            false,              /*isTypeReferenceDirective*/
            ResolutionModeNone, // resolutionMode,
            compiler_options,
            None, // redirectedReference,
            self,
            trace_builder_owner.tracer.take(),
        );
        if state.tracer.is_some() {
            trace!(
                state.tracer,
                tsc_diagnostics::AUTO_DISCOVERY_FOR_TYPINGS_IS_ENABLED_IN_PROJECT_0_RUNNING_EXTRA_RESOLUTION_PASS_FOR_MODULE_1_USING_CACHE_LOCATION_2,
                self.resolution_data.project_name(),
                module_name,
                self.resolution_data.typings_location()
            );
        }
        let global_resolved = state.load_module_from_immediate_node_modules_directory(
            Extensions::extensionsDeclaration,
            typings_location,
            false,
        );
        if global_resolved.should_continue_searching() {
            // Restore the tracer ownership before returning (the outer state
            // still wants to harvest the traces below).
            trace_builder_owner.tracer = state.tracer.take();
            return original_result;
        }
        let mut result = state.create_resolved_module(global_resolved, true);
        // Go: `append(originalResult.ResolutionDiagnostics, result.ResolutionDiagnostics...)`.
        let mut combined = original_result.resolution_diagnostics.clone();
        combined.extend(std::mem::take(&mut result.resolution_diagnostics));
        result.resolution_diagnostics = combined;
        trace_builder_owner.tracer = state.tracer.take();
        result
    }

    // Go: `func (r *DefaultResolver) resolveConfig(...)`.
    fn resolve_config(&self, module_name: &str, containing_file: &RootedFilePath) -> Option<Arc<ResolvedModule>> {
        let containing_directory = containing_file.directory();
        let mut state = new_resolution_state(
            module_name,
            &containing_directory,
            false, /*isTypeReferenceDirective*/
            ResolutionModeCommonJS,
            self.resolution_data.compiler_options(),
            None,
            self,
            None,
        );
        state.is_config_lookup = true;
        state.extensions = Extensions::extensionsJson;
        Some(Arc::new(state.resolve_node_like()))
    }
}

// Go: `func (r *tracer) traceTypeReferenceDirectiveResult(...)` — a
// nil-receiver-safe free fn over the tracer slot.
fn trace_type_reference_directive_result(
    tracer: &mut Option<Tracer>,
    type_reference_directive_name: &str,
    result: &ResolvedTypeReferenceDirective,
) {
    if !result.is_resolved() {
        write_trace(
            tracer,
            &tsc_diagnostics::TYPE_REFERENCE_DIRECTIVE_0_WAS_NOT_RESOLVED,
            vec![diag_arg(type_reference_directive_name)],
        );
    } else if !result.package_id.name.is_empty() {
        write_trace(
            tracer,
            &tsc_diagnostics::TYPE_REFERENCE_DIRECTIVE_0_WAS_SUCCESSFULLY_RESOLVED_TO_1_WITH_PACKAGE_ID_2_PRIMARY_COLON_3,
            vec![
                diag_arg(type_reference_directive_name),
                diag_arg(&result.resolved_file_name),
                diag_arg(&result.package_id),
                diag_arg(&result.primary),
            ],
        );
    } else {
        write_trace(
            tracer,
            &tsc_diagnostics::TYPE_REFERENCE_DIRECTIVE_0_WAS_SUCCESSFULLY_RESOLVED_TO_1_PRIMARY_COLON_2,
            vec![
                diag_arg(type_reference_directive_name),
                diag_arg(&result.resolved_file_name),
                diag_arg(&result.primary),
            ],
        );
    }
}

impl ResolutionState<'_> {
    // Go: `func (r *resolutionState) resolveTypeReferenceDirective(...)`.
    fn resolve_type_reference_directive(
        &mut self,
        type_roots: &[RootedDirectoryPath],
        from_config: bool,
        from_inferred_types_containing_file: bool,
    ) -> ResolvedTypeReferenceDirective {
        // Primary lookup
        if !type_roots.is_empty() {
            if self.tracer.is_some() {
                trace!(
                    self.tracer,
                    tsc_diagnostics::RESOLVING_WITH_PRIMARY_SEARCH_PATH_0,
                    type_roots.iter().map(|root| root.as_string().to_string()).collect::<Vec<_>>().join(", ")
                );
            }
            for type_root in type_roots {
                let candidate = self.get_candidate_from_type_root(type_root);
                let directory_exists = self.resolver.fs.directory_exists(type_root);
                if !directory_exists {
                    if self.tracer.is_some() {
                        trace!(
                            self.tracer,
                            tsc_diagnostics::DIRECTORY_0_DOES_NOT_EXIST_SKIPPING_ALL_LOOKUPS_IN_IT,
                            type_root
                        );
                    }
                    continue;
                }
                if from_config {
                    // Custom typeRoots resolve as file or directory just like
                    // we do modules
                    if !candidate.has_trailing_directory_separator() {
                        let resolved_from_file =
                            self.load_module_from_file(Extensions::extensionsDeclaration, &candidate);
                        if !resolved_from_file.should_continue_searching() {
                            let mut resolved_from_file = resolved_from_file.unwrap();
                            let package_directory =
                                node_module_package_root_for_file(&resolved_from_file.path);
                            if !package_directory.is_empty() {
                                let pd = self.package_directory(&package_directory);
                                let info = self.get_package_json_info(&pd);
                                resolved_from_file.package_id =
                                    self.get_package_id(&resolved_from_file.path, info.as_ref());
                            }
                            return self
                                .create_resolved_type_reference_directive(Some(resolved_from_file), true /*primary*/);
                        }
                    }
                }
                let resolved_from_directory = self.load_node_module_from_directory(
                    Extensions::extensionsDeclaration,
                    &candidate,
                    true, /*considerPackageJson*/
                );
                if !resolved_from_directory.should_continue_searching() {
                    return self
                        .create_resolved_type_reference_directive(resolved_from_directory, true /*primary*/);
                }
            }
        } else if self.tracer.is_some() {
            trace!(
                self.tracer,
                tsc_diagnostics::ROOT_DIRECTORY_CANNOT_BE_DETERMINED_SKIPPING_PRIMARY_SEARCH_PATHS
            );
        }

        // Secondary lookup
        let mut resolved = None;
        if !from_config || !from_inferred_types_containing_file {
            if self.tracer.is_some() {
                trace!(
                    self.tracer,
                    tsc_diagnostics::LOOKING_UP_IN_NODE_MODULES_FOLDER_INITIAL_LOCATION_0,
                    self.containing_directory_path()
                );
            }
            if !is_external_module_name_relative(&self.name) {
                resolved = self.load_module_from_nearest_node_modules_directory(false /*typesScopeOnly*/);
            } else {
                let candidate = normalize_path_for_cjs_resolution(
                    &self.containing_directory,
                    &self.name,
                );
                resolved = self.node_load_module_by_relative_name(
                    Extensions::extensionsDeclaration,
                    &candidate,
                    true, /*considerPackageJson*/
                );
            }
        } else if self.tracer.is_some() {
            trace!(
                self.tracer,
                tsc_diagnostics::RESOLVING_TYPE_REFERENCE_DIRECTIVE_FOR_PROGRAM_THAT_SPECIFIES_CUSTOM_TYPEROOTS_SKIPPING_LOOKUP_IN_NODE_MODULES_FOLDER
            );
        }
        self.create_resolved_type_reference_directive(resolved, false /*primary*/)
    }

    // Go: `func (r *resolutionState) getCandidateFromTypeRoot(...)`.
    fn get_candidate_from_type_root(&self, type_root: &RootedDirectoryPath) -> ResolutionCandidate {
        let mut name_for_lookup = self.name.clone();
        if type_root.base_name() == "@types" && type_root.as_path().directory().base_name() == "node_modules" {
            name_for_lookup = self.mangle_scoped_package_name(&self.name.clone());
        }
        resolution_candidate_from_directory_path(type_root, &name_for_lookup)
    }

    // Go: `func (r *resolutionState) mangleScopedPackageName(...)`.
    fn mangle_scoped_package_name(&self, name: &str) -> String {
        let mangled = mangle_scoped_package_name(name);
        if self.tracer.is_some() && mangled != name {
            trace!(self.tracer, tsc_diagnostics::SCOPED_PACKAGE_DETECTED_LOOKING_IN_0, mangled);
        }
        mangled
    }

    // resolveFromTypeRoot tries to resolve a module name from the configured
    // typeRoots. This is used as a fallback after node_modules resolution
    // fails, for declaration file lookups. Returns nil if typeRoots is not
    // configured or if no matching module is found in any typeRoot directory.
    fn resolve_from_type_root(&mut self) -> Option<Resolved> {
        if self.compiler_options.type_roots.is_empty() {
            return None;
        }
        for type_root in self.compiler_options.type_roots.clone() {
            let candidate = self.get_candidate_from_type_root(&type_root);
            let directory_exists = self.resolver.fs.directory_exists(&type_root);
            if !directory_exists {
                if self.tracer.is_some() {
                    trace!(
                        self.tracer,
                        tsc_diagnostics::DIRECTORY_0_DOES_NOT_EXIST_SKIPPING_ALL_LOOKUPS_IN_IT,
                        type_root
                    );
                }
                continue;
            }
            if !candidate.has_trailing_directory_separator() {
                let resolved_from_file =
                    self.load_module_from_file(Extensions::extensionsDeclaration, &candidate);
                if !resolved_from_file.should_continue_searching() {
                    let mut resolved_from_file = resolved_from_file.unwrap();
                    let package_directory = node_module_package_root_for_file(&resolved_from_file.path);
                    if !package_directory.is_empty() {
                        let pd = self.package_directory(&package_directory);
                        let info = self.get_package_json_info(&pd);
                        resolved_from_file.package_id =
                            self.get_package_id(&resolved_from_file.path, info.as_ref());
                    }
                    return Some(resolved_from_file);
                }
            }
            let resolved = self.load_node_module_from_directory(
                Extensions::extensionsDeclaration,
                &candidate,
                true, /*considerPackageJson*/
            );
            if !resolved.should_continue_searching() {
                return resolved;
            }
        }
        None
    }

    // Go: `func (r *resolutionState) getPackageScopeForPath(...) *packagejson.InfoCacheEntry`.
    //
    // PORT: Go's ForEachAncestorDirectoryStoppingAtGlobalCache yields the zero
    // value on root exhaustion (nil); the Rust helper returns Option<T> where
    // T is itself Option<Arc<InfoCacheEntry>>. The callback only ever stops
    // with a non-nil result (Go: `return result, result != nil`), so
    // Some(None) is unreachable and the flattened Option<Arc<...>> is
    // behaviorally identical.
    fn get_package_scope_for_path(&mut self, directory: &RootedDirectoryPath) -> Option<Arc<InfoCacheEntry>> {
        let package_directory = self
            .resolver
            .resolution_data()
            .package_json_info_cache()
            .package_directory(directory.clone());
        tsc_packagejson::for_each_ancestor_directory_stopping_at_global_cache(
            self.resolver.resolution_data().typings_location(),
            package_directory,
            |directory| {
                let result = self.get_package_json_info(directory);
                (result.clone(), result.is_some())
            },
        )
        .flatten()
    }

    // Go: `func (r *resolutionState) resolveNodeLike() *ResolvedModule`.
    fn resolve_node_like(&mut self) -> ResolvedModule {
        if self.tracer.is_some() {
            let conditions = self
                .conditions
                .iter()
                .map(|c| format!("'{c}'"))
                .collect::<Vec<_>>()
                .join(", ");
            if self.esm_mode {
                trace!(
                    self.tracer,
                    tsc_diagnostics::RESOLVING_IN_0_MODE_WITH_CONDITIONS_1,
                    "ESM",
                    conditions
                );
            } else {
                trace!(
                    self.tracer,
                    tsc_diagnostics::RESOLVING_IN_0_MODE_WITH_CONDITIONS_1,
                    "CJS",
                    conditions
                );
            }
        }
        let mut result = self.resolve_node_like_worker();
        if self.resolved_package_directory
            && !self.is_config_lookup
            && self
                .features
                .intersects(NodeResolutionFeatures::NodeResolutionFeaturesExports)
            && self
                .extensions
                .intersects(Extensions::extensionsTypeScript | Extensions::extensionsDeclaration)
            && !is_external_module_name_relative(&self.name)
            && ResolvedModule::is_resolved(Some(&result))
            && result.is_external_library_import
            && !extension_is_ok(
                Extensions::extensionsTypeScript | Extensions::extensionsDeclaration,
                &result.extension,
            )
            && self.conditions.iter().any(|c| c == "import")
        {
            if self.tracer.is_some() {
                trace!(
                    self.tracer,
                    tsc_diagnostics::RESOLUTION_OF_NON_RELATIVE_NAME_FAILED_TRYING_WITH_MODERN_NODE_RESOLUTION_FEATURES_DISABLED_TO_SEE_IF_NPM_LIBRARY_NEEDS_CONFIGURATION_UPDATE
                );
            }
            self.features = self.features & !NodeResolutionFeatures::NodeResolutionFeaturesExports;
            self.extensions =
                self.extensions & (Extensions::extensionsTypeScript | Extensions::extensionsDeclaration);
            let diagnostics_count = self.diagnostics.len();
            let diagnostic_result = self.resolve_node_like_worker();
            if ResolvedModule::is_resolved(Some(&diagnostic_result))
                && diagnostic_result.is_external_library_import
            {
                result.alternate_result = diagnostic_result.resolved_file_name.clone();
            }
            self.diagnostics.truncate(diagnostics_count);
        }
        result
    }

    // Go: `func (r *resolutionState) resolveNodeLikeWorker() *ResolvedModule`.
    fn resolve_node_like_worker(&mut self) -> ResolvedModule {
        let resolved = self.try_load_module_using_optional_resolution_settings();
        if !resolved.should_continue_searching() {
            return self.create_resolved_module_handling_symlink(resolved);
        }

        if !is_external_module_name_relative(&self.name) {
            if self
                .features
                .intersects(NodeResolutionFeatures::NodeResolutionFeaturesImports)
                && self.name.starts_with('#')
            {
                let resolved = self.load_module_from_imports();
                if !resolved.should_continue_searching() {
                    return self.create_resolved_module_handling_symlink(resolved);
                }
            }
            if self
                .features
                .intersects(NodeResolutionFeatures::NodeResolutionFeaturesSelfName)
            {
                let resolved = self.load_module_from_self_name_reference();
                if !resolved.should_continue_searching() {
                    return self.create_resolved_module_handling_symlink(resolved);
                }
            }
            if self.name.contains(':') {
                if self.tracer.is_some() {
                    trace!(
                        self.tracer,
                        tsc_diagnostics::SKIPPING_MODULE_0_THAT_LOOKS_LIKE_AN_ABSOLUTE_URI_TARGET_FILE_TYPES_COLON_1,
                        self.name,
                        self.extensions
                    );
                }
                return self.create_resolved_module(None, false);
            }
            if self.tracer.is_some() {
                trace!(
                    self.tracer,
                    tsc_diagnostics::LOADING_MODULE_0_FROM_NODE_MODULES_FOLDER_TARGET_FILE_TYPES_COLON_1,
                    self.name,
                    self.extensions
                );
            }
            let resolved = self.load_module_from_nearest_node_modules_directory(false /*typesScopeOnly*/);
            if !resolved.should_continue_searching() {
                return self.create_resolved_module_handling_symlink(resolved);
            }
            if self.extensions.intersects(Extensions::extensionsDeclaration) {
                let resolved = self.resolve_from_type_root();
                if !resolved.should_continue_searching() {
                    return self.create_resolved_module_handling_symlink(resolved);
                }
            }
        } else {
            let candidate = normalize_path_for_cjs_resolution(&self.containing_directory, &self.name);
            let resolved = self.node_load_module_by_relative_name(self.extensions, &candidate, true);
            let in_node_modules = resolved
                .as_ref()
                .is_some_and(|r| r.path.contains_lowercase_directory_sequence("/node_modules/"));
            return self.create_resolved_module(resolved, in_node_modules);
        }
        self.create_resolved_module(None, false)
    }

    // Go: `func (r *resolutionState) loadModuleFromSelfNameReference() *resolved`.
    fn load_module_from_self_name_reference(&mut self) -> Option<Resolved> {
        let scope = self.get_package_scope_for_path(&self.containing_directory.clone());
        if !InfoCacheEntry::exists(scope.as_deref())
            || scope
                .as_ref()
                .and_then(|s| s.contents.as_ref())
                .is_some_and(|c| c.path_fields.exports.is_falsy())
        {
            // !!! falsy check seems wrong?
            return continue_searching();
        }
        let contents = scope.as_ref().unwrap().contents.as_ref().unwrap();
        let Some(name) = contents.fields.header_fields.name.get_value() else {
            return continue_searching();
        };
        let name = name.clone();
        let parts = get_path_components(&self.name);
        let name_parts = get_path_components(name);
        if parts.len() < name_parts.len() || name_parts[..] != parts[..name_parts.len()] {
            return continue_searching();
        }
        let trailing_parts = &parts[name_parts.len()..];
        let subpath: String = if !trailing_parts.is_empty() {
            let refs: Vec<&str> = trailing_parts.iter().map(|s| s.as_str()).collect();
            combine_paths(".", &refs)
        } else {
            ".".to_string()
        };
        // Maybe TODO: splitting extensions into two priorities should be unnecessary, except
        // https://github.com/microsoft/TypeScript/issues/50762 makes the behavior different.
        // As long as that bug exists, we need to do two passes here in self-name loading
        // in order to be consistent with (non-self) library-name loading in
        // `loadModuleFromNearestNodeModulesDirectoryWorker`, which uses two passes in order
        // to prioritize `@types` packages higher up the directory tree over untyped
        // implementation packages. See the selfNameModuleAugmentation.ts test for why this
        // matters.
        //
        // However, there's an exception. If the user has `allowJs` and `declaration`, we need
        // to ensure that self-name imports of their own package can resolve back to their
        // input JS files via `tryLoadInputFileForPath` at a higher priority than their output
        // declaration files, so we need to do a single pass with all extensions for that case.
        if self.compiler_options.get_allow_js()
            && !self
                .containing_directory
                .contains_lowercase_directory_sequence("/node_modules/")
        {
            return self.load_module_from_exports(
                scope.as_ref().unwrap(),
                self.extensions,
                &subpath,
            );
        }
        let priority_extensions =
            self.extensions & (Extensions::extensionsTypeScript | Extensions::extensionsDeclaration);
        let secondary_extensions =
            self.extensions & !(Extensions::extensionsTypeScript | Extensions::extensionsDeclaration);
        let resolved =
            self.load_module_from_exports(scope.as_ref().unwrap(), priority_extensions, &subpath);
        if !resolved.should_continue_searching() {
            return resolved;
        }
        self.load_module_from_exports(scope.as_ref().unwrap(), secondary_extensions, &subpath)
    }
}

impl ResolutionState<'_> {
    // Go: `func (r *resolutionState) loadModuleFromImports() *resolved`.
    fn load_module_from_imports(&mut self) -> Option<Resolved> {
        if self.name == "#"
            || (self.name.starts_with("#/")
                && !self
                    .features
                    .intersects(NodeResolutionFeatures::NodeResolutionFeaturesImportsPatternRoot))
        {
            if self.tracer.is_some() {
                trace!(
                    self.tracer,
                    tsc_diagnostics::INVALID_IMPORT_SPECIFIER_0_HAS_NO_POSSIBLE_RESOLUTIONS,
                    self.name
                );
            }
            return continue_searching();
        }
        let directory_path = self.containing_directory.clone();
        let scope = self.get_package_scope_for_path(&directory_path);
        if !InfoCacheEntry::exists(scope.as_deref()) {
            if self.tracer.is_some() {
                trace!(
                    self.tracer,
                    tsc_diagnostics::DIRECTORY_0_HAS_NO_CONTAINING_PACKAGE_JSON_SCOPE_IMPORTS_WILL_NOT_RESOLVE,
                    directory_path
                );
            }
            return continue_searching();
        }
        let contents = InfoCacheEntry::get_contents(scope.as_deref()).expect("checked exists");
        if contents.fields.path_fields.imports.type_ != JSONValueType::Object {
            // !!! Old compiler only checks for undefined, but then assumes `imports` is an object if present.
            // Maybe should have a new diagnostic for imports of an invalid type. Also, array should be handled?
            if self.tracer.is_some() {
                trace!(
                    self.tracer,
                    tsc_diagnostics::X_PACKAGE_JSON_SCOPE_0_HAS_NO_IMPORTS_DEFINED,
                    scope.as_ref().unwrap().package_directory
                );
            }
            return continue_searching();
        }

        let scope = scope.expect("checked exists");
        let result = self.load_module_from_exports_or_imports(
            self.extensions,
            &self.name.clone(),
            contents.fields.path_fields.imports.as_object(),
            &scope,
            true, /*isImports*/
        );
        if !result.should_continue_searching() {
            return result;
        }

        if self.tracer.is_some() {
            trace!(
                self.tracer,
                tsc_diagnostics::IMPORT_SPECIFIER_0_DOES_NOT_EXIST_IN_PACKAGE_JSON_SCOPE_AT_PATH_1,
                self.name,
                scope.package_directory
            );
        }
        continue_searching()
    }

    // Go: `func (r *resolutionState) loadModuleFromExports(...) *resolved`.
    fn load_module_from_exports(
        &mut self,
        package_info: &Arc<InfoCacheEntry>,
        ext: Extensions,
        subpath: &str,
    ) -> Option<Resolved> {
        // !!! This is ported exactly, but the falsy check seems wrong
        if !InfoCacheEntry::exists(Some(package_info))
            || package_info
                .contents
                .as_ref()
                .is_some_and(|c| c.fields.path_fields.exports.is_falsy())
        {
            return continue_searching();
        }
        let contents = package_info.contents.as_ref().expect("checked exists");
        let exports = &contents.fields.path_fields.exports;

        if subpath == "." {
            let mut main_export = ExportsOrImports::default();
            match exports.type_ {
                JSONValueType::String | JSONValueType::Array => main_export = exports.clone(),
                JSONValueType::Object => {
                    if exports.is_conditions() {
                        main_export = exports.clone();
                    } else if let Some(dot) = exports.as_object().get(".") {
                        main_export = dot.clone();
                    }
                }
                _ => {}
            }
            if main_export.is_present() {
                return self.load_module_from_target_export_or_import(
                    ext,
                    subpath,
                    package_info,
                    false, /*isImports*/
                    &main_export,
                    "",
                    false, /*isPattern*/
                    ".",
                );
            }
        } else if exports.type_ == JSONValueType::Object && exports.is_subpaths() {
            let result = self.load_module_from_exports_or_imports(
                ext,
                subpath,
                exports.as_object(),
                package_info,
                false, /*isImports*/
            );
            if !result.should_continue_searching() {
                return result;
            }
        }

        if self.tracer.is_some() {
            trace!(
                self.tracer,
                tsc_diagnostics::EXPORT_SPECIFIER_0_DOES_NOT_EXIST_IN_PACKAGE_JSON_SCOPE_AT_PATH_1,
                subpath,
                package_info.package_directory
            );
        }
        continue_searching()
    }

    // Go: `func (r *resolutionState) loadModuleFromExportsOrImports(...)`.
    fn load_module_from_exports_or_imports(
        &mut self,
        extensions: Extensions,
        module_name: &str,
        lookup_table: &OrderedMap<String, ExportsOrImports>,
        scope: &Arc<InfoCacheEntry>,
        is_imports: bool,
    ) -> Option<Resolved> {
        if !module_name.ends_with('/') && !module_name.contains('*') {
            if let Some(target) = lookup_table.get(module_name) {
                return self.load_module_from_target_export_or_import(
                    extensions,
                    module_name,
                    scope,
                    is_imports,
                    target,
                    "",
                    false, /*isPattern*/
                    module_name,
                );
            }
        }

        let mut expanding_keys: Vec<String> = Vec::with_capacity(lookup_table.size());
        for key in lookup_table.keys() {
            if key.matches('*').count() == 1 || key.ends_with('/') {
                expanding_keys.push(key.clone());
            }
        }
        expanding_keys.sort_by(compare_pattern_keys);

        for potential_target in &expanding_keys {
            if self
                .features
                .intersects(NodeResolutionFeatures::NodeResolutionFeaturesExportsPatternTrailers)
                && matches_pattern_with_trailer(potential_target, module_name)
            {
                let target = lookup_table.get(potential_target).expect("key came from the table");
                let star_pos = potential_target.find('*').expect("trailer match implies a star");
                let start = star_pos;
                let end = module_name.len() - (potential_target.len() - 1 - star_pos);
                let subpath = &module_name[start..end];
                return self.load_module_from_target_export_or_import(
                    extensions,
                    module_name,
                    scope,
                    is_imports,
                    target,
                    subpath,
                    true, /*isPattern*/
                    potential_target,
                );
            } else if potential_target.ends_with('*')
                && module_name.starts_with(&potential_target[..potential_target.len() - 1])
            {
                let target = lookup_table.get(potential_target).expect("key came from the table");
                let subpath = &module_name[potential_target.len() - 1..];
                return self.load_module_from_target_export_or_import(
                    extensions,
                    module_name,
                    scope,
                    is_imports,
                    target,
                    subpath,
                    true, /*isPattern*/
                    potential_target,
                );
            } else if module_name.starts_with(potential_target.as_str()) {
                let target = lookup_table.get(potential_target).expect("key came from the table");
                let subpath = &module_name[potential_target.len()..];
                return self.load_module_from_target_export_or_import(
                    extensions,
                    module_name,
                    scope,
                    is_imports,
                    target,
                    subpath,
                    false, /*isPattern*/
                    potential_target,
                );
            }
        }

        continue_searching()
    }

    // Go: `func (r *resolutionState) loadModuleFromTargetExportOrImport(...)`.
    fn load_module_from_target_export_or_import(
        &mut self,
        extensions: Extensions,
        module_name: &str,
        scope: &Arc<InfoCacheEntry>,
        is_imports: bool,
        target: &ExportsOrImports,
        subpath: &str,
        is_pattern: bool,
        key: &str,
    ) -> Option<Resolved> {
        match target.type_ {
            JSONValueType::String => {
                let target_string = target.as_string();
                if !is_pattern && !subpath.is_empty() && !target_string.ends_with('/') {
                    if self.tracer.is_some() {
                        trace!(
                            self.tracer,
                            tsc_diagnostics::X_PACKAGE_JSON_SCOPE_0_HAS_INVALID_TYPE_FOR_TARGET_OF_SPECIFIER_1,
                            scope.package_directory,
                            module_name
                        );
                    }
                    return continue_searching();
                }
                if !target_string.starts_with("./") {
                    if is_imports
                        && !target_string.starts_with("../")
                        && !target_string.starts_with('/')
                        && !is_rooted_disk_path(target_string)
                    {
                        let combined_lookup = if is_pattern {
                            target_string.replace('*', subpath)
                        } else {
                            format!("{}{}", target_string, subpath)
                        };
                        let scope_containing_directory = ensure_trailing_directory_separator(
                            scope.package_directory.as_directory_path().as_string(),
                        )
                        .into_owned();
                        if self.tracer.is_some() {
                            trace!(
                                self.tracer,
                                tsc_diagnostics::USING_0_SUBPATH_1_WITH_TARGET_2,
                                "imports",
                                key,
                                combined_lookup
                            );
                            trace!(
                                self.tracer,
                                tsc_diagnostics::RESOLVING_MODULE_0_FROM_1,
                                combined_lookup,
                                scope_containing_directory
                            );
                        }
                        // Go swaps the request fields for the recursive
                        // resolveNodeLike and restores them in a defer.
                        let saved_name =
                            std::mem::replace(&mut self.name, combined_lookup);
                        let saved_containing_directory = std::mem::replace(
                            &mut self.containing_directory,
                            scope.package_directory.as_directory_path().clone(),
                        );
                        let saved_has_trailing_separator = std::mem::replace(
                            &mut self.containing_directory_has_trailing_separator,
                            true,
                        );
                        let result = self.resolve_node_like();
                        self.name = saved_name;
                        self.containing_directory = saved_containing_directory;
                        self.containing_directory_has_trailing_separator =
                            saved_has_trailing_separator;
                        if ResolvedModule::is_resolved(Some(&result)) {
                            return Some(Resolved {
                                path: result.resolved_file_name,
                                extension: result.extension,
                                package_id: result.package_id,
                                original_path: result.original_path,
                                resolved_using_ts_extension: result.resolved_using_ts_extension,
                                resolved_using_extra_extensions: false,
                            });
                        }
                        return continue_searching();
                    }
                    if self.tracer.is_some() {
                        trace!(
                            self.tracer,
                            tsc_diagnostics::X_PACKAGE_JSON_SCOPE_0_HAS_INVALID_TYPE_FOR_TARGET_OF_SPECIFIER_1,
                            scope.package_directory,
                            module_name
                        );
                    }
                    return continue_searching();
                }
                let parts: Vec<&str> = if path_is_relative(target_string) {
                    get_path_components(target_string)[1..].to_vec()
                } else {
                    get_path_components(target_string)
                };
                let parts_after_first = &parts[1..];
                if parts_after_first.contains(&"..")
                    || parts_after_first.contains(&".")
                    || parts_after_first.contains(&"node_modules")
                {
                    if self.tracer.is_some() {
                        trace!(
                            self.tracer,
                            tsc_diagnostics::X_PACKAGE_JSON_SCOPE_0_HAS_INVALID_TYPE_FOR_TARGET_OF_SPECIFIER_1,
                            scope.package_directory,
                            module_name
                        );
                    }
                    return continue_searching();
                }
                // TODO: Assert that `resolvedTarget` is actually within the package directory? That's what the spec says.... but I'm not sure we need
                // to be in the business of validating everyone's import and export map correctness.
                let subpath_parts = get_path_components(subpath);
                if subpath_parts.contains(&"..")
                    || subpath_parts.contains(&".")
                    || subpath_parts.contains(&"node_modules")
                {
                    if self.tracer.is_some() {
                        trace!(
                            self.tracer,
                            tsc_diagnostics::X_PACKAGE_JSON_SCOPE_0_HAS_INVALID_TYPE_FOR_TARGET_OF_SPECIFIER_1,
                            scope.package_directory,
                            module_name
                        );
                    }
                    return continue_searching();
                }

                let target_path = if is_pattern {
                    target_string.replace('*', subpath)
                } else {
                    format!("{}{}", target_string, subpath)
                };
                if self.tracer.is_some() {
                    trace!(
                        self.tracer,
                        tsc_diagnostics::USING_0_SUBPATH_1_WITH_TARGET_2,
                        if is_imports { "imports" } else { "exports" },
                        key,
                        target_path
                    );
                }
                let final_path = resolution_candidate_from_directory_path(
                    scope.package_directory.as_directory_path(),
                    &target_path,
                );
                let package_json_file = scope.package_directory.resolve_file("package.json");
                let input_link = self.try_load_input_file_for_path(
                    &final_path,
                    subpath,
                    &package_json_file,
                    is_imports,
                );
                if !input_link.should_continue_searching() {
                    let mut input_link = input_link.unwrap();
                    input_link.package_id = self.get_package_id(&input_link.path, Some(scope));
                    return Some(input_link);
                }
                let result = self.load_file_name_from_package_json_field(
                    extensions,
                    &final_path,
                    target_string,
                );
                if !result.should_continue_searching() {
                    let mut result = result.unwrap();
                    result.package_id = self.get_package_id(&result.path, Some(scope));
                    return Some(result);
                }
                continue_searching()
            }
            JSONValueType::Object => {
                if self.tracer.is_some() {
                    trace!(self.tracer, tsc_diagnostics::ENTERING_CONDITIONAL_EXPORTS);
                }
                for (condition, sub_target) in target.as_object().entries() {
                    if self.condition_matches(condition) {
                        if self.tracer.is_some() {
                            trace!(
                                self.tracer,
                                tsc_diagnostics::MATCHED_0_CONDITION_1,
                                if is_imports { "imports" } else { "exports" },
                                condition
                            );
                        }
                        let result = self.load_module_from_target_export_or_import(
                            extensions,
                            module_name,
                            scope,
                            is_imports,
                            sub_target,
                            subpath,
                            is_pattern,
                            key,
                        );
                        if !result.should_continue_searching() {
                            if result.is_resolved() && self.tracer.is_some() {
                                trace!(
                                    self.tracer,
                                    tsc_diagnostics::RESOLVED_UNDER_CONDITION_0,
                                    condition
                                );
                            }
                            if self.tracer.is_some() {
                                trace!(self.tracer, tsc_diagnostics::EXITING_CONDITIONAL_EXPORTS);
                            }
                            return result;
                        } else if self.tracer.is_some() {
                            trace!(
                                self.tracer,
                                tsc_diagnostics::FAILED_TO_RESOLVE_UNDER_CONDITION_0,
                                condition
                            );
                        }
                    } else if self.tracer.is_some() {
                        trace!(
                            self.tracer,
                            tsc_diagnostics::SAW_NON_MATCHING_CONDITION_0,
                            condition
                        );
                    }
                }
                if self.tracer.is_some() {
                    trace!(self.tracer, tsc_diagnostics::EXITING_CONDITIONAL_EXPORTS);
                }
                continue_searching()
            }
            JSONValueType::Array => {
                if target.as_array().is_empty() {
                    if self.tracer.is_some() {
                        trace!(
                            self.tracer,
                            tsc_diagnostics::X_PACKAGE_JSON_SCOPE_0_HAS_INVALID_TYPE_FOR_TARGET_OF_SPECIFIER_1,
                            scope.package_directory,
                            module_name
                        );
                    }
                    return continue_searching();
                }
                for elem in target.as_array() {
                    let result = self.load_module_from_target_export_or_import(
                        extensions,
                        module_name,
                        scope,
                        is_imports,
                        elem,
                        subpath,
                        is_pattern,
                        key,
                    );
                    if !result.should_continue_searching() {
                        return result;
                    }
                }
                continue_searching()
            }
            JSONValueType::Null => {
                if self.tracer.is_some() {
                    trace!(
                        self.tracer,
                        tsc_diagnostics::X_PACKAGE_JSON_SCOPE_0_EXPLICITLY_MAPS_SPECIFIER_1_TO_NULL,
                        scope.package_directory,
                        module_name
                    );
                }
                unresolved()
            }
            _ => {
                if self.tracer.is_some() {
                    trace!(
                        self.tracer,
                        tsc_diagnostics::X_PACKAGE_JSON_SCOPE_0_HAS_INVALID_TYPE_FOR_TARGET_OF_SPECIFIER_1,
                        scope.package_directory,
                        module_name
                    );
                }
                continue_searching()
            }
        }
    }

    // Go: `func (r *resolutionState) tryLoadInputFileForPath(...) *resolved`.
    fn try_load_input_file_for_path(
        &mut self,
        final_candidate: &ResolutionCandidate,
        entry: &str,
        package_path: &RootedFilePath,
        is_imports: bool,
    ) -> Option<Resolved> {
        if final_candidate.directory_only {
            return continue_searching();
        }
        let final_path = final_candidate.path.clone();
        let final_file_name = rooted_file_path_from_path(final_path.as_string());
        let case_sensitivity = self.resolver.fs.case_sensitivity();
        // Replace any references to outputs for files in the program with the input files to support package self-names used with outDir
        if !self.is_config_lookup
            && (!self.compiler_options.declaration_dir.is_empty()
                || !self.compiler_options.out_dir.is_empty())
            && !final_file_name.contains_lowercase_directory_sequence("/node_modules/")
            && (self.compiler_options.config_file_path.is_empty()
                || case_sensitivity.contains_file_path(
                    &package_path.directory(),
                    &self.compiler_options.config_file_path,
                ))
        {
            // Note: this differs from Strada's tryLoadInputFileForPath in that it
            // does not attempt to perform "guesses", instead requring a clear root indicator.

            let root_dir: RootedDirectoryPath;
            if !self.compiler_options.root_dir.is_empty() {
                // A `rootDir` compiler option strongly indicates the root location
                root_dir = self.compiler_options.root_dir.clone();
            } else if !self.compiler_options.config_file_path.is_empty() {
                // When no explicit rootDir is set, treat the config file's directory as the project root, which establishes the common source directory, so no other locations need to be checked.
                root_dir = self.compiler_options.config_file_path.directory();
            } else {
                let diagnostic = Diagnostic::new(
                    None,
                    TextRange::default(),
                    if is_imports {
                        &tsc_diagnostics::THE_PROJECT_ROOT_IS_AMBIGUOUS_BUT_IS_REQUIRED_TO_RESOLVE_IMPORT_MAP_ENTRY_0_IN_FILE_1_SUPPLY_THE_ROOTDIR_COMPILER_OPTION_TO_DISAMBIGUATE
                    } else {
                        &tsc_diagnostics::THE_PROJECT_ROOT_IS_AMBIGUOUS_BUT_IS_REQUIRED_TO_RESOLVE_EXPORT_MAP_ENTRY_0_IN_FILE_1_SUPPLY_THE_ROOTDIR_COMPILER_OPTION_TO_DISAMBIGUATE
                    },
                    vec![
                        // replace empty string with `.` - the reverse of the operation done when entries are built - so main entrypoint errors don't look weird
                        if entry.is_empty() { ".".to_string() } else { entry.to_string() },
                        package_path.as_string().to_string(),
                    ],
                );
                self.diagnostics.push(Arc::new(diagnostic));
                return unresolved();
            }

            let candidate_directories = self.get_output_directories();
            for candidate_dir in &candidate_directories {
                if let Some(path_fragment) =
                    case_sensitivity.relative_file_path_from_directory(candidate_dir, &final_file_name)
                {
                    // The matched export is looking up something in either the out declaration or js dir, now map the written path back into the source dir and source extension
                    let possible_input_base = root_dir.resolve_relative_file(&path_fragment);
                    let js_and_dts_extensions = [
                        EXTENSION_MJS,
                        EXTENSION_CJS,
                        EXTENSION_JS,
                        EXTENSION_JSON,
                        EXTENSION_DMTS,
                        EXTENSION_DCTS,
                        EXTENSION_DTS,
                    ];
                    for ext in js_and_dts_extensions {
                        if possible_input_base.extension_is(ext) {
                            let input_exts = possible_input_base.possible_original_input_extensions();
                            for possible_ext in &input_exts {
                                if !extension_is_ok(self.extensions, possible_ext) {
                                    continue;
                                }
                                let possible_input_with_input_extension =
                                    possible_input_base.change_extension(possible_ext);
                                if self.resolver.fs.file_exists(&possible_input_with_input_extension) {
                                    let resolved = self.load_file_name_from_package_json_field(
                                        self.extensions,
                                        &resolution_candidate_from_file_name(
                                            &possible_input_with_input_extension,
                                        ),
                                        "",
                                    );
                                    if !resolved.should_continue_searching() {
                                        return resolved;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        continue_searching()
    }

    // Go: `func (r *resolutionState) getOutputDirectories() []tspath.RootedDirectoryPath`.
    fn get_output_directories(&self) -> Vec<RootedDirectoryPath> {
        let mut candidate_directories = Vec::new();
        if !self.compiler_options.declaration_dir.is_empty() {
            candidate_directories.push(self.compiler_options.declaration_dir.clone());
        }
        if !self.compiler_options.out_dir.is_empty()
            && self.compiler_options.out_dir != self.compiler_options.declaration_dir
        {
            candidate_directories.push(self.compiler_options.out_dir.clone());
        }
        candidate_directories
    }

    // Go: `func (r *resolutionState) loadModuleFromNearestNodeModulesDirectory(typesScopeOnly bool) *resolved`.
    fn load_module_from_nearest_node_modules_directory(&mut self, types_scope_only: bool) -> Option<Resolved> {
        let mut mode = ResolutionModeCommonJS;
        if self.esm_mode || self.condition_matches("import") {
            mode = ResolutionModeESM;
        }
        // Do (up to) two passes through node_modules:
        //   1. For each ancestor node_modules directory, try to find:
        //      i.  TS/DTS files in the implementation package
        //      ii. DTS files in the @types package
        //   2. For each ancestor node_modules directory, try to find:
        //      i.  JS files in the implementation package
        let priority_extensions = self.extensions
            & (Extensions::extensionsTypeScript | Extensions::extensionsDeclaration);
        let secondary_extensions =
            self.extensions & !(Extensions::extensionsTypeScript | Extensions::extensionsDeclaration);
        // (1)
        if priority_extensions != Extensions::default() {
            if self.tracer.is_some() {
                trace!(
                    self.tracer,
                    tsc_diagnostics::SEARCHING_ALL_ANCESTOR_NODE_MODULES_DIRECTORIES_FOR_PREFERRED_EXTENSIONS_COLON_0,
                    priority_extensions
                );
            }
            let result = self.load_module_from_nearest_node_modules_directory_worker(
                priority_extensions,
                mode,
                types_scope_only,
            );
            if !result.should_continue_searching() {
                return result;
            }
        }
        // (2)
        if secondary_extensions != Extensions::default() && !types_scope_only {
            if self.tracer.is_some() {
                trace!(
                    self.tracer,
                    tsc_diagnostics::SEARCHING_ALL_ANCESTOR_NODE_MODULES_DIRECTORIES_FOR_FALLBACK_EXTENSIONS_COLON_0,
                    secondary_extensions
                );
            }
            return self.load_module_from_nearest_node_modules_directory_worker(
                secondary_extensions,
                mode,
                types_scope_only,
            );
        }
        continue_searching()
    }

    // Go: `func (r *resolutionState) loadModuleFromNearestNodeModulesDirectoryWorker(ext, mode, typesScopeOnly) *resolved`.
    //
    // PORT: Go threads a `mode` argument that the worker never reads (it is
    // captured by an earlier revision's closure chain); the port keeps the
    // parameter for signature parity, unused.
    fn load_module_from_nearest_node_modules_directory_worker(
        &mut self,
        ext: Extensions,
        _mode: ResolutionMode,
        types_scope_only: bool,
    ) -> Option<Resolved> {
        let result = self
            .containing_directory
            .clone()
            .for_each_ancestor_directory(|directory| {
                // !!! stop at global cache
                if directory.as_path().base_name() != "node_modules" {
                    let result = self.load_module_from_immediate_node_modules_directory(
                        ext,
                        directory.clone(),
                        types_scope_only,
                    );
                    let stop = !result.should_continue_searching();
                    (result, stop)
                } else {
                    (continue_searching(), false)
                }
            });
        // Go's `result, _ :=` — an exhausted walk is Go's nil result.
        result.flatten()
    }

    // Go: `func (r *resolutionState) loadModuleFromImmediateNodeModulesDirectory(extensions, directory, typesScopeOnly) *resolved`.
    fn load_module_from_immediate_node_modules_directory(
        &mut self,
        extensions: Extensions,
        directory: RootedDirectoryPath,
        types_scope_only: bool,
    ) -> Option<Resolved> {
        let node_modules_folder = directory.resolve_directory("node_modules");
        if !self.resolver.fs.directory_exists(&node_modules_folder) {
            if self.tracer.is_some() {
                trace!(
                    self.tracer,
                    tsc_diagnostics::DIRECTORY_0_DOES_NOT_EXIST_SKIPPING_ALL_LOOKUPS_IN_IT,
                    node_modules_folder
                );
            }
            return continue_searching();
        }

        if !types_scope_only {
            let package_result = self.load_module_from_specific_node_modules_directory(
                extensions,
                &self.name.clone(),
                &node_modules_folder,
            );
            if !package_result.should_continue_searching() {
                return package_result;
            }
        }

        if extensions.intersects(Extensions::extensionsDeclaration) {
            let node_modules_at_types = node_modules_folder.resolve_directory("@types");
            if !self.resolver.fs.directory_exists(&node_modules_at_types) {
                if self.tracer.is_some() {
                    trace!(
                        self.tracer,
                        tsc_diagnostics::DIRECTORY_0_DOES_NOT_EXIST_SKIPPING_ALL_LOOKUPS_IN_IT,
                        node_modules_at_types
                    );
                }
                return continue_searching();
            }
            let mangled = self.mangle_scoped_package_name(&self.name.clone());
            return self.load_module_from_specific_node_modules_directory(
                Extensions::extensionsDeclaration,
                &mangled,
                &node_modules_at_types,
            );
        }

        continue_searching()
    }

    // Go: `func (r *resolutionState) loadModuleFromSpecificNodeModulesDirectory(...) *resolved`.
    fn load_module_from_specific_node_modules_directory(
        &mut self,
        ext: Extensions,
        module_name: &str,
        node_modules_directory: &RootedDirectoryPath,
    ) -> Option<Resolved> {
        // Strip any trailing directory separator so that imports like `pkg/` and `pkg`
        // produce identical `candidate` and `packageDirectory` strings. Otherwise the
        // `package.json` info cache (which is keyed by normalized path but stores the
        // caller's `PackageDirectory` verbatim) can hand back, under concurrent
        // inserts, an entry whose `PackageDirectory` doesn't match `candidate`,
        // causing `loadNodeModuleFromDirectoryWorker`'s `ComparePaths(candidate, ...)`
        // check to fail and skip loading the package's `main`/`types` entry.
        // https://github.com/microsoft/TypeScript/tsc/issues/3526
        let candidate = resolution_candidate_from_directory_path(node_modules_directory, module_name);
        let candidate_directory = candidate.as_directory_path();
        let (package_name, rest) = parse_package_name(module_name);
        let rest = rest.to_string();
        let package_directory: RootedDirectoryPath = if package_name.is_empty() {
            candidate_directory.clone()
        } else {
            node_modules_directory.resolve_directory(&path_for_dynamic_resolution(
                node_modules_directory,
                &package_name,
                true,
            ))
        };

        if self.resolve_package_directory_only {
            if self.resolver.fs.directory_exists(&package_directory) {
                return Some(Resolved {
                    path: rooted_file_path_from_path(package_directory.as_path().as_string()),
                    ..Default::default()
                });
            }
            return continue_searching();
        }

        let mut root_package_info: Option<Arc<InfoCacheEntry>> = None;
        // First look for a nested package.json, as in `node_modules/foo/bar/package.json`
        let candidate_pd = self.package_directory(&candidate_directory);
        let mut package_info = self.get_package_json_info(&candidate_pd);
        // But only if we're not respecting export maps (if we are, we might redirect around this location)
        if !rest.is_empty() && InfoCacheEntry::exists(package_info.as_deref()) {
            if self
                .features
                .intersects(NodeResolutionFeatures::NodeResolutionFeaturesExports)
            {
                let package_pd = self.package_directory(&package_directory);
                root_package_info = self.get_package_json_info(&package_pd);
            }
            if !InfoCacheEntry::exists(root_package_info.as_deref())
                || root_package_info
                    .as_ref()
                    .and_then(|i| i.contents.as_ref())
                    .map(|c| c.fields.path_fields.exports.type_ == JSONValueType::NotPresent)
                    .unwrap_or(true)
            {
                let from_file = self.load_module_from_file(ext, &candidate);
                if !from_file.should_continue_searching() {
                    return from_file;
                }

                let from_directory = self.load_node_module_from_directory_worker(
                    ext,
                    &candidate_directory,
                    package_info.as_ref(),
                );
                if !from_directory.should_continue_searching() {
                    let mut from_directory = from_directory.unwrap();
                    from_directory.package_id =
                        self.get_package_id(&from_directory.path, package_info.as_ref());
                    return Some(from_directory);
                }
            }
        }

        if !rest.is_empty() {
            package_info = root_package_info.clone();
            if package_info.is_none() {
                // Previous `packageInfo` may have been from a nested package.json; ensure we have the one from the package root now.
                let package_pd = self.package_directory(&package_directory);
                package_info = self.get_package_json_info(&package_pd);
            }
        }
        if let Some(package_info) = &package_info {
            self.resolved_package_directory = true;
            if self
                .features
                .intersects(NodeResolutionFeatures::NodeResolutionFeaturesExports)
                && InfoCacheEntry::exists(Some(package_info))
                && !package_info
                    .contents
                    .as_ref()
                    .is_some_and(|c| c.fields.path_fields.exports.is_falsy())
            {
                // package exports are higher priority than file/directory/typesVersions lookups and (and, if there's exports present*, blocks them)
                // *Well, weirdly enough a top-level `"exports": null` does NOT block fallback resolution.
                // https://github.com/microsoft/TypeScript/pull/49327
                let subpath = combine_paths(".", &[rest.as_str()]);
                return self.load_module_from_exports(package_info, ext, &subpath);
            }
            if !rest.is_empty() && InfoCacheEntry::exists(Some(package_info)) {
                let contents = package_info.contents.as_ref().expect("checked exists");
                let mut trace = |m: &'static Message, args: &[String]| {
                    write_trace(&mut self.tracer, m, args.to_vec())
                };
                let version_paths = contents.get_version_paths(Some(&mut trace));
                if version_paths.exists() {
                    if self.tracer.is_some() {
                        trace!(
                            self.tracer,
                            tsc_diagnostics::X_PACKAGE_JSON_HAS_A_TYPESVERSIONS_ENTRY_0_THAT_MATCHES_COMPILER_VERSION_1_LOOKING_FOR_A_PATTERN_TO_MATCH_MODULE_NAME_2,
                            version_paths.version,
                            tsc_core::version(),
                            rest
                        );
                    }
                    let path_patterns = try_parse_patterns(version_paths.get_paths().unwrap());
                    let from_paths = self.try_load_module_using_paths(
                        ext,
                        &rest,
                        &package_directory,
                        version_paths.get_paths().unwrap(),
                        &path_patterns,
                        LoaderKind::SpecificNodeModulesDirectory {
                            rest: &rest,
                            package_info: Some(package_info),
                        },
                    );
                    if !from_paths.should_continue_searching() {
                        return from_paths;
                    }
                }
            }
        }
        self.run_loader(
            LoaderKind::SpecificNodeModulesDirectory {
                rest: &rest,
                package_info: package_info.as_ref(),
            },
            ext,
            candidate,
        )
    }

    // run_loader dispatches the Go loader closures (see LoaderKind).
    fn run_loader(
        &mut self,
        loader: LoaderKind<'_>,
        extensions: Extensions,
        candidate: ResolutionCandidate,
    ) -> Option<Resolved> {
        match loader {
            LoaderKind::NodeRelative => {
                self.node_load_module_by_relative_name(extensions, candidate, true /*considerPackageJson*/)
            }
            LoaderKind::SpecificNodeModulesDirectory { rest, package_info } => {
                if !rest.is_empty() || !self.esm_mode {
                    let from_file = self.load_module_from_file(extensions, &candidate);
                    if !from_file.should_continue_searching() {
                        let mut from_file = from_file.unwrap();
                        from_file.package_id =
                            self.get_package_id(&from_file.path, package_info);
                        return Some(from_file);
                    }
                }
                let from_directory = self.load_node_module_from_directory_worker(
                    extensions,
                    &candidate.as_directory_path(),
                    package_info,
                );
                if !from_directory.should_continue_searching() {
                    let mut from_directory = from_directory.unwrap();
                    from_directory.package_id =
                        self.get_package_id(&from_directory.path, package_info);
                    return Some(from_directory);
                }
                if rest.is_empty()
                    && InfoCacheEntry::exists(package_info)
                    && package_info
                        .and_then(|info| info.contents.as_ref())
                        .map(|c| {
                            c.fields.path_fields.exports.type_ == JSONValueType::NotPresent
                                || c.fields.path_fields.exports.type_ == JSONValueType::Null
                        })
                        .unwrap_or(false)
                    && self.esm_mode
                {
                    // EsmMode disables index lookup in `loadNodeModuleFromDirectoryWorker` generally, however non-relative package resolutions still assume
                    // a default `index.js` entrypoint if no `main` or `exports` are present
                    let index_candidate = resolution_candidate_from_directory_path(
                        &candidate.as_directory_path(),
                        "index.js",
                    );
                    let index_result = self.load_module_from_file(extensions, &index_candidate);
                    if !index_result.should_continue_searching() {
                        let mut index_result = index_result.unwrap();
                        index_result.package_id =
                            self.get_package_id(&index_result.path, package_info);
                        return Some(index_result);
                    }
                }
                continue_searching()
            }
            LoaderKind::DirectoryWorker {
                package_file,
                package_info,
            } => {
                let package_file_str = package_file
                    .map(|f| f.as_string())
                    .unwrap_or_default();
                let from_file = self.load_file_name_from_package_json_field(
                    extensions,
                    &candidate,
                    &package_file_str,
                );
                if !from_file.should_continue_searching() {
                    return from_file;
                }

                // Even if `extensions == extensionsDeclaration`, we can still look up a .ts file as a result of package.json "types"
                // !!! should we not set this before the filename lookup above?
                let expanded_extensions = if extensions == Extensions::extensionsDeclaration {
                    Extensions::extensionsTypeScript | Extensions::extensionsDeclaration
                } else {
                    extensions
                };

                // Disable `esmMode` for the resolution of the package path for CJS-mode packages (so the `main` field can omit extensions)
                let save_esm_mode = self.esm_mode;
                let save_candidate_ending_is_from_config = self.candidate_ending_is_from_config;
                self.candidate_ending_is_from_config = true;
                if InfoCacheEntry::exists(package_info)
                    && package_info
                        .and_then(|info| info.contents.as_ref())
                        .map(|c| c.fields.header_fields.type_.value != "module")
                        .unwrap_or(true)
                {
                    self.esm_mode = false;
                }
                let result = self.node_load_module_by_relative_name(
                    expanded_extensions,
                    candidate,
                    false, /*considerPackageJson*/
                );
                self.esm_mode = save_esm_mode;
                self.candidate_ending_is_from_config = save_candidate_ending_is_from_config;
                result
            }
        }
    }

    // Go: `func (r *resolutionState) createResolvedModuleHandlingSymlink(resolved *resolved) *ResolvedModule`.
    fn create_resolved_module_handling_symlink(&mut self, resolved: Option<Resolved>) -> ResolvedModule {
        let is_external_library_import = resolved
            .as_ref()
            .is_some_and(|r| r.path.contains_lowercase_directory_sequence("/node_modules/"));
        if self.compiler_options.preserve_symlinks != Tristate::TSTrue
            && is_external_library_import
            && resolved.as_ref().is_some_and(|r| r.original_path.is_empty())
            && !is_external_module_name_relative(&self.name)
        {
            let resolved_path = resolved.as_ref().expect("checked").path.clone();
            let (original_path, resolved_file_name) =
                self.get_original_and_resolved_file_name(&resolved_path);
            if !original_path.is_empty() {
                let resolved = resolved.as_mut().expect("checked");
                resolved.path = resolved_file_name;
                resolved.original_path = original_path;
            }
        }
        self.create_resolved_module(resolved, is_external_library_import)
    }

    // Go: `func (r *resolutionState) createResolvedModule(resolved *resolved, isExternalLibraryImport bool) *ResolvedModule`.
    fn create_resolved_module(
        &mut self,
        resolved: Option<Resolved>,
        is_external_library_import: bool,
    ) -> ResolvedModule {
        let mut resolved_module = ResolvedModule::default();
        resolved_module.resolution_diagnostics = self.diagnostics.clone();

        if let Some(resolved) = resolved {
            if resolved.is_resolved() {
                resolved_module.resolved_file_name = resolved.path.clone();
                resolved_module.resolved_path = self
                    .resolver
                    .fs
                    .case_sensitivity()
                    .path_key(&resolved.path.as_path());
                if !resolved.original_path.is_empty() {
                    resolved_module.original_path = resolved.original_path.clone();
                }
            }
            resolved_module.is_external_library_import = is_external_library_import;
            resolved_module.resolved_using_ts_extension = resolved.resolved_using_ts_extension;
            resolved_module.resolved_using_extra_extensions = resolved.resolved_using_extra_extensions;
            resolved_module.extension = resolved.extension.clone();
            resolved_module.package_id = resolved.package_id.clone();
        }
        resolved_module
    }

    // Go: `func (r *resolutionState) createResolvedTypeReferenceDirective(resolved *resolved, primary bool) *ResolvedTypeReferenceDirective`.
    fn create_resolved_type_reference_directive(
        &mut self,
        resolved: Option<Resolved>,
        primary: bool,
    ) -> ResolvedTypeReferenceDirective {
        let mut directive = ResolvedTypeReferenceDirective::default();
        directive.resolution_diagnostics = self.diagnostics.clone();

        if resolved.is_resolved() {
            let resolved = resolved.as_ref().expect("is_resolved");
            if !extension_is_ts(&resolved.extension) {
                panic!("expected a TypeScript file extension");
            }
            directive.resolved_file_name = resolved.path.clone();
            directive.primary = primary;
            directive.package_id = resolved.package_id.clone();
            directive.is_external_library_import = resolved
                .path
                .contains_lowercase_directory_sequence("/node_modules/");

            if self.compiler_options.preserve_symlinks != Tristate::TSTrue {
                let (original_path, resolved_file_name) =
                    self.get_original_and_resolved_file_name(&resolved.path);
                if !original_path.is_empty() {
                    directive.resolved_file_name = resolved_file_name;
                    directive.original_path = original_path;
                }
            }
            directive.resolved_path = self
                .resolver
                .fs
                .case_sensitivity()
                .path_key(&directive.resolved_file_name.as_path());
        }
        directive
    }

    // Go: `func (r *resolutionState) getOriginalAndResolvedFileName(...)`.
    fn get_original_and_resolved_file_name(
        &mut self,
        file_name: &RootedFilePath,
    ) -> (RootedFilePath, RootedFilePath) {
        let resolved_file_name = self.real_path(file_name);
        let case_sensitivity = self.resolver.fs.case_sensitivity();
        if case_sensitivity.compare_file_paths(file_name, &resolved_file_name) == 0 {
            // If the fileName and realpath are differing only in casing, prefer fileName
            // so that we can issue correct errors for casing under forceConsistentCasingInFileNames
            return (RootedFilePath::default(), file_name.clone());
        }
        (file_name.clone(), resolved_file_name)
    }
}

// __RESOLVER_PART5__

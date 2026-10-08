// Ported from tsc/internal/module/types.go @ ec47d33c23e464a17cdf2475632cba629bee8763

use std::fmt;
use std::sync::Arc;

use tsc_ast::Diagnostic;
use tsc_core::compileroptions::ResolutionMode;
use tsc_core::options_generated::CompilerOptions;
use tsc_tspath::{
    EXTENSION_JSON, SUPPORTED_DECLARATION_EXTENSIONS, SUPPORTED_JS_EXTENSIONS_FLAT,
    SUPPORTED_TS_IMPLEMENTATION_EXTENSIONS, PathKey, RootedDirectoryPath, RootedFilePath,
};

use crate::cache::ResolutionData;

// ResolutionHost is the host interface the resolver uses to interact with the
// containing environment.
//
// PORT: Go holds `ResolutionHost` as an interface value; the port mirrors the
// vfs crate's `Arc<dyn Fs>` precedent with `Arc<dyn ResolutionHost>` (hosts
// cross threads exactly like `vfs.FS` values do in Go).
pub trait ResolutionHost: Send + Sync {
    /// FS returns the filesystem to resolve within.
    fn fs(&self) -> Arc<dyn tsc_vfs::Fs>;
    /// GetCurrentDirectory returns the directory to resolve relative paths
    /// against.
    fn get_current_directory(&self) -> RootedDirectoryPath;
}

/// The result of a module-name resolution: Go returns
/// `(*ResolvedModule, []DiagAndArgs, error)` where the error is always nil at
/// this revision (both implementations return nil); the port drops the
/// vestigial error — reintroduce it when the compiler crate ports Go's
/// Resolver consumers.
pub type ResolveModuleNameResult = (Option<Arc<ResolvedModule>>, Vec<crate::resolver::DiagAndArgs>);

/// The result of a type-reference-directive resolution (no error channel in Go).
pub type ResolveTypeReferenceDirectiveResult =
    (Option<Arc<ResolvedTypeReferenceDirective>>, Vec<crate::resolver::DiagAndArgs>);

// Resolver is the interface for a module resolver.
//
// PORT (trait design): Go's `Resolver` interface has exactly two
// implementations (DefaultResolver, StaticResolver); the port keeps the
// interface as an object-safe trait used via `Arc<dyn Resolver>`, matching
// the vfs crate's `Arc<dyn Fs>` precedent for Go interface values.
pub trait Resolver: Send + Sync {
    fn base_directory(&self) -> RootedDirectoryPath;
    fn resolve_module_name(
        &self,
        module_name: &str,
        containing_file: &RootedFilePath,
        resolution_mode: ResolutionMode,
        redirected_reference: Option<&Arc<dyn ResolvedProjectReference>>,
    ) -> ResolveModuleNameResult;
    fn resolve_module_name_from_directory(
        &self,
        module_name: &str,
        containing_directory: &RootedDirectoryPath,
        resolution_mode: ResolutionMode,
    ) -> ResolveModuleNameResult;
    fn resolve_type_reference_directive(
        &self,
        type_reference_directive_name: &str,
        containing_file: &RootedFilePath,
        resolution_mode: ResolutionMode,
        redirected_reference: Option<&Arc<dyn ResolvedProjectReference>>,
    ) -> ResolveTypeReferenceDirectiveResult;
    fn get_resolution_data(&self) -> Arc<ResolutionData>;
}

/// Go: `type ModeAwareCacheKey struct { Name string; Mode core.ResolutionMode }`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ModeAwareCacheKey {
    pub name: String,
    pub mode: ResolutionMode,
}

/// Go: `type ModeAwareCache[T any] map[ModeAwareCacheKey]T`.
///
/// PORT: Go's bare map type becomes an alias over `FxHashMap` (the port's
/// backing map for un-ordered keyed lookups, per the collections crate).
pub type ModeAwareCache<T> = rustc_hash::FxHashMap<ModeAwareCacheKey, T>;

// ResolvedProjectReference is the surface the resolver needs from a resolved
// project reference.
pub trait ResolvedProjectReference: Send + Sync {
    fn config_name(&self) -> &RootedFilePath;
    fn compiler_options(&self) -> Option<&CompilerOptions>;
}

// NodeResolutionFeatures is a bitfield of enabled module resolution features.
//
// PORT: Go's `type NodeResolutionFeatures int32` const block becomes a
// newtype with the exact Go bit values and Go-parity const names (the
// `non_upper_case_globals` allow follows the gen-options precedent for
// Go-parity constants).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NodeResolutionFeatures(pub i32);

#[allow(non_upper_case_globals)]
impl NodeResolutionFeatures {
    pub const NodeResolutionFeaturesImports: Self = Self(1 << 0);
    pub const NodeResolutionFeaturesSelfName: Self = Self(1 << 1);
    pub const NodeResolutionFeaturesExports: Self = Self(1 << 2);
    pub const NodeResolutionFeaturesExportsPatternTrailers: Self = Self(1 << 3);
    /// allowing `#/` root imports in package.json imports field
    /// not supported until mass adoption - https://github.com/nodejs/node/pull/60864
    pub const NodeResolutionFeaturesImportsPatternRoot: Self = Self(1 << 4);

    pub const NodeResolutionFeaturesNone: Self = Self(0);
    pub const NodeResolutionFeaturesAll: Self = Self(
        Self::NodeResolutionFeaturesImports.0
            | Self::NodeResolutionFeaturesSelfName.0
            | Self::NodeResolutionFeaturesExports.0
            | Self::NodeResolutionFeaturesExportsPatternTrailers.0
            | Self::NodeResolutionFeaturesImportsPatternRoot.0,
    );
    pub const NodeResolutionFeaturesNode16Default: Self = Self(
        Self::NodeResolutionFeaturesImports.0
            | Self::NodeResolutionFeaturesSelfName.0
            | Self::NodeResolutionFeaturesExports.0
            | Self::NodeResolutionFeaturesExportsPatternTrailers.0,
    );
    pub const NodeResolutionFeaturesNodeNextDefault: Self = Self::NodeResolutionFeaturesAll;
    pub const NodeResolutionFeaturesBundlerDefault: Self = Self(
        Self::NodeResolutionFeaturesImports.0
            | Self::NodeResolutionFeaturesSelfName.0
            | Self::NodeResolutionFeaturesExports.0
            | Self::NodeResolutionFeaturesExportsPatternTrailers.0
            | Self::NodeResolutionFeaturesImportsPatternRoot.0,
    );

    /// Go: `features&other != 0`.
    #[inline]
    pub const fn intersects(self, other: Self) -> bool {
        (self.0 & other.0) != 0
    }
    /// Go: `features&other == other`.
    #[inline]
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
    #[inline]
    pub const fn bits(self) -> i32 {
        self.0
    }
}

impl std::ops::BitAnd for NodeResolutionFeatures {
    type Output = Self;
    #[inline]
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

impl std::ops::BitAndAssign for NodeResolutionFeatures {
    #[inline]
    fn bitand_assign(&mut self, rhs: Self) {
        self.0 &= rhs.0;
    }
}

impl std::ops::BitOr for NodeResolutionFeatures {
    type Output = Self;
    #[inline]
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for NodeResolutionFeatures {
    #[inline]
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl std::ops::Not for NodeResolutionFeatures {
    type Output = Self;
    #[inline]
    fn not(self) -> Self {
        Self(!self.0)
    }
}

/// PackageId identifies a package containing a module.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PackageId {
    pub name: String,
    pub sub_module_name: String,
    pub version: String,
    pub peer_dependencies: String,
}

impl PackageId {
    /// Go: `func (p *PackageId) PackageName() string`.
    pub fn package_name(&self) -> String {
        if !self.sub_module_name.is_empty() {
            return format!("{}/{}", self.name, self.sub_module_name);
        }
        self.name.clone()
    }
}

// Go: `func (p *PackageId) String() string` — `fmt.Sprintf("%s@%s%s", ...)`.
impl fmt::Display for PackageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}@{}{}",
            self.package_name(),
            self.version,
            self.peer_dependencies
        )
    }
}

/// ResolvedModule is the result of a module name resolution.
#[derive(Clone, Debug, Default)]
pub struct ResolvedModule {
    pub resolution_diagnostics: Vec<Arc<Diagnostic>>,
    pub resolved_file_name: RootedFilePath,
    pub resolved_path: PathKey,
    pub original_path: RootedFilePath,
    pub extension: String,
    pub resolved_using_ts_extension: bool,
    pub resolved_using_extra_extensions: bool,
    pub is_external_library_import: bool,
    pub is_custom_resolution: bool,
    pub package_id: PackageId,
    pub alternate_result: RootedFilePath,
}

impl ResolvedModule {
    /// Go: `func (r *ResolvedModule) IsResolved() bool { return r != nil && ... }`.
    ///
    /// PORT: the nil-receiver check is the collections-crate convention
    /// (packagejson::InfoCacheEntry::exists): an associated fn over
    /// `Option<&Self>`, so nil results read
    /// `ResolvedModule::is_resolved(r.as_deref())`.
    pub fn is_resolved(this: Option<&Self>) -> bool {
        this.is_some_and(|r| !r.resolved_file_name.is_empty())
    }
}

/// ResolvedTypeReferenceDirective is the result of a type reference directive
/// resolution.
#[derive(Clone, Debug, Default)]
pub struct ResolvedTypeReferenceDirective {
    pub resolution_diagnostics: Vec<Arc<Diagnostic>>,
    pub primary: bool,
    pub resolved_file_name: RootedFilePath,
    pub resolved_path: PathKey,
    pub original_path: RootedFilePath,
    pub package_id: PackageId,
    pub is_external_library_import: bool,
}

impl ResolvedTypeReferenceDirective {
    /// Go: `func (r *ResolvedTypeReferenceDirective) IsResolved() bool` —
    /// the Go method has no nil-receiver guard (the resolver never stores a
    /// nil result), so the port mirrors that as a plain method.
    pub fn is_resolved(&self) -> bool {
        !self.resolved_file_name.is_empty()
    }
}

// extensions is a bitfield of file types to look for.
//
// PORT: Go's unexported `extensions int32` const block becomes a pub(crate)
// newtype with Go-parity const names.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Extensions(pub i32);

#[allow(non_upper_case_globals)]
impl Extensions {
    pub const extensionsTypeScript: Self = Self(1 << 0);
    pub const extensionsJavaScript: Self = Self(1 << 1);
    pub const extensionsDeclaration: Self = Self(1 << 2);
    pub const extensionsJson: Self = Self(1 << 3);

    pub const extensionsImplementationFiles: Self =
        Self(Self::extensionsTypeScript.0 | Self::extensionsJavaScript.0);

    /// Go: `extensions&other != 0`.
    #[inline]
    pub const fn intersects(self, other: Self) -> bool {
        (self.0 & other.0) != 0
    }
    #[inline]
    pub const fn bits(self) -> i32 {
        self.0
    }
}

impl std::ops::BitAnd for Extensions {
    type Output = Self;
    #[inline]
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

impl std::ops::BitAndAssign for Extensions {
    #[inline]
    fn bitand_assign(&mut self, rhs: Self) {
        self.0 &= rhs.0;
    }
}

impl std::ops::BitOr for Extensions {
    type Output = Self;
    #[inline]
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for Extensions {
    #[inline]
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl std::ops::Not for Extensions {
    type Output = Self;
    #[inline]
    fn not(self) -> Self {
        Self(!self.0)
    }
}

// Go: `func (e extensions) String() string`.
impl fmt::Display for Extensions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut result: Vec<&str> = Vec::with_capacity(self.0.count_ones() as usize);
        if self.intersects(Self::extensionsTypeScript) {
            result.push("TypeScript");
        }
        if self.intersects(Self::extensionsJavaScript) {
            result.push("JavaScript");
        }
        if self.intersects(Self::extensionsDeclaration) {
            result.push("Declaration");
        }
        if self.intersects(Self::extensionsJson) {
            result.push("JSON");
        }
        f.write_str(&result.join(", "))
    }
}

impl Extensions {
    /// Go: `func (e extensions) Array() []string`.
    pub fn array(&self) -> Vec<String> {
        let mut result: Vec<String> = Vec::new();
        if self.intersects(Self::extensionsTypeScript) {
            result.extend(
                SUPPORTED_TS_IMPLEMENTATION_EXTENSIONS
                    .iter()
                    .map(|s| s.to_string()),
            );
        }
        if self.intersects(Self::extensionsJavaScript) {
            result.extend(SUPPORTED_JS_EXTENSIONS_FLAT.iter().map(|s| s.to_string()));
        }
        if self.intersects(Self::extensionsDeclaration) {
            result.extend(SUPPORTED_DECLARATION_EXTENSIONS.iter().map(|s| s.to_string()));
        }
        if self.intersects(Self::extensionsJson) {
            result.push(EXTENSION_JSON.to_string());
        }
        result
    }
}

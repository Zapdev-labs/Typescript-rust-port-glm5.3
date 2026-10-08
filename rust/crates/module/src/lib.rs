// Ported from tsc/internal/module @ ec47d33c23e464a17cdf2475632cba629bee8763
//
//! Package module implements TypeScript's module resolution: node16/nodenext/
//! bundler node-like resolution, package.json exports/imports/typesVersions
//! walking, paths/rootDirs mapping, typeRoots lookup, and the module
//! resolution cache.
//!
//! File map (one Go file = one Rust file):
//!   - types.rs          (Go types.go)  ResolutionHost, Resolver trait,
//!                                      PackageId, ResolvedModule, ...,
//!                                      NodeResolutionFeatures, Extensions.
//!   - util.rs           (Go util.go)   package-name mangling, versioned-types
//!                                      key, GetResolutionDiagnostic, ...
//!   - cache.rs          (Go cache.go)  resolution caches, ResolutionData.
//!   - resolver.rs       (Go resolver.go) DefaultResolver + resolutionState.
//!   - staticresolver.rs (Go staticresolver.go)
//!   - vfsmatch.rs       (PORT: tsc/internal/vfs/vfsmatch/vfsmatch.go's
//!     ReadDirectory, ported in-crate because the resolver's
//!     GetEntrypointsFromPackageJsonInfo needs it and the vfs crate
//!     deliberately did not port the vfsmatch subpackage; hoist into tsc-vfs
//!     when a second consumer appears.)

pub mod cache;
pub mod resolver;
pub mod staticresolver;
pub mod types;
pub mod util;
pub mod vfsmatch;

pub use cache::{
    ModeAwareCache, ModeAwareCacheKey, ResolutionData, get_redirect_config_name,
    new_resolution_data,
};
pub use resolver::{
    DefaultResolver, DiagAndArgs, Ending, ParsedPatterns, ResolvedEntrypoint, ResolverOptions,
    get_automatic_type_directive_names, get_compiler_options_with_redirect, get_conditions,
    match_pattern_or_exact, new_resolver, new_resolver_with_options, resolve_config,
    try_parse_patterns,
};
pub use staticresolver::{
    NewStaticResolutionsResult, StaticResolutionEntry, StaticResolutions, StaticResolver,
    new_static_resolver, new_static_resolutions,
};
pub use types::{
    NodeResolutionFeatures, PackageId, ResolvedModule, ResolvedProjectReference, ResolvedTypeReferenceDirective,
    ResolutionHost, Resolver,
};
pub use util::{
    ComparePatternKeys as compare_pattern_keys, GetPackageNameFromTypesPackageName,
    GetResolutionDiagnostic, GetTypesPackageName, InferredTypesContainingFile,
    IsApplicableVersionedTypesKey, MangleScopedPackageName, NodeModulePackageRootForDirectory,
    NodeModulePackageRootForFile, ParsePackageName, TryGetJSExtensionForFile,
    TryGetJSExtensionForFileName, UnmangleScopedPackageName,
};

#[cfg(test)]
mod resolver_internal_test;
#[cfg(test)]
mod resolver_test;
#[cfg(test)]
mod staticresolver_test;

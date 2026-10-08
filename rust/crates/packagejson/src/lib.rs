// Ported from tsc/internal/packagejson @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Package packagejson reads and caches package.json files.
//
// Go's flat `packagejson.X` surface is mirrored by the flat re-exports below.
// Go struct embedding has no Rust counterpart:
//   - `Fields` embeds HeaderFields + PathFields + DependencyFields — nested in
//     Rust as `header_fields` / `path_fields` / `dependency_fields`, with
//     delegating impls for the promoted method surface.
//   - `PackageJson` embeds Fields — nested as `fields`.
//   - `ExportsOrImports` embeds JSONValue — flattened into the same
//     (Type, payload) shape with the scalar accessors re-implemented on both.

pub mod cache;
pub mod expected;
pub mod exportsorimports;
pub mod jsonvalue;
pub mod packagejson;
pub mod validated;

pub use cache::{
    InfoCache, InfoCacheEntry, PackageDirectory, PackageJson, VersionPaths,
    for_each_ancestor_directory_stopping_at_global_cache, new_info_cache, new_package_directory,
};
pub use expected::{Expected, ExpectedJsonType, expected_of};
pub use exportsorimports::ExportsOrImports;
pub use jsonvalue::{JSONValue, JSONValuePayload, JSONValueType};
pub use packagejson::{
    ContentMapperFields, DependencyFields, Fields, HeaderFields, PathFields, parse,
};
pub use validated::TypeValidatedField;

#[cfg(test)]
mod exportsorimports_test;
#[cfg(test)]
mod expected_test;
#[cfg(test)]
mod jsonvalue_test;
#[cfg(test)]
mod packagejson_test;

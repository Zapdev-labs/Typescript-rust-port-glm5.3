// Ported from tsc/internal/packagejson/packagejson.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Go name mapping:
//   HeaderFields / PathFields / DependencyFields / Fields /
//   ContentMapperFields                  → same
//   typeScriptFields                     → TypeScriptFields (pub(crate); unexported in Go)
//   HasDependency                        → has_dependency
//   RangeDependencies                    → range_dependencies
//   GetRuntimeDependencyNames            → get_runtime_dependency_names
//   Parse                                → parse
//
// PORT: Go promotes the embedded HeaderFields/PathFields/DependencyFields onto
// `Fields` (fields.Name, fields.Main, ...) and the promoted methods further up
// onto `PackageJson`; the port nests the groups (`header_fields`, `path_fields`,
// `dependency_fields`) and adds delegating impls for the promoted methods at
// each embedding level.
//
// The JSON member names are Go's `json:"..."` tags verbatim. `Fields`,
// `TypeScriptFields`, and `ContentMapperFields` use hand-rolled duplicate-
// tolerant map visitors instead of derived Deserialize: Go decodes
// package.json with `json.AllowDuplicateNames(true)` (later occurrences win),
// while serde's derive rejects duplicate fields outright. JSONValue /
// ExportsOrImports / OrderedMap are duplicate-tolerant already (their maps
// overwrite in place).
//
// Go's `map[string]string` dependency maps become `HashMap<String, String>`
// (unordered in both languages; no consumer iterates them in order).

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;

use serde::de::{IgnoredAny, MapAccess, Visitor};
use serde::Deserialize;
use tsc_collections::Set;

use crate::expected::Expected;
use crate::exportsorimports::ExportsOrImports;
use crate::jsonvalue::JSONValue;

/// HeaderFields are the identifying package.json fields.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HeaderFields {
    pub name: Expected<String>,
    pub version: Expected<String>,
    pub type_: Expected<String>,
}

/// PathFields are the package.json fields that influence module resolution.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PathFields {
    pub tsconfig: Expected<String>,
    pub main: Expected<String>,
    pub types: Expected<String>,
    pub typings: Expected<String>,
    pub types_versions: JSONValue,
    pub imports: ExportsOrImports,
    pub exports: ExportsOrImports,
}

/// DependencyFields are the package.json dependency tables.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DependencyFields {
    pub dependencies: Expected<HashMap<String, String>>,
    pub dev_dependencies: Expected<HashMap<String, String>>,
    pub peer_dependencies: Expected<HashMap<String, String>>,
    pub optional_dependencies: Expected<HashMap<String, String>>,
}

impl DependencyFields {
    /// HasDependency returns true if the package.json has a dependency with the
    /// given name under any of the dependency fields (dependencies,
    /// devDependencies, peerDependencies, optionalDependencies).
    pub fn has_dependency(&self, name: &str) -> bool {
        for deps in [
            &self.dependencies,
            &self.dev_dependencies,
            &self.peer_dependencies,
            &self.optional_dependencies,
        ] {
            if let Some(deps) = deps.get_value() {
                if deps.contains_key(name) {
                    return true;
                }
            }
        }
        false
    }

    /// RangeDependencies calls f with every (name, version, field) across the
    /// dependency fields, stopping as soon as f returns false. The field names
    /// are the Go literal strings ("dependencies", "devDependencies", ...).
    pub fn range_dependencies(&self, mut f: impl FnMut(&str, &str, &str) -> bool) {
        for (field, deps) in [
            ("dependencies", &self.dependencies),
            ("devDependencies", &self.dev_dependencies),
            ("peerDependencies", &self.peer_dependencies),
            ("optionalDependencies", &self.optional_dependencies),
        ] {
            if let Some(deps) = deps.get_value() {
                for (name, version) in deps {
                    if !f(name, version, field) {
                        return;
                    }
                }
            }
        }
    }

    /// GetRuntimeDependencyNames collects the runtime dependency names —
    /// dependencies + peerDependencies + optionalDependencies (devDependencies
    /// deliberately excluded).
    pub fn get_runtime_dependency_names(&self) -> Set<String> {
        let mut count = 0;
        if let Some(deps) = self.dependencies.get_value() {
            count += deps.len();
        }
        if let Some(peer_deps) = self.peer_dependencies.get_value() {
            count += peer_deps.len();
        }
        if let Some(opt_deps) = self.optional_dependencies.get_value() {
            count += opt_deps.len();
        }
        let mut names = Set::with_size_hint(count);
        if let Some(deps) = self.dependencies.get_value() {
            for name in deps.keys() {
                names.add(name.clone());
            }
        }
        if let Some(peer_deps) = self.peer_dependencies.get_value() {
            for name in peer_deps.keys() {
                names.add(name.clone());
            }
        }
        if let Some(opt_deps) = self.optional_dependencies.get_value() {
            for name in opt_deps.keys() {
                names.add(name.clone());
            }
        }
        names
    }
}

/// ContentMapperFields are the `typescript.contentMapper` payload fields.
///
/// Hand-rolled duplicate-tolerant Deserialize (see the file banner).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ContentMapperFields {
    pub exec: Expected<Vec<String>>,
    pub compiler_options: Expected<Vec<String>>,
    pub dynamic_config: Expected<bool>,
}

// Go json tags: "exec", "compilerOptions", "dynamicConfig".
impl<'de> Deserialize<'de> for ContentMapperFields {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ContentMapperFieldsVisitor;

        impl<'de> Visitor<'de> for ContentMapperFieldsVisitor {
            type Value = ContentMapperFields;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a contentMapper object")
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(ContentMapperFields::default())
            }

            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                let mut exec = Expected::<Vec<String>>::default();
                let mut compiler_options = Expected::<Vec<String>>::default();
                let mut dynamic_config = Expected::<bool>::default();
                while let Some(key) = access.next_key::<Cow<'de, str>>()? {
                    match key.as_ref() {
                        "exec" => exec = access.next_value()?,
                        "compilerOptions" => compiler_options = access.next_value()?,
                        "dynamicConfig" => dynamic_config = access.next_value()?,
                        _ => {
                            access.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(ContentMapperFields {
                    exec,
                    compiler_options,
                    dynamic_config,
                })
            }
        }

        deserializer.deserialize_map(ContentMapperFieldsVisitor)
    }
}

/// Go: `type typeScriptFields struct` (unexported) — the `typescript` field.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct TypeScriptFields {
    pub content_mapper: Expected<ContentMapperFields>,
}

// Go json tag: "contentMapper". Hand-rolled duplicate-tolerant Deserialize
// (see the file banner).
impl<'de> Deserialize<'de> for TypeScriptFields {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct TypeScriptFieldsVisitor;

        impl<'de> Visitor<'de> for TypeScriptFieldsVisitor {
            type Value = TypeScriptFields;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a typescript object")
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(TypeScriptFields::default())
            }

            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                let mut content_mapper = Expected::<ContentMapperFields>::default();
                while let Some(key) = access.next_key::<Cow<'de, str>>()? {
                    match key.as_ref() {
                        "contentMapper" => content_mapper = access.next_value()?,
                        _ => {
                            access.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                Ok(TypeScriptFields { content_mapper })
            }
        }

        deserializer.deserialize_map(TypeScriptFieldsVisitor)
    }
}

/// Fields is every package.json field the compiler reads, grouped as Go
/// grouped them. The JSON member set is flat (Go's embedded structs flatten
/// during decode); `content_mapper` is not a JSON member of its own
/// (Go tag `json:"-"`) — it is filled from the `typescript` field.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fields {
    pub header_fields: HeaderFields,
    pub path_fields: PathFields,
    pub dependency_fields: DependencyFields,
    pub content_mapper: Expected<ContentMapperFields>,
}

impl Fields {
    /// Promoted from DependencyFields (Go struct embedding).
    pub fn has_dependency(&self, name: &str) -> bool {
        self.dependency_fields.has_dependency(name)
    }

    /// Promoted from DependencyFields (Go struct embedding).
    pub fn range_dependencies(&self, f: impl FnMut(&str, &str, &str) -> bool) {
        self.dependency_fields.range_dependencies(f);
    }

    /// Promoted from DependencyFields (Go struct embedding).
    pub fn get_runtime_dependency_names(&self) -> Set<String> {
        self.dependency_fields.get_runtime_dependency_names()
    }
}

// Fields is a valid unmarshal target on its own (Go's BenchmarkPackageJSON
// decodes straight into packagejson.Fields; Parse decodes into the same flat
// member set). This visitor is the port of Go's anonymous parse struct +
// `json.AllowDuplicateNames(true)`: later occurrences of a member win, and a
// top-level `null` is the no-op zero value (encoding/json's convention).
impl<'de> Deserialize<'de> for Fields {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct FieldsVisitor;

        impl<'de> Visitor<'de> for FieldsVisitor {
            type Value = Fields;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a package.json object")
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(Fields::default())
            }

            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                let mut name = Expected::<String>::default();
                let mut version = Expected::<String>::default();
                let mut type_ = Expected::<String>::default();
                let mut tsconfig = Expected::<String>::default();
                let mut main = Expected::<String>::default();
                let mut types = Expected::<String>::default();
                let mut typings = Expected::<String>::default();
                let mut types_versions = JSONValue::default();
                let mut imports = ExportsOrImports::default();
                let mut exports = ExportsOrImports::default();
                let mut dependencies = Expected::<HashMap<String, String>>::default();
                let mut dev_dependencies = Expected::<HashMap<String, String>>::default();
                let mut peer_dependencies = Expected::<HashMap<String, String>>::default();
                let mut optional_dependencies = Expected::<HashMap<String, String>>::default();
                let mut typescript = Expected::<TypeScriptFields>::default();
                while let Some(key) = access.next_key::<Cow<'de, str>>()? {
                    match key.as_ref() {
                        "name" => name = access.next_value()?,
                        "version" => version = access.next_value()?,
                        "type" => type_ = access.next_value()?,
                        "tsconfig" => tsconfig = access.next_value()?,
                        "main" => main = access.next_value()?,
                        "types" => types = access.next_value()?,
                        "typings" => typings = access.next_value()?,
                        "typesVersions" => types_versions = access.next_value()?,
                        "imports" => imports = access.next_value()?,
                        "exports" => exports = access.next_value()?,
                        "dependencies" => dependencies = access.next_value()?,
                        "devDependencies" => dev_dependencies = access.next_value()?,
                        "peerDependencies" => peer_dependencies = access.next_value()?,
                        "optionalDependencies" => optional_dependencies = access.next_value()?,
                        "typescript" => typescript = access.next_value()?,
                        _ => {
                            access.next_value::<IgnoredAny>()?;
                        }
                    }
                }
                // Go: `typeScript, _ := parsed.TypeScript.GetValue()` — the
                // value is taken whether or not it parsed (an invalid parse
                // holds the zero value, whose contentMapper is the zero
                // Expected).
                Ok(Fields {
                    header_fields: HeaderFields {
                        name,
                        version,
                        type_,
                    },
                    path_fields: PathFields {
                        tsconfig,
                        main,
                        types,
                        typings,
                        types_versions,
                        imports,
                        exports,
                    },
                    dependency_fields: DependencyFields {
                        dependencies,
                        dev_dependencies,
                        peer_dependencies,
                        optional_dependencies,
                    },
                    content_mapper: typescript.value.content_mapper,
                })
            }
        }

        deserializer.deserialize_map(FieldsVisitor)
    }
}

/// Go: `func Parse(data []byte) (Fields, error)`.
pub fn parse(data: &[u8]) -> Result<Fields, serde_json::Error> {
    tsc_json::unmarshal::<Fields>(data, &[tsc_json::allow_duplicate_names(true)])
}

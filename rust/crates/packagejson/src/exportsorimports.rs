// Ported from tsc/internal/packagejson/exportsorimports.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Go name mapping:
//   ExportsOrImports                        → ExportsOrImports
//   UnmarshalJSONFrom                       → UnmarshalerFrom impl (tsc-json hook)
//   IsSubpaths/IsImports/IsConditions       → is_subpaths/is_imports/is_conditions
//   initObjectKind                          → init_object_kind
//
// PORT: Go embeds JSONValue (so ExportsOrImports shares its Type/Value and the
// scalar accessors) while overriding AsObject/AsArray with the
// ExportsOrImports-typed element maps. The port flattens the embed into the
// same (type_, value) shape with `JSONValuePayload<ExportsOrImports>` elements
// and re-implements the JSONValue surface; the panic messages follow each Go
// method exactly ("expected object"/"expected array" for the overrides, the
// "expected string, got %v" shape for the promoted accessor).

use std::io;

use serde::Deserialize;
use tsc_collections::OrderedMap;
use tsc_json::{Decoder, UnmarshalerFrom, deserialize_unmarshaler_from};

use crate::jsonvalue::{
    JSONValuePayload, JSONValueType, ParseJSONValueElement, unmarshal_json_value_from,
};

// Go: `type objectKind int8` (+ objectKindUnknown..objectKindInvalid consts).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(i8)]
enum ObjectKind {
    #[default]
    Unknown = 0,
    Subpaths = 1,
    Conditions = 2,
    Imports = 3,
    Invalid = 4,
}

/// ExportsOrImports models a package.json `exports` or `imports` field: a
/// possibly nested tree whose top-level object shape classifies the
/// conditional-exports resolution surface:
///   - subpaths:   keys starting with "." (a subpath map)
///   - imports:    keys starting with "#" (a subpath-imports map)
///   - conditions: bare condition names ("node", "import", "default"), or an
///     empty object
///   - invalid:    a mixture of the above kinds (never resolvable)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExportsOrImports {
    pub type_: JSONValueType,
    pub value: JSONValuePayload<ExportsOrImports>,
}

impl ExportsOrImports {
    /// Promoted from the embedded JSONValue (Go: `IsPresent`).
    pub fn is_present(&self) -> bool {
        self.type_ != JSONValueType::NotPresent
    }

    /// Promoted from the embedded JSONValue (Go: `IsFalsy`).
    pub fn is_falsy(&self) -> bool {
        self.value.is_falsy()
    }

    /// Go: `func (e ExportsOrImports) AsObject() *collections.OrderedMap[string, ExportsOrImports]`.
    pub fn as_object(&self) -> &OrderedMap<String, ExportsOrImports> {
        if self.type_ != JSONValueType::Object {
            panic!("expected object");
        }
        match &self.value {
            JSONValuePayload::Object(object) => object,
            _ => unreachable!("ExportsOrImports type/payload mismatch"),
        }
    }

    /// Go: `func (e ExportsOrImports) AsArray() []ExportsOrImports`.
    pub fn as_array(&self) -> &[ExportsOrImports] {
        if self.type_ != JSONValueType::Array {
            panic!("expected array");
        }
        match &self.value {
            JSONValuePayload::Array(elements) => elements,
            _ => unreachable!("ExportsOrImports type/payload mismatch"),
        }
    }

    /// Promoted from the embedded JSONValue (Go: `AsString`, with the embedded
    /// method's "got %v" panic message).
    pub fn as_string(&self) -> &str {
        if self.type_ != JSONValueType::String {
            panic!("expected string, got {}", self.type_);
        }
        match &self.value {
            JSONValuePayload::String(s) => s,
            _ => unreachable!("ExportsOrImports type/payload mismatch"),
        }
    }

    /// Go: `func (e ExportsOrImports) IsSubpaths() bool`.
    pub fn is_subpaths(&self) -> bool {
        self.init_object_kind() == ObjectKind::Subpaths
    }

    /// Go: `func (e ExportsOrImports) IsImports() bool`.
    pub fn is_imports(&self) -> bool {
        self.init_object_kind() == ObjectKind::Imports
    }

    /// Go: `func (e ExportsOrImports) IsConditions() bool`.
    pub fn is_conditions(&self) -> bool {
        self.init_object_kind() == ObjectKind::Conditions
    }

    /// Go: `func (e *ExportsOrImports) initObjectKind()` — Go lazily caches
    /// `objectKind` into the (copied) receiver, so the cache never survives the
    /// predicate call; the port recomputes per call (identical results, and the
    /// struct stays pure). Non-objects keep the Unknown kind, so every
    /// predicate is false for them.
    fn init_object_kind(&self) -> ObjectKind {
        if self.type_ != JSONValueType::Object {
            return ObjectKind::Unknown;
        }
        let obj = self.as_object();
        if obj.size() > 0 {
            let mut seen_dot = false;
            let mut seen_hash = false;
            let mut seen_other = false;
            for k in obj.keys() {
                if let Some(&first) = k.as_bytes().first() {
                    seen_dot |= first == b'.';
                    seen_hash |= first == b'#';
                    seen_other |= first != b'.' && first != b'#';
                    if seen_other && (seen_dot || seen_hash) {
                        return ObjectKind::Invalid;
                    }
                }
            }
            if seen_dot {
                return ObjectKind::Subpaths;
            }
            if seen_hash {
                return ObjectKind::Imports;
            }
        }
        ObjectKind::Conditions
    }
}

impl ParseJSONValueElement for ExportsOrImports {
    fn parse_json_value_element<R: io::Read>(
        dec: &mut Decoder<R>,
    ) -> Result<Self, serde_json::Error> {
        let (type_, value) = unmarshal_json_value_from::<ExportsOrImports, R>(dec)?;
        Ok(ExportsOrImports { type_, value })
    }
}

// Go: `var _ json.UnmarshalerFrom = (*ExportsOrImports)(nil)`.
impl UnmarshalerFrom for ExportsOrImports {
    fn unmarshal_json_from<R: io::Read>(
        &mut self,
        dec: &mut Decoder<R>,
    ) -> Result<(), serde_json::Error> {
        *self = Self::parse_json_value_element(dec)?;
        Ok(())
    }
}

impl<'de> Deserialize<'de> for ExportsOrImports {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_unmarshaler_from(deserializer)
    }
}

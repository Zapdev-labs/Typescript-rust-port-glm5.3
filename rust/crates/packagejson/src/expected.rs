// Ported from tsc/internal/packagejson/expected.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Go name mapping:
//   Expected[T]                        → Expected<T>
//   ExpectedOf                         → expected_of
//   UnmarshalJSON                      → Deserialize impl (serde hook)
//   IsPresent/GetValue/IsValid         → is_present/get_value/is_valid
//   ExpectedJSONType/ActualJSONType    → expected_json_type/actual_json_type
//
// PORT: Go derives `ExpectedJSONType` from `reflect.TypeFor[T]().Kind()`; Rust
// has no reflection, so the kinds used by the ported fields are enumerated in
// the `ExpectedJsonType` trait (String → "string", bool → "boolean", Vec →
// "array", maps → "object", integers → "number"; everything else — structs,
// `any`, floats (Go's switch omits the float kinds) — → "unknown", mirroring
// Go's `default` arm).

use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::value::RawValue;

use crate::packagejson::ContentMapperFields;
use crate::validated::TypeValidatedField;

/// Expected holds a value that may or may not have parsed as its expected
/// type: `valid` records a successful parse, `null` records a JSON null
/// (which Go deliberately does not attempt to parse), and `actual_json_type`
/// records the raw bytes' JSON classification (empty when the field was
/// absent from the document).
#[derive(Clone, Debug, Default)]
pub struct Expected<T> {
    actual_json_type: &'static str,
    pub null: bool,
    pub valid: bool,
    pub value: T,
}

impl<T> Expected<T> {
    /// Go: `func (e *Expected[T]) IsPresent() bool`.
    pub fn is_present(&self) -> bool {
        !self.actual_json_type.is_empty()
    }

    /// Go: `func (e *Expected[T]) GetValue() (value T, ok bool)` — the value is
    /// only usable when `ok`; Rust hands back a borrow instead of a copy.
    pub fn get_value(&self) -> Option<&T> {
        self.valid.then_some(&self.value)
    }

    /// Go: `func (e *Expected[T]) IsValid() bool`.
    pub fn is_valid(&self) -> bool {
        self.valid
    }

    /// Go: `func (e *Expected[T]) ActualJSONType() string`.
    pub fn actual_json_type(&self) -> &'static str {
        self.actual_json_type
    }
}

impl<T: ExpectedJsonType> Expected<T> {
    /// Go: `func (e *Expected[T]) ExpectedJSONType() string` (reflection on T).
    pub fn expected_json_type(&self) -> &'static str {
        T::expected_json_type()
    }
}

// Go: `func (e *Expected[T]) UnmarshalJSON(data []byte) error` — the port
// captures the same raw field bytes via Box<RawValue>, then mirrors Go's flow:
// a literal `null` only sets the Null/actual-type bits (the value keeps its
// zero state and the decode is not attempted); otherwise the bytes are
// classified by their first byte and a failed decode into T leaves `valid`
// unset without failing the surrounding document parse.
impl<'de, T> Deserialize<'de> for Expected<T>
where
    T: DeserializeOwned + Default,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = Box::<RawValue>::deserialize(deserializer)?;
        let data = raw.get().trim();
        if data == "null" {
            return Ok(Expected {
                null: true,
                actual_json_type: "null",
                ..Expected::default()
            });
        }
        let actual_json_type = match data.as_bytes().first() {
            Some(b'"') => "string",
            Some(b't') | Some(b'f') => "boolean",
            Some(b'[') => "array",
            Some(b'{') => "object",
            _ => "number",
        };
        // Go: `if json.Unmarshal(data, &e.Value) == nil { e.Valid = true }`.
        Ok(match serde_json::from_str::<T>(data) {
            Ok(value) => Expected {
                actual_json_type,
                null: false,
                valid: true,
                value,
            },
            Err(_) => Expected {
                actual_json_type,
                null: false,
                valid: false,
                value: T::default(),
            },
        })
    }
}

/// PORT: Go compares `Expected` values with `cmpopts.IgnoreUnexported(...)`,
/// which excludes `actualJSONType`; this PartialEq does the same (null/valid/
/// value only), so `expected_of(...)` compares equal to a byte-identical
/// decode.
impl<T: PartialEq> PartialEq for Expected<T> {
    fn eq(&self, other: &Self) -> bool {
        self.null == other.null && self.valid == other.valid && self.value == other.value
    }
}

/// Go: `func ExpectedOf[T any](value T) Expected[T]`.
pub fn expected_of<T: ExpectedJsonType>(value: T) -> Expected<T> {
    Expected {
        value,
        valid: true,
        null: false,
        actual_json_type: T::expected_json_type(),
    }
}

/// PORT: the replacement for Go's reflection-based `ExpectedJSONType`.
pub trait ExpectedJsonType {
    fn expected_json_type() -> &'static str;
}

impl ExpectedJsonType for String {
    fn expected_json_type() -> &'static str {
        "string"
    }
}

impl ExpectedJsonType for bool {
    fn expected_json_type() -> &'static str {
        "boolean"
    }
}

impl<T> ExpectedJsonType for Vec<T> {
    fn expected_json_type() -> &'static str {
        "array"
    }
}

impl<K, V> ExpectedJsonType for std::collections::HashMap<K, V> {
    fn expected_json_type() -> &'static str {
        "object"
    }
}

impl<K, V> ExpectedJsonType for std::collections::BTreeMap<K, V> {
    fn expected_json_type() -> &'static str {
        "object"
    }
}

// Go's `case reflect.Int ... reflect.Uint64` arm maps every integer kind to
// "number"; the float kinds fall through to `default` ("unknown").
impl ExpectedJsonType for i8 {
    fn expected_json_type() -> &'static str {
        "number"
    }
}

impl ExpectedJsonType for i16 {
    fn expected_json_type() -> &'static str {
        "number"
    }
}

impl ExpectedJsonType for i32 {
    fn expected_json_type() -> &'static str {
        "number"
    }
}

impl ExpectedJsonType for i64 {
    fn expected_json_type() -> &'static str {
        "number"
    }
}

impl ExpectedJsonType for u8 {
    fn expected_json_type() -> &'static str {
        "number"
    }
}

impl ExpectedJsonType for u16 {
    fn expected_json_type() -> &'static str {
        "number"
    }
}

impl ExpectedJsonType for u32 {
    fn expected_json_type() -> &'static str {
        "number"
    }
}

impl ExpectedJsonType for u64 {
    fn expected_json_type() -> &'static str {
        "number"
    }
}

impl ExpectedJsonType for f32 {
    fn expected_json_type() -> &'static str {
        "unknown"
    }
}

impl ExpectedJsonType for f64 {
    fn expected_json_type() -> &'static str {
        "unknown"
    }
}

// Go: struct kinds (ContentMapperFields) and interface kinds (`any`, i.e. the
// Go tests' Expected[any] → serde_json::Value) hit `default` ("unknown").
impl ExpectedJsonType for ContentMapperFields {
    fn expected_json_type() -> &'static str {
        "unknown"
    }
}

impl ExpectedJsonType for serde_json::Value {
    fn expected_json_type() -> &'static str {
        "unknown"
    }
}

// Go: `Expected[T]` satisfies TypeValidatedField implicitly (all four methods
// exist on the concrete type).
impl<T: ExpectedJsonType> TypeValidatedField for Expected<T> {
    fn is_present(&self) -> bool {
        Expected::is_present(self)
    }

    fn is_valid(&self) -> bool {
        self.valid
    }

    fn expected_json_type(&self) -> &'static str {
        T::expected_json_type()
    }

    fn actual_json_type(&self) -> &str {
        self.actual_json_type
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // PORT: no direct Go counterpart — pins the reflection mapping table.
    #[test]
    fn expected_json_type_mapping() {
        assert_eq!(<String as ExpectedJsonType>::expected_json_type(), "string");
        assert_eq!(<bool as ExpectedJsonType>::expected_json_type(), "boolean");
        assert_eq!(
            <Vec<String> as ExpectedJsonType>::expected_json_type(),
            "array"
        );
        assert_eq!(
            <std::collections::HashMap<String, String> as ExpectedJsonType>::expected_json_type(),
            "object"
        );
        assert_eq!(<i64 as ExpectedJsonType>::expected_json_type(), "number");
        assert_eq!(<f64 as ExpectedJsonType>::expected_json_type(), "unknown");
        assert_eq!(
            <ContentMapperFields as ExpectedJsonType>::expected_json_type(),
            "unknown"
        );

        let expected: Expected<String> = expected_of("x".to_string());
        assert_eq!(expected.actual_json_type(), "string");
        assert!(expected.is_present());
        assert!(expected.is_valid());
        assert_eq!(expected.get_value(), Some(&"x".to_string()));
    }
}

// Ported from tsc/internal/packagejson/jsonvalue.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Go name mapping:
//   JSONValueType (+ NotPresent..Object consts) → JSONValueType enum
//   JSONValueType.String                  → impl Display
//   JSONValue.IsPresent/IsFalsy            → is_present/is_falsy
//   JSONValue.AsObject/AsArray/AsString    → as_object/as_array/as_string
//   UnmarshalJSONFrom                      → UnmarshalerFrom impl (tsc-json hook)
//   unmarshalJSONValueFrom[T]              → unmarshal_json_value_from + ParseJSONValueElement
//
// PORT: Go stores the payload in a single untyped `Value any`; Rust needs one
// concrete type per payload shape, so the payload is the generic enum
// `JSONValuePayload<T>` where T is the concrete JSONValue-like type held by
// arrays and objects — `JSONValuePayload<JSONValue>` for JSONValue itself and
// `JSONValuePayload<ExportsOrImports>` for ExportsOrImports. That keeps Go's
// single generic decoder algorithm as one generic function over the element
// type.

use std::fmt;
use std::io;

use serde::Deserialize;
use tsc_collections::OrderedMap;
use tsc_json::{Decoder, UnmarshalerFrom, deserialize_unmarshaler_from, unmarshal_decode};

/// JSONValueType mirrors Go's `type JSONValueType int8`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(i8)]
pub enum JSONValueType {
    #[default]
    NotPresent = 0,
    Null = 1,
    String = 2,
    Number = 3,
    Boolean = 4,
    Array = 5,
    Object = 6,
}

// Go: `func (t JSONValueType) String() string` — the default arm prints
// "unknown(%d)" for every value outside the named set (i.e. NotPresent).
impl fmt::Display for JSONValueType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JSONValueType::Null => f.write_str("null"),
            JSONValueType::String => f.write_str("string"),
            JSONValueType::Number => f.write_str("number"),
            JSONValueType::Boolean => f.write_str("boolean"),
            JSONValueType::Array => f.write_str("array"),
            JSONValueType::Object => f.write_str("object"),
            JSONValueType::NotPresent => write!(f, "unknown({})", *self as i8),
        }
    }
}

/// The `Value any` payload of a JSONValue-like type, parameterized by the
/// concrete type held by arrays and objects.
///
/// `Null` is Go's nil `any` — the payload for both NotPresent and Null values
/// (distinguished by the `type_` field).
#[derive(Clone, Debug, Default)]
pub enum JSONValuePayload<T> {
    #[default]
    Null,
    Boolean(bool),
    Number(f64),
    String(String),
    Array(Vec<T>),
    Object(Box<OrderedMap<String, T>>),
}

// PORT: OrderedMap (Go's) has no PartialEq of its own — Go compares maps via
// reflect.DeepEqual in tests — so payload equality routes object payloads
// through OrderedMap::equal_func (insertion-ordered keys + value equality).
impl<T: PartialEq> PartialEq for JSONValuePayload<T> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (JSONValuePayload::Null, JSONValuePayload::Null) => true,
            (JSONValuePayload::Boolean(a), JSONValuePayload::Boolean(b)) => a == b,
            (JSONValuePayload::Number(a), JSONValuePayload::Number(b)) => a == b,
            (JSONValuePayload::String(a), JSONValuePayload::String(b)) => a == b,
            (JSONValuePayload::Array(a), JSONValuePayload::Array(b)) => a == b,
            (JSONValuePayload::Object(a), JSONValuePayload::Object(b)) => {
                OrderedMap::equal_func(Some(a.as_ref()), Some(b.as_ref()), |a, b| a == b)
            }
            _ => false,
        }
    }
}

impl<T> JSONValuePayload<T> {
    /// The IsFalsy switch over Go's `v.Value` (NotPresent and Null share the
    /// Null payload, so both are falsy).
    pub(crate) fn is_falsy(&self) -> bool {
        match self {
            JSONValuePayload::Null => true,
            JSONValuePayload::String(s) => s.is_empty(),
            JSONValuePayload::Number(n) => *n == 0.0,
            JSONValuePayload::Boolean(b) => !*b,
            _ => false,
        }
    }
}

/// JSONValue is an arbitrary JSON value.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JSONValue {
    pub type_: JSONValueType,
    pub value: JSONValuePayload<JSONValue>,
}

impl JSONValue {
    /// Go: `func (v *JSONValue) IsPresent() bool`.
    pub fn is_present(&self) -> bool {
        self.type_ != JSONValueType::NotPresent
    }

    /// Go: `func (v *JSONValue) IsFalsy() bool`.
    pub fn is_falsy(&self) -> bool {
        self.value.is_falsy()
    }

    /// Go: `func (v JSONValue) AsObject() *collections.OrderedMap[string, JSONValue]`.
    pub fn as_object(&self) -> &OrderedMap<String, JSONValue> {
        if self.type_ != JSONValueType::Object {
            panic!("expected object, got {}", self.type_);
        }
        match &self.value {
            JSONValuePayload::Object(object) => object,
            _ => unreachable!("JSONValue type/payload mismatch"),
        }
    }

    /// Go: `func (v JSONValue) AsArray() []JSONValue`.
    pub fn as_array(&self) -> &[JSONValue] {
        if self.type_ != JSONValueType::Array {
            panic!("expected array, got {}", self.type_);
        }
        match &self.value {
            JSONValuePayload::Array(elements) => elements,
            _ => unreachable!("JSONValue type/payload mismatch"),
        }
    }

    /// Go: `func (v JSONValue) AsString() string`.
    pub fn as_string(&self) -> &str {
        if self.type_ != JSONValueType::String {
            panic!("expected string, got {}", self.type_);
        }
        match &self.value {
            JSONValuePayload::String(s) => s,
            _ => unreachable!("JSONValue type/payload mismatch"),
        }
    }
}

/// PORT: Go's `unmarshalJSONValueFrom[T]` is generic over the element type T
/// of arrays/objects; this trait supplies "parse one element of the concrete
/// JSONValue-like type" so the decoder algorithm is shared between JSONValue
/// and ExportsOrImports.
pub(crate) trait ParseJSONValueElement: serde::de::DeserializeOwned + Sized {
    fn parse_json_value_element<R: io::Read>(
        dec: &mut Decoder<R>,
    ) -> Result<Self, serde_json::Error>;
}

/// Go: `func (v *JSONValue) unmarshalJSONValueFrom[T any](dec *json.Decoder) error`
/// — generalized to the element type; returns the parsed (Type, payload) pair.
pub(crate) fn unmarshal_json_value_from<T, R>(
    dec: &mut Decoder<R>,
) -> Result<(JSONValueType, JSONValuePayload<T>), serde_json::Error>
where
    R: io::Read,
    T: ParseJSONValueElement,
{
    match dec.peek_kind()? {
        // json.Null.Kind()
        b'n' => {
            dec.read_token()?;
            Ok((JSONValueType::Null, JSONValuePayload::Null))
        }
        b'"' => {
            let value: String = unmarshal_decode(dec, &[])?;
            Ok((JSONValueType::String, JSONValuePayload::String(value)))
        }
        b'[' => {
            dec.read_token()?;
            let mut elements = Vec::new();
            while dec.peek_kind()? != b']' {
                // json.EndArray.Kind()
                elements.push(T::parse_json_value_element(dec)?);
            }
            dec.read_token()?;
            Ok((JSONValueType::Array, JSONValuePayload::Array(elements)))
        }
        b'{' => {
            let object: OrderedMap<String, T> = unmarshal_decode(dec, &[])?;
            Ok((
                JSONValueType::Object,
                JSONValuePayload::Object(Box::new(object)),
            ))
        }
        // json.True.Kind(), json.False.Kind()
        b't' | b'f' => {
            let value: bool = unmarshal_decode(dec, &[])?;
            Ok((JSONValueType::Boolean, JSONValuePayload::Boolean(value)))
        }
        _ => {
            let value: f64 = unmarshal_decode(dec, &[])?;
            Ok((JSONValueType::Number, JSONValuePayload::Number(value)))
        }
    }
}

impl ParseJSONValueElement for JSONValue {
    fn parse_json_value_element<R: io::Read>(
        dec: &mut Decoder<R>,
    ) -> Result<Self, serde_json::Error> {
        let (type_, value) = unmarshal_json_value_from::<JSONValue, R>(dec)?;
        Ok(JSONValue { type_, value })
    }
}

// Go: `var _ json.UnmarshalerFrom = (*JSONValue)(nil)`.
impl UnmarshalerFrom for JSONValue {
    fn unmarshal_json_from<R: io::Read>(
        &mut self,
        dec: &mut Decoder<R>,
    ) -> Result<(), serde_json::Error> {
        *self = Self::parse_json_value_element(dec)?;
        Ok(())
    }
}

impl<'de> Deserialize<'de> for JSONValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserialize_unmarshaler_from(deserializer)
    }
}

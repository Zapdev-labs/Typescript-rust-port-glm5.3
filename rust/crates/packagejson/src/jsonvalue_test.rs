// Ported from tsc/internal/packagejson/jsonvalue_test.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Go name mapping:
//   TestJSONValue/UnmarshalJSONV2 → json_value — the Go suite runs the same
//   table through both decoder paths; both collapse onto the single
//   `json.Unmarshal` facade in the port.

use serde::Deserialize;

use crate::{JSONValue, JSONValuePayload, JSONValueType};

#[derive(Default, Deserialize)]
#[serde(default)]
struct PackageJson {
    private: JSONValue,
    #[serde(rename = "false")]
    false_: JSONValue,
    name: JSONValue,
    version: JSONValue,
    exports: JSONValue,
    imports: JSONValue,
    not_present: JSONValue,
}

#[test]
fn json_value() {
    let json_string = r#"{
        "private": true,
        "false": false,
        "name": "test",
        "version": 2,
        "exports": {
            ".": {
                "import": "./test.ts",
                "default": "./test.ts"
            },
            "./test": [
                "./test1.ts",
                "./test2.ts",
                null
            ],
            "./null": null
        },
        "imports": null
    }"#;

    let p: PackageJson = tsc_json::unmarshal(json_string.as_bytes(), &[]).expect("unmarshal");

    assert_eq!(p.private.type_, JSONValueType::Boolean);
    assert_eq!(p.private.value, JSONValuePayload::Boolean(true));

    assert_eq!(p.name.type_, JSONValueType::String);
    assert_eq!(p.name.value, JSONValuePayload::String("test".to_string()));

    assert_eq!(p.version.type_, JSONValueType::Number);
    assert_eq!(p.version.value, JSONValuePayload::Number(2.0));

    assert_eq!(p.exports.type_, JSONValueType::Object);
    assert_eq!(p.exports.as_object().size(), 3);
    let dot = p.exports.as_object().get_or_zero(".");
    assert_eq!(dot.type_, JSONValueType::Object);
    assert_eq!(
        dot.as_object().get_or_zero("import").value,
        JSONValuePayload::String("./test.ts".to_string())
    );

    let test = p.exports.as_object().get_or_zero("./test");
    assert_eq!(test.type_, JSONValueType::Array);
    assert_eq!(test.as_array().len(), 3);
    assert_eq!(
        test.as_array()[0].value,
        JSONValuePayload::String("./test1.ts".to_string())
    );
    assert_eq!(
        test.as_array()[1].value,
        JSONValuePayload::String("./test2.ts".to_string())
    );
    assert_eq!(test.as_array()[2].type_, JSONValueType::Null);

    assert_eq!(
        p.exports.as_object().get_or_zero("./null").type_,
        JSONValueType::Null
    );

    assert_eq!(p.imports.type_, JSONValueType::Null);
    assert_eq!(p.imports.value, JSONValuePayload::Null);

    assert_eq!(p.not_present.type_, JSONValueType::NotPresent);
    assert_eq!(p.not_present.value, JSONValuePayload::Null);
}

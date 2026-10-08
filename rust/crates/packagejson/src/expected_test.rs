// Ported from tsc/internal/packagejson/expected_test.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Go name mapping: TestExpected → expected. The Go suite races the v1 and v2
// decoder paths; both collapse onto the single `json.Unmarshal` facade in the
// port. Go's `Expected[any]` becomes `Expected<serde_json::Value>`.

use serde::Deserialize;

use crate::Expected;

#[derive(Default, Deserialize)]
#[serde(default)]
struct PackageJson {
    name: Expected<String>,
    version: Expected<String>,
    exports: Expected<serde_json::Value>,
    main: Expected<String>,
}

#[test]
fn expected() {
    let json_string = r#"{
        "name": "test",
        "version": 2,
        "exports": null
    }"#;

    let p: PackageJson = tsc_json::unmarshal(json_string.as_bytes(), &[]).expect("unmarshal");

    assert!(p.name.valid);
    assert_eq!(p.name.value, "test");

    assert!(!p.version.valid);
    assert_eq!(p.version.value, "");

    assert!(p.exports.null);
    assert!(!p.exports.valid);

    assert!(!p.main.valid);
    assert!(!p.main.null);
    assert_eq!(p.main.value, "");
}

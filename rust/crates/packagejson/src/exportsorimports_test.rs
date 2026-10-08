// Ported from tsc/internal/packagejson/exportsorimports_test.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// Go name mapping:
//   TestExports/UnmarshalJSONV2 → exports — the Go suite races both decoder
//   paths; both collapse onto the single `json.Unmarshal` facade in the port.

use serde::Deserialize;

use crate::{ExportsOrImports, JSONValueType};

#[derive(Default, Deserialize)]
#[serde(default)]
struct Exports {
    imports: ExportsOrImports,
    exports: ExportsOrImports,
}

#[test]
fn exports() {
    let json_string = r##"{
        "imports": {
            "#foo": {
                "import": "./foo.ts"
            }
        },
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
        }
    }"##;

    let e: Exports = tsc_json::unmarshal(json_string.as_bytes(), &[]).expect("unmarshal");

    assert!(e.exports.is_subpaths());
    assert_eq!(e.exports.as_object().size(), 3);
    let dot = e.exports.as_object().get_or_zero(".");
    assert!(dot.is_conditions());
    assert_eq!(dot.as_object().get_or_zero("import").type_, JSONValueType::String);
    assert_eq!(
        e.exports
            .as_object()
            .get_or_zero("./test")
            .as_array()[2]
            .type_,
        JSONValueType::Null
    );
    assert_eq!(
        e.exports.as_object().get_or_zero("./null").type_,
        JSONValueType::Null
    );

    assert!(e.imports.is_imports());
    assert_eq!(e.imports.as_object().size(), 1);
    let foo = e.imports.as_object().get_or_zero("#foo");
    assert!(foo.is_conditions());
    assert_eq!(
        foo.as_object().get_or_zero("import").type_,
        JSONValueType::String
    );
}

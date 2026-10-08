// Tests for the tsc-contentmapper port (contentmapper.go's portable surface
// + the tsoptions-side parseContentMapper wiring).
//
// PORT notes:
//   - Go's transform_test.go exercises `ParseResult` (hostimpl.go), which is
//     out of the M4 wave (the Host machinery is unported); these tests cover
//     the surface that IS ported: virtual extensions, identity, equality,
//     TransformIdentity determinism (FNV-1a-128 substitution), and the
//     generated declared-options marshal.

use tsc_contentmapper::{
    is_supported_virtual_extension, Definition, Manifest, Mapper,
};
use tsc_tspath::rooted_directory_path_from_absolute;

fn mapper(name: &str, version: &str, declared: &[&str]) -> Mapper {
    Mapper {
        definition: Definition {
            package: "@scope/mapper".to_string(),
            extensions: vec![".astro".to_string()],
            options: None,
        },
        manifest: Manifest {
            name: name.to_string(),
            version: version.to_string(),
            exec: Vec::new(),
            compiler_options: declared.iter().map(|s| s.to_string()).collect(),
            dynamic_config: false,
        },
        package_directory: rooted_directory_path_from_absolute("/node_modules/@scope/mapper"),
        contribution_id: String::new(),
    }
}

#[test]
fn test_is_supported_virtual_extension() {
    for ext in [
        ".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx", ".mts", ".cts", ".json",
    ] {
        assert!(is_supported_virtual_extension(ext), "{ext}");
    }
    assert!(!is_supported_virtual_extension(".astro"));
    assert!(!is_supported_virtual_extension(".js.js"));
    assert!(!is_supported_virtual_extension(""));
}

#[test]
fn test_identity() {
    // Contribution IDs dominate.
    let mut m = mapper("name", "1.0.0", &[]);
    m.contribution_id = "id-1".to_string();
    assert_eq!(m.identity(), "id-1 (name@1.0.0)");

    // name@version, then bare name, then empty.
    assert_eq!(mapper("name", "1.0.0", &[]).identity(), "name@1.0.0");
    assert_eq!(mapper("name", "", &[]).identity(), "name");
    assert_eq!(mapper("", "", &[]).identity(), "");

    // DiagnosticName prefers the manifest name, then the package.
    assert_eq!(mapper("name", "1.0.0", &[]).diagnostic_name(), "name");
    let mut m = mapper("", "", &[]);
    assert_eq!(m.diagnostic_name(), "@scope/mapper");
    m.contribution_id = "id-2".to_string();
    m.definition.package = String::new();
    assert_eq!(m.diagnostic_name(), "id-2");
}

#[test]
fn test_equals() {
    let a = mapper("name", "1.0.0", &[]);
    let mut b = a.clone();
    assert!(a.equals(&b));
    b.manifest.version = "2.0.0".to_string();
    assert!(!a.equals(&b));
    let mut c = a.clone();
    c.definition.extensions.push(".vue".to_string());
    assert!(!a.equals(&c));
    let mut d = a.clone();
    d.package_directory = rooted_directory_path_from_absolute("/other");
    assert!(!a.equals(&d));
}

/// TransformIdentity is a deterministic function of (identity, declared
/// options, definition options) — the xxh3 substitution keeps the contract
/// (same inputs → same fingerprint), not Go's exact values.
#[test]
fn test_transform_identity_is_deterministic() {
    let m = mapper("name", "1.0.0", &["target", "module"]);
    let mut options = tsc_core::options_generated::CompilerOptions::default();
    options.target = tsc_core::options_generated::ScriptTarget::ES2020;
    let first = m.transform_identity(Some(&options));
    let second = m.transform_identity(Some(&options));
    assert_eq!(first, second);

    options.target = tsc_core::options_generated::ScriptTarget::ES2015;
    let changed = m.transform_identity(Some(&options));
    assert_ne!(first, changed);

    // Different mapper identity → different fingerprint.
    let m2 = mapper("other", "1.0.0", &["target", "module"]);
    let other = m2.transform_identity(Some(&options));
    assert_ne!(changed, other);
}

/// `MarshalDeclaredOptions` walks only the manifest-declared compiler
/// options, in manifest order, skipping zero values (Go's reflection walk
/// over the 132 json tags).
#[test]
fn test_marshal_declared_options() {
    let m = mapper("name", "1.0.0", &["outDir", "target", "module", "noSuchOption"]);
    let mut options = tsc_core::options_generated::CompilerOptions::default();
    options.target = tsc_core::options_generated::ScriptTarget::ES2020;
    options
        .module
        = tsc_core::options_generated::ModuleKind::CommonJS;
    let (declared, err) = m.marshal_declared_options(Some(&options));
    assert!(err.is_none());
    let keys: Vec<&String> = declared.keys().collect();
    assert_eq!(keys, ["target", "module"], "zero and unknown options are skipped");
    assert_eq!(declared.get("target").unwrap().get(), "7");
    assert_eq!(declared.get("module").unwrap().get(), "1");

    // No options at all → empty map.
    let (declared, _) = m.marshal_declared_options(None);
    assert!(declared.is_empty());
}


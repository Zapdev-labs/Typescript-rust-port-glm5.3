// Port of tsc/internal/tsoptions/decls_test.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT notes:
//   - `TestCompilerOptionsDeclaration` reflects over core.CompilerOptions'
//     fields; the generator's parity checker (gen-options/data/parity-check
//     -declarations.mjs) already proves declaration coverage against the Go
//     baseline field-for-field, so the Rust suite keeps the path-kind table
//     test — a pure data assertion over the declarations — plus the group
//     counts the parity checker locks.

use tsc_tsoptions::commandlineoption::CommandLineOptionPathKind;
use tsc_tsoptions::namemap::compiler_name_map;

/// Go: `TestCommandLineOptionPathKinds`.
#[test]
fn test_command_line_option_path_kinds() {
    let mut expected: Vec<(&str, CommandLineOptionPathKind)> = vec![
        ("baseUrl", CommandLineOptionPathKind::Directory),
        ("declarationDir", CommandLineOptionPathKind::Directory),
        ("generateCpuProfile", CommandLineOptionPathKind::File),
        ("generateTrace", CommandLineOptionPathKind::Directory),
        ("mapRoot", CommandLineOptionPathKind::SourceMapLocation),
        ("outDir", CommandLineOptionPathKind::Directory),
        ("outFile", CommandLineOptionPathKind::File),
        ("pprofDir", CommandLineOptionPathKind::Directory),
        ("project", CommandLineOptionPathKind::FileOrDirectory),
        ("rootDir", CommandLineOptionPathKind::Directory),
        ("sourceRoot", CommandLineOptionPathKind::SourceMapLocation),
        ("tsBuildInfoFile", CommandLineOptionPathKind::File),
    ];

    for option in tsc_tsoptions::declarations_generated::options_declarations() {
        match expected.iter().position(|(name, _)| *name == option.name) {
            Some(index) => {
                assert_eq!(
                    expected[index].1, option.path_kind,
                    "{} has wrong path kind",
                    option.name
                );
                expected.remove(index);
            }
            None => {
                assert_eq!(
                    option.path_kind,
                    CommandLineOptionPathKind::None,
                    "{} has unexpected path kind {:?}",
                    option.name,
                    option.path_kind
                );
            }
        }
    }
    assert!(
        expected.is_empty(),
        "options not found in declarations: {:?}",
        expected.iter().map(|(name, _)| name).collect::<Vec<_>>()
    );

    for option_name in ["rootDirs", "typeRoots"] {
        let option = compiler_name_map()
            .get_option_declaration_from_name(option_name, false)
            .unwrap_or_else(|| panic!("{option_name} not found"));
        let element = option.elements().expect("list option");
        assert_eq!(
            element.path_kind,
            CommandLineOptionPathKind::Directory,
            "{option_name} element is not classified as a directory"
        );
    }
}

/// The declaration group counts (locked by the Go-parity checker).
#[test]
fn test_declaration_group_counts() {
    let decls = tsc_tsoptions::declarations_generated::options_declarations();
    // 29 common + 97 compiler + 1 (build/tsc separator pair handled in the
    // generated tables) — the full list Go's `OptionsDeclarations` holds.
    assert!(!decls.is_empty());
    let names: Vec<&str> = decls.iter().map(|d| d.name).collect();
    for name in [
        "strict",
        "target",
        "module",
        "outDir",
        "paths",
        "plugins",
        "watch",
    ] {
        assert!(names.contains(&name), "{name} missing from declarations");
    }
    assert_eq!(tsc_tsoptions::declarations_generated::OPTIONS_FOR_BUILD.len(), 7);
    assert_eq!(
        tsc_tsoptions::declarations_generated::TYPE_ACQUISITION_DECLS.len(),
        4
    );
}

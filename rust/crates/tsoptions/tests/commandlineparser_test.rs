// Port of tsc/internal/tsoptions/commandlineparser_test.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT notes:
//   - The Go table tests run through the `tsoptionstest` baseline machinery
//     (golden JS diagnostics via diagnosticwriter). The port asserts the
//     same observable outcomes directly: option values set on
//     `compiler_options()`, error counts, and file name collection.
//   - `TestRemovedWatchOptions` baselines are replaced by error-count
//     assertions; the removed options produce exactly one unknown-option
//     diagnostic each in both parser modes.

use tsc_core::tristate::Tristate;
use tsc_tspath::{
    rooted_directory_path_from_absolute, CaseSensitivity, RootedDirectoryPath,
};
use tsc_vfs::vfstest::from_map;

fn cwd() -> RootedDirectoryPath {
    rooted_directory_path_from_absolute("/project")
}

fn parse(args: &[&str]) -> tsc_tsoptions::ParsedCommandLine {
    let fs = from_map(Vec::<(String, String)>::new(), CaseSensitivity::CaseSensitive);
    tsc_tsoptions::parse_command_line(
        &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        fs.as_ref(),
        &cwd(),
    )
}

fn parse_build(args: &[&str]) -> tsc_tsoptions::ParsedBuildCommandLine {
    let fs = from_map(Vec::<(String, String)>::new(), CaseSensitivity::CaseSensitive);
    tsc_tsoptions::parse_build_command_line(
        &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        fs.as_ref(),
        &cwd(),
    )
}

/// Go: `TestCommandLineParseResult` (the concrete scenarios the port can
/// observe without the golden-file machinery).
#[test]
fn test_command_line_parse_result() {
    // --lib es6 0.ts (the enum map maps the key to its lib file name).
    let parsed = parse(&["--lib", "es6", "0.ts"]);
    assert!(parsed.errors.is_empty(), "errors: {:?}", parsed.errors.len());
    assert_eq!(
        parsed.compiler_options().lib,
        Some(vec!["lib.es2015.d.ts".to_string()])
    );
    assert_eq!(parsed.root_file_names_for_diagnostics, vec!["0.ts"]);

    // Parse multiple options of library flags.
    let parsed = parse(&["--lib", "es5,es2015.symbol.wellknown", "0.ts"]);
    assert!(parsed.errors.is_empty());
    assert_eq!(
        parsed.compiler_options().lib,
        Some(vec![
            "lib.es5.d.ts".to_string(),
            "lib.es2015.symbol.wellknown.d.ts".to_string()
        ])
    );

    // Parse invalid option of library flags — an error is produced, the
    // valid entries still land.
    let parsed = parse(&["--lib", "es5,invalidOption", "0.ts"]);
    assert_eq!(parsed.errors.len(), 1);
    assert_eq!(
        parsed.compiler_options().lib,
        Some(vec!["lib.es5.d.ts".to_string()])
    );

    // Parse empty options of --jsx (last argument, enum kind).
    let parsed = parse(&["0.ts", "--jsx"]);
    assert!(!parsed.errors.is_empty());

    // Parse empty options of --module.
    let parsed = parse(&["0.ts", "--module"]);
    assert!(!parsed.errors.is_empty());

    // Parse empty options of --lib: sets the empty (non-nil) list, one type error.
    let parsed = parse(&["0.ts", "--lib"]);
    assert_eq!(parsed.compiler_options().lib, Some(Vec::new()));
    assert_eq!(parsed.errors.len(), 1);

    // Parse immediately following command line argument of --lib: the
    // "-sourcemap"-prefixed next argument is not consumed as a lib value.
    let parsed = parse(&["0.ts", "--lib", "--sourcemap"]);
    assert_eq!(parsed.compiler_options().lib, Some(Vec::new()));

    // Parse explicit boolean flag value.
    let parsed = parse(&["--strictNullChecks", "false", "0.ts"]);
    assert_eq!(parsed.compiler_options().strict_null_checks, Tristate::TSFalse);
    assert_eq!(parsed.root_file_names_for_diagnostics, vec!["0.ts"]);

    // Parse non boolean argument after boolean flag: the flag sticks, the
    // argument stays a file name.
    let parsed = parse(&["--noImplicitAny", "t", "0.ts"]);
    assert_eq!(parsed.compiler_options().no_implicit_any, Tristate::TSTrue);
    assert_eq!(parsed.root_file_names_for_diagnostics, vec!["t", "0.ts"]);

    // Parse implicit boolean flag value.
    let parsed = parse(&["--strictNullChecks"]);
    assert_eq!(parsed.compiler_options().strict_null_checks, Tristate::TSTrue);

    // Parse multiple compiler flags with input files in the middle.
    let parsed = parse(&[
        "--module",
        "commonjs",
        "--target",
        "es5",
        "0.ts",
        "--lib",
        "es5,es2015.symbol.wellknown",
    ]);
    assert!(parsed.errors.is_empty());
    assert_eq!(parsed.root_file_names_for_diagnostics, vec!["0.ts"]);

    // Allows tsconfig only option to be set to null (no error).
    let parsed = parse(&["--composite", "null", "--tsBuildInfoFile", "null", "0.ts"]);
    assert_eq!(parsed.errors.len(), 0);
    assert_eq!(parsed.root_file_names_for_diagnostics, vec!["0.ts"]);
}

/// Go: `TestCommandLineParseResult`'s "Handles may only be used with --build
/// flags" scenario — build-only options surface the alternate-mode
/// diagnostic when parsed in compiler mode.
#[test]
fn test_build_flags_in_compiler_mode() {
    let parsed = parse(&["--build", "--clean", "--dry", "--force", "--verbose"]);
    // --build is consumed as the tscbuildoption (no error); every build
    // option after it reports "may only be used with --build".
    assert!(parsed.errors.len() >= 4);
}

/// Go: `TestCommandLineParseResult`'s "Handles did you mean for misspelt
/// flags" scenario.
#[test]
fn test_did_you_mean_for_misspelt_flags() {
    let parsed = parse(&["--declarations", "--allowTS"]);
    // --declarations is a removed option (unknown); --allowTS misspells
    // --allowTs.
    assert!(!parsed.errors.is_empty());
}

/// Go: `TestRemovedWatchOptions`.
#[test]
fn test_removed_watch_options() {
    for option in [
        "watchInterval",
        "watchFile",
        "watchDirectory",
        "fallbackPolling",
        "synchronousWatchDirectory",
        "excludeDirectories",
        "excludeFiles",
    ] {
        let args = ["--watch".to_string(), format!("--{option}")];
        let fs = from_map(Vec::<(String, String)>::new(), CaseSensitivity::CaseSensitive);
        let parsed = tsc_tsoptions::parse_command_line(&args, fs.as_ref(), &cwd());
        assert!(parsed.compiler_options().watch.is_true(), "{option}");
        assert_eq!(parsed.errors.len(), 1, "{option}");

        let build = tsc_tsoptions::parse_build_command_line(&args, fs.as_ref(), &cwd());
        assert!(build.compiler_options.watch.is_true(), "{option}");
        assert_eq!(build.errors.len(), 1, "{option}");
    }
}

/// Go: `TestResponseFileDoesNotPanic` — `@` with an empty or relative file
/// name produces a diagnostic instead of panicking.
#[test]
fn test_response_file_does_not_panic() {
    let parsed = parse(&["@"]);
    assert!(!parsed.errors.is_empty());

    // A relative name resolves against the current directory and simply
    // fails to read.
    let parsed = parse(&["@missing.rsp"]);
    assert!(!parsed.errors.is_empty());
}

/// Go: `TestResponseFileParsing` — response files split on whitespace with
/// quote support, and may reference further response files.
#[test]
fn test_response_file_parsing() {
    let fs = from_map(
        vec![
            (
                "/project/response1.rsp".to_string(),
                "--strict\n@response2.rsp\n0.ts\n".to_string(),
            ),
            (
                "/project/response2.rsp".to_string(),
                "--outDir dist\n\"a b.ts\"\n".to_string(),
            ),
        ],
        CaseSensitivity::CaseSensitive,
    );
    let parsed = tsc_tsoptions::parse_command_line(
        &["@response1.rsp".to_string()],
        fs.as_ref(),
        &cwd(),
    );
    assert!(parsed.errors.is_empty(), "errors: {}", parsed.errors.len());
    assert!(parsed.compiler_options().strict.is_true());
    assert_eq!(parsed.compiler_options().out_dir.as_string(), "/project/dist");
    // The nested response file's files land at their command position.
    assert_eq!(parsed.root_file_names_for_diagnostics, vec!["a b.ts", "0.ts"]);
}

/// Go: `TestParseCommandLineTypeRootsRelativePath`.
#[test]
fn test_parse_command_line_type_roots_relative_path() {
    let fs = from_map(Vec::<(String, String)>::new(), CaseSensitivity::CaseSensitive);
    let current_directory = rooted_directory_path_from_absolute("/home/project");
    let parsed = tsc_tsoptions::parse_command_line(
        &["--typeRoots".to_string(), "t".to_string()],
        fs.as_ref(),
        &current_directory,
    );
    let type_roots = parsed.compiler_options().type_roots.as_deref().unwrap();
    assert_eq!(type_roots.len(), 1);
    assert_eq!(type_roots[0].as_string(), "/home/project/t");
}

/// Go: `TestCustomConditionsNullOverride` — explicit null on the command
/// line clears a previously-set list option.
#[test]
fn test_custom_conditions_null_override() {
    let parsed = parse(&["--customConditions", "a,b", "--customConditions", "null"]);
    assert!(parsed.errors.is_empty(), "errors: {}", parsed.errors.len());
    // Go: the null override leaves CustomConditions nil — None here, NOT an
    // explicitly-empty list.
    assert!(parsed.compiler_options().custom_conditions.is_none());
    assert!(parsed.raw.get("customConditions").is_some());
    assert!(matches!(
        parsed.raw.get("customConditions"),
        Some(tsc_tsoptions::CompilerOptionsValue::Null)
    ));
}

/// Go: `TestParseBuildCommandLine` (the observable surface: option mapping,
/// the implicit "." project, and the nonsensical-combination errors).
#[test]
fn test_parse_build_command_line() {
    let parsed = parse_build(&["--dry", "--force", "--verbose", "project"]);
    assert!(parsed.compiler_options.watch.is_unknown());
    assert!(parsed.errors.is_empty());
    assert_eq!(parsed.projects, vec!["project".to_string()]);

    // tsc -b invoked with no extra arguments acts as if invoked with ".".
    let parsed = parse_build(&[]);
    assert_eq!(parsed.projects, vec![".".to_string()]);

    // clean+force cannot be combined.
    let parsed = parse_build(&["--clean", "--force"]);
    assert_eq!(parsed.errors.len(), 1);
    // clean+verbose cannot be combined.
    let parsed = parse_build(&["--clean", "--verbose"]);
    assert_eq!(parsed.errors.len(), 1);
    // clean+watch cannot be combined.
    let parsed = parse_build(&["--clean", "--watch"]);
    assert_eq!(parsed.errors.len(), 1);
    // watch+dry cannot be combined.
    let parsed = parse_build(&["--watch", "--dry"]);
    assert_eq!(parsed.errors.len(), 1);
}

/// The `ResolvedProjectPaths` lazy mapping roots project names against the
/// current directory (PORT: without core's project-reference config-name
/// resolution).
#[test]
fn test_resolved_project_paths() {
    let parsed = parse_build(&["project"]);
    assert_eq!(parsed.resolved_project_paths().len(), 1);
    assert_eq!(
        parsed.resolved_project_paths()[0].as_string(),
        "/project/project"
    );
}

// Ported from tsc/internal/compiler/program_test.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT notes (skipped Go tests — see PORTING-NOTES.md for the roll-up):
//   - TestProgramSharedData: Go's `testutil.CheckDataOnly` walks struct
//     layouts through reflection; Rust has no reflection (the ownership
//     rules encode the data-only discipline).
//   - TestProgramHostsAndFactories / TestClonedProgramProjectReferenceResolution:
//     depend on `ReuseProgram`/`UpdateProgram` (not ported — owned `files` +
//     the content-mapper seam) and `tracing` (not ported).
//   - BenchmarkNewProgram: the benchmark harness is the separate
//     bench-harness crate (out of this wave's touch scope); the test corpus
//     below covers the same program constructions.
//
// PORT notes (adapted constructions):
//   - Go's `tsoptions.NewParsedCommandLine(&opts, files, nil, base, cs)`
//     builds a zero config with an explicit file list; the Rust tsoptions
//     crate has no such constructor (`file_names` is not a pub field), so
//     the tests parse a written tsconfig.json (`files` entries are included
//     verbatim with no existence check, exactly as Go's literal list — the
//     same dataflow through the M4 config-parsing surface).
//   - Go's `NewCompilerHost(fs, dir, nil, nil, nil)` nil extended-config
//     cache becomes the always-miss closure.

use std::sync::Arc;

use tsc_compiler::file_include::{FileIncludeKind, FileIncludeReason};
use tsc_compiler::{
    new_compiler_host, new_program, Program, ProgramConfig, ProgramFactories, ProgramHosts,
    ProgramOptions,
};
use tsc_tsoptions::tsconfigparsing::ExtendedConfigCache;
use tsc_tsoptions::{get_parsed_command_line_of_config_file_path, CompilerOptionsValue};
use tsc_tspath::{
    rooted_directory_path_from_normalized, CaseSensitivity, PathKey, RootedDirectoryPath,
    RootedFilePath, RootedPath,
};
use tsc_vfs::vfstest;

// Go: `var esnextLibs = []string{...}` — the default lib file set for
// target esnext (the ordering corpus asserts the exact program order).
#[rustfmt::skip]
const ESNEXT_LIBS: &[&str] = &[
    "lib.es5.d.ts",
    "lib.es2015.d.ts",
    "lib.es2016.d.ts",
    "lib.es2017.d.ts",
    "lib.es2018.d.ts",
    "lib.es2019.d.ts",
    "lib.es2020.d.ts",
    "lib.es2021.d.ts",
    "lib.es2022.d.ts",
    "lib.es2023.d.ts",
    "lib.es2024.d.ts",
    "lib.es2025.d.ts",
    "lib.es2026.d.ts",
    "lib.esnext.d.ts",
    "lib.dom.d.ts",
    "lib.dom.iterable.d.ts",
    "lib.dom.asynciterable.d.ts",
    "lib.webworker.importscripts.d.ts",
    "lib.scripthost.d.ts",
    "lib.es2015.core.d.ts",
    "lib.es2015.collection.d.ts",
    "lib.es2015.generator.d.ts",
    "lib.es2015.iterable.d.ts",
    "lib.es2015.promise.d.ts",
    "lib.es2015.proxy.d.ts",
    "lib.es2015.reflect.d.ts",
    "lib.es2015.symbol.d.ts",
    "lib.es2015.symbol.wellknown.d.ts",
    "lib.es2016.array.include.d.ts",
    "lib.es2016.intl.d.ts",
    "lib.es2017.arraybuffer.d.ts",
    "lib.es2017.date.d.ts",
    "lib.es2017.object.d.ts",
    "lib.es2017.sharedmemory.d.ts",
    "lib.es2017.string.d.ts",
    "lib.es2017.intl.d.ts",
    "lib.es2017.typedarrays.d.ts",
    "lib.es2018.asyncgenerator.d.ts",
    "lib.es2018.asynciterable.d.ts",
    "lib.es2018.intl.d.ts",
    "lib.es2018.promise.d.ts",
    "lib.es2018.regexp.d.ts",
    "lib.es2019.array.d.ts",
    "lib.es2019.object.d.ts",
    "lib.es2019.string.d.ts",
    "lib.es2019.symbol.d.ts",
    "lib.es2019.intl.d.ts",
    "lib.es2020.bigint.d.ts",
    "lib.es2020.date.d.ts",
    "lib.es2020.promise.d.ts",
    "lib.es2020.sharedmemory.d.ts",
    "lib.es2020.string.d.ts",
    "lib.es2020.symbol.wellknown.d.ts",
    "lib.es2020.intl.d.ts",
    "lib.es2020.number.d.ts",
    "lib.es2021.promise.d.ts",
    "lib.es2021.string.d.ts",
    "lib.es2021.weakref.d.ts",
    "lib.es2021.intl.d.ts",
    "lib.es2022.array.d.ts",
    "lib.es2022.error.d.ts",
    "lib.es2022.intl.d.ts",
    "lib.es2022.object.d.ts",
    "lib.es2022.string.d.ts",
    "lib.es2022.regexp.d.ts",
    "lib.es2023.array.d.ts",
    "lib.es2023.collection.d.ts",
    "lib.es2023.intl.d.ts",
    "lib.es2024.arraybuffer.d.ts",
    "lib.es2024.collection.d.ts",
    "lib.es2024.object.d.ts",
    "lib.es2024.promise.d.ts",
    "lib.es2024.regexp.d.ts",
    "lib.es2024.sharedmemory.d.ts",
    "lib.es2024.string.d.ts",
    "lib.es2025.collection.d.ts",
    "lib.es2025.float16.d.ts",
    "lib.es2025.intl.d.ts",
    "lib.es2025.iterator.d.ts",
    "lib.es2025.promise.d.ts",
    "lib.es2025.regexp.d.ts",
    "lib.es2026.array.d.ts",
    "lib.es2026.collection.d.ts",
    "lib.es2026.error.d.ts",
    "lib.es2026.iterator.d.ts",
    "lib.es2026.json.d.ts",
    "lib.es2026.math.d.ts",
    "lib.es2026.typedarrays.d.ts",
    "lib.esnext.promise.d.ts",
    "lib.esnext.date.d.ts",
    "lib.esnext.decorators.d.ts",
    "lib.esnext.disposable.d.ts",
    "lib.esnext.intl.d.ts",
    "lib.esnext.modulesource.d.ts",
    "lib.esnext.sharedmemory.d.ts",
    "lib.esnext.temporal.d.ts",
    "lib.decorators.d.ts",
    "lib.decorators.legacy.d.ts",
    "lib.esnext.full.d.ts",
];

/// Go: `type testFile struct`.
struct TestFile {
    file_name: &'static str,
    contents: &'static str,
}

/// Go: `type programTest struct` (`target` is the tsconfig's "target" value —
/// Go's corpus only uses `core.ScriptTargetESNext`).
struct ProgramTestCase {
    test_name: &'static str,
    files: &'static [TestFile],
    expected_files: Vec<String>,
}

/// Go: `var programTestCases = []programTest{...}` (the three ordering
/// corpora, verbatim).
static BASIC_FILE_ORDERING_FILES: &[TestFile] = &[
    TestFile {
        file_name: "c:/dev/src/index.ts",
        contents: "/// <reference path='c:/dev/src2/a/5.ts' />\n/// <reference path='c:/dev/src2/a/10.ts' />",
    },
    TestFile {
        file_name: "c:/dev/src2/a/5.ts",
        contents: "/// <reference path='4.ts' />",
    },
    TestFile {
        file_name: "c:/dev/src2/a/4.ts",
        contents: "/// <reference path='b/3.ts' />",
    },
    TestFile {
        file_name: "c:/dev/src2/a/b/3.ts",
        contents: "/// <reference path='2.ts' />",
    },
    TestFile {
        file_name: "c:/dev/src2/a/b/2.ts",
        contents: "/// <reference path='c/1.ts' />",
    },
    TestFile {
        file_name: "c:/dev/src2/a/b/c/1.ts",
        contents: "console.log('hello');",
    },
    TestFile {
        file_name: "c:/dev/src2/a/10.ts",
        contents: "/// <reference path='b/c/d/9.ts' />",
    },
    TestFile {
        file_name: "c:/dev/src2/a/b/c/d/9.ts",
        contents: "/// <reference path='e/8.ts' />",
    },
    TestFile {
        file_name: "c:/dev/src2/a/b/c/d/e/8.ts",
        contents: "/// <reference path='7.ts' />",
    },
    TestFile {
        file_name: "c:/dev/src2/a/b/c/d/e/7.ts",
        contents: "/// <reference path='f/6.ts' />",
    },
    TestFile {
        file_name: "c:/dev/src2/a/b/c/d/e/f/6.ts",
        contents: "console.log('world!');",
    },
];

static FILE_ORDERING_IMPORTS_FILES: &[TestFile] = &[
    TestFile {
        file_name: "c:/dev/src/index.ts",
        contents: "import * as five from '../src2/a/5.ts';\nimport * as ten from '../src2/a/10.ts';",
    },
    TestFile {
        file_name: "c:/dev/src2/a/5.ts",
        contents: "import * as four from './4.ts';",
    },
    TestFile {
        file_name: "c:/dev/src2/a/4.ts",
        contents: "import * as three from './b/3.ts';",
    },
    TestFile {
        file_name: "c:/dev/src2/a/b/3.ts",
        contents: "import * as two from './2.ts';",
    },
    TestFile {
        file_name: "c:/dev/src2/a/b/2.ts",
        contents: "import * as one from './c/1.ts';",
    },
    TestFile {
        file_name: "c:/dev/src2/a/b/c/1.ts",
        contents: "console.log('hello');",
    },
    TestFile {
        file_name: "c:/dev/src2/a/10.ts",
        contents: "import * as nine from './b/c/d/9.ts';",
    },
    TestFile {
        file_name: "c:/dev/src2/a/b/c/d/9.ts",
        contents: "import * as eight from './e/8.ts';",
    },
    TestFile {
        file_name: "c:/dev/src2/a/b/c/d/e/8.ts",
        contents: "import * as seven from './7.ts';",
    },
    TestFile {
        file_name: "c:/dev/src2/a/b/c/d/e/7.ts",
        contents: "import * as six from './f/6.ts';",
    },
    TestFile {
        file_name: "c:/dev/src2/a/b/c/d/e/f/6.ts",
        contents: "console.log('world!');",
    },
];

static FILE_ORDERING_CYCLES_FILES: &[TestFile] = &[
    TestFile {
        file_name: "c:/dev/src/index.ts",
        contents: "import * as five from '../src2/a/5.ts';\nimport * as ten from '../src2/a/10.ts';",
    },
    TestFile {
        file_name: "c:/dev/src2/a/5.ts",
        contents: "import * as four from './4.ts';",
    },
    TestFile {
        file_name: "c:/dev/src2/a/4.ts",
        contents: "import * as three from './b/3.ts';",
    },
    TestFile {
        file_name: "c:/dev/src2/a/b/3.ts",
        contents: "import * as two from './2.ts';\nimport * as cycle from 'c:/dev/src/index.ts'; ",
    },
    TestFile {
        file_name: "c:/dev/src2/a/b/2.ts",
        contents: "import * as one from './c/1.ts';",
    },
    TestFile {
        file_name: "c:/dev/src2/a/b/c/1.ts",
        contents: "console.log('hello');",
    },
    TestFile {
        file_name: "c:/dev/src2/a/10.ts",
        contents: "import * as nine from './b/c/d/9.ts';",
    },
    TestFile {
        file_name: "c:/dev/src2/a/b/c/d/9.ts",
        contents: "import * as eight from './e/8.ts';\nimport * as cycle from 'c:/dev/src/index.ts';",
    },
    TestFile {
        file_name: "c:/dev/src2/a/b/c/d/e/8.ts",
        contents: "import * as seven from './7.ts';",
    },
    TestFile {
        file_name: "c:/dev/src2/a/b/c/d/e/7.ts",
        contents: "import * as six from './f/6.ts';",
    },
    TestFile {
        file_name: "c:/dev/src2/a/b/c/d/e/f/6.ts",
        contents: "console.log('world!');",
    },
];

fn program_test_cases() -> Vec<ProgramTestCase> {
    let expected_program_files = [
        "c:/dev/src2/a/b/c/1.ts",
        "c:/dev/src2/a/b/2.ts",
        "c:/dev/src2/a/b/3.ts",
        "c:/dev/src2/a/4.ts",
        "c:/dev/src2/a/5.ts",
        "c:/dev/src2/a/b/c/d/e/f/6.ts",
        "c:/dev/src2/a/b/c/d/e/7.ts",
        "c:/dev/src2/a/b/c/d/e/8.ts",
        "c:/dev/src2/a/b/c/d/9.ts",
        "c:/dev/src2/a/10.ts",
        "c:/dev/src/index.ts",
    ];
    let mut expected_files: Vec<String> = ESNEXT_LIBS.iter().map(|lib| lib.to_string()).collect();
    expected_files.extend(expected_program_files.iter().map(|file| file.to_string()));
    vec![
        ProgramTestCase {
            test_name: "BasicFileOrdering",
            files: BASIC_FILE_ORDERING_FILES,
            expected_files: expected_files.clone(),
        },
        ProgramTestCase {
            test_name: "FileOrderingImports",
            files: FILE_ORDERING_IMPORTS_FILES,
            expected_files: expected_files.clone(),
        },
        ProgramTestCase {
            test_name: "FileOrderingCycles",
            files: FILE_ORDERING_CYCLES_FILES,
            expected_files,
        },
    ]
}

/// The always-miss extended-config cache (Go's nil cache).
fn nil_extended_config_cache() -> ExtendedConfigCache {
    Arc::new(|_: &str| -> Option<CompilerOptionsValue> { None })
}

/// Builds the program over the corpus files (the shared body of Go's
/// `TestProgram`/`BenchmarkNewProgram` subtests): the bundled lib overlay,
/// the corpus files written into the map fs, and the parsed tsconfig whose
/// files list is the corpus root.
fn build_corpus_program(test: &ProgramTestCase) -> Program {
    let lib_prefix = format!("{}/", tsc_bundled::lib_path().as_string());
    let mut files: Vec<(String, String)> = vec![(
        "c:/dev/tsconfig.json".to_string(),
        // Go: core.CompilerOptions{Target: testCase.target} — the corpus only
        // uses target esnext.
        r#"{"compilerOptions":{"target":"esnext"},"files":["src/index.ts"]}"#.to_string(),
    )];
    for test_file in test.files {
        files.push((test_file.file_name.to_string(), test_file.contents.to_string()));
    }
    let fs = tsc_bundled::wrap_fs(vfstest::from_map(files, CaseSensitivity::CaseInsensitive));

    let config = get_parsed_command_line_of_config_file_path(
        &RootedFilePath::from("c:/dev/tsconfig.json"),
        fs.as_ref(),
        None,
    );
    assert!(
        config.errors.is_empty(),
        "config parse errors: {}",
        config.errors.len()
    );

    let host = new_compiler_host(
        Arc::clone(&fs),
        tsc_bundled::lib_path(),
        nil_extended_config_cache(),
        None,
    );
    let program = new_program(ProgramOptions {
        config: ProgramConfig {
            config: Arc::new(config),
            ..Default::default()
        },
        hosts: ProgramHosts { host: Some(host) },
        factories: ProgramFactories::default(),
    });

    // Go: the expected list is trimmed of the bundled lib prefix.
    let actual_files: Vec<String> = program
        .get_source_files()
        .iter()
        .map(|file| {
            // Go: strings.TrimPrefix(fileName, libPrefix) — strip_prefix (the
            // trim_prefix surface is unstable on this toolchain).
            file.file_name()
                .as_string()
                .strip_prefix(lib_prefix.as_str())
                .unwrap_or_else(|| file.file_name().as_string())
                .to_string()
        })
        .collect();
    assert_eq!(actual_files, test.expected_files, "{}", test.test_name);
    program
}

/// Go: `func TestProgram(t *testing.T)`.
///
/// PORT: Go skips when `!bundled.Embedded`; the port always embeds (the
/// noembed build is not ported), so the skip guard is dead.
#[test]
fn test_program() {
    for test in program_test_cases() {
        build_corpus_program(&test);
    }
}

/// Go: `func TestImportSourceProgram(t *testing.T)`.
///
/// PORT: only the initial construction is ported — the rest of Go's test
/// drives `UpdateProgram` (not ported; see the file header). Go's
/// `collectPackageNames().unresolved` assertion is skipped with it (the
/// lazy package-name cache is part of the same surface).
#[test]
fn test_import_source_program() {
    // The first/third tuple elements (the subtest name and the evaluation
    // form the UpdateProgram rounds swap in — not ported, see the file
    // header) stay unused here.
    for (_name, source, _evaluation) in [
        ("static", r#"import source a from "./a.js";"#, r#"import { a as value } from "./a.js";"#),
        ("dynamic", r#"import.source("./a.js");"#, r#"import("./a.js");"#),
    ] {
        let content = format!(
            r#"{}import source b from "missing"; import.source("other");"#,
            source
        );
        let files: Vec<(String, String)> = vec![
            (
                "/src/tsconfig.json".to_string(),
                r#"{"compilerOptions":{"module":"esnext","noLib":true},"files":["index.ts"]}"#
                    .to_string(),
            ),
            ("/src/index.ts".to_string(), content),
            ("/src/a.ts".to_string(), "export const a = 1;".to_string()),
        ];
        let fs = vfstest::from_map(files, CaseSensitivity::CaseSensitive);
        let host = new_compiler_host(
            Arc::clone(&fs),
            rooted_directory_path_from_normalized("/"),
            nil_extended_config_cache(),
            None,
        );
        let config = get_parsed_command_line_of_config_file_path(
            &RootedFilePath::from("/src/tsconfig.json"),
            fs.as_ref(),
            None,
        );
        assert!(
            config.errors.is_empty(),
            "config parse errors: {}",
            config.errors.len()
        );
        let program = new_program(ProgramOptions {
            config: ProgramConfig {
                config: Arc::new(config),
                ..Default::default()
            },
            hosts: ProgramHosts { host: Some(host) },
            factories: ProgramFactories::default(),
        });
        let file = program
            .get_source_file(&RootedFilePath::from("/src/index.ts"))
            .expect("file");
        assert!(program.get_source_file(&RootedFilePath::from("/src/a.ts")).is_none());
        let path = CaseSensitivity::CaseSensitive.path_key(&RootedPath::from(
            file.file_name().as_string(),
        ));
        assert_eq!(
            program.get_resolved_modules().get(&path).map_or(0, |m| m.len()),
            0
        );
        assert_eq!(program.get_unresolved_imports().len(), 0);
    }
}

/// Go: `func TestIncludeProcessorDiagnosticsWithMissingFileCasing(t *testing.T)`.
///
/// PORT: Go skips when `!bundled.Embedded` (dead here) and builds the config
/// with `NewParsedCommandLine`; the port parses a written tsconfig (see the
/// file header). `GetProgramDiagnostics` must not panic — a test panic fails
/// the test, exactly the Go recover-based assertion.
#[test]
fn test_include_processor_diagnostics_with_missing_file_casing() {
    // Use case-sensitive file names so that /src/MyFile.ts and /src/myFile.ts
    // have different canonical paths but the same lower-case path, triggering
    // file casing diagnostics in the include processor.
    let files: Vec<(String, String)> = vec![
        (
            "/src/tsconfig.json".to_string(),
            // Go: core.CompilerOptions{SkipDefaultLibCheck: core.TSTrue} —
            // and both casings listed as root files: the first one
            // (/src/MyFile.ts) does not exist on the case-sensitive fs.
            r#"{"compilerOptions":{"skipDefaultLibCheck":true},"files":["MyFile.ts","myFile.ts"]}"#
                .to_string(),
        ),
        ("/src/myFile.ts".to_string(), "export const y = 2;".to_string()),
    ];
    let fs = tsc_bundled::wrap_fs(vfstest::from_map(files, CaseSensitivity::CaseSensitive));

    let config = get_parsed_command_line_of_config_file_path(
        &RootedFilePath::from("/src/tsconfig.json"),
        fs.as_ref(),
        None,
    );
    let host = new_compiler_host(
        Arc::clone(&fs),
        tsc_bundled::lib_path(),
        nil_extended_config_cache(),
        None,
    );
    let program = new_program(ProgramOptions {
        config: ProgramConfig {
            config: Arc::new(config),
            ..Default::default()
        },
        hosts: ProgramHosts { host: Some(host) },
        factories: ProgramFactories::default(),
    });

    // GetProgramDiagnostics triggers getDiagnostics which processes all
    // include processor diagnostics including the casing diagnostic whose
    // file path points to the missing /src/MyFile.ts.
    let _ = program.get_program_diagnostics();
}

/// Go: `func TestIncludeReasonDiagnosticsAreProgramLocal(t *testing.T)`.
///
/// PORT: Go asserts *ast.Diagnostic pointer identity — the port returns
/// values from a per-program cache, so the identity assertion becomes the
/// cache-locality check (each program caches exactly its own diagnostic;
/// repeated computation in one program is the cached value), with the
/// value determinism asserted by the diagnostic signature.
#[test]
fn test_include_reason_diagnostics_are_program_local() {
    let opts = ProgramConfig::default();
    // Go: &Program{opts: opts} literals — the port's Program has private
    // cached state, so the test shape is Default + field assignment.
    let mut old_program = Program::default();
    old_program.opts = opts.clone();
    let mut new_program = Program::default();
    new_program.opts = opts;
    let reason = Arc::new(FileIncludeReason {
        kind: FileIncludeKind::RootFile,
        index: 0,
        is_default_lib: false,
        referenced_file: None,
        automatic_type_directive: None,
        canonical_source_file: PathKey::default(),
    });
    for relative in [false, true] {
        let relative_to = RootedDirectoryPath::default();
        let old_diagnostic = reason.to_diagnostic(&old_program, relative, relative_to.clone());
        let new_diagnostic = reason.to_diagnostic(&new_program, relative, relative_to);
        assert_eq!(
            diagnostic_signature(&reason.to_diagnostic(
                &old_program,
                relative,
                Default::default()
            )),
            diagnostic_signature(&old_diagnostic)
        );
        assert_eq!(
            diagnostic_signature(&reason.to_diagnostic(
                &new_program,
                relative,
                Default::default()
            )),
            diagnostic_signature(&new_diagnostic)
        );
    }
    // Program-local: each program's cache holds exactly its own diagnostics
    // (one per (relative, relativeTo) key — two loop rounds), computed
    // independently of the other program (Go: distinct pointers from the
    // distinct per-program caches).
    assert_eq!(
        old_program
            .include_processor
            .reason_diagnostics
            .lock()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        new_program
            .include_processor
            .reason_diagnostics
            .lock()
            .unwrap()
            .len(),
        2
    );
}

/// The diagnostic comparison signature (code + category + file name — the
/// identity Go compares by pointer).
fn diagnostic_signature(
    diagnostic: &tsc_compiler::FileDiagnostic,
) -> (Option<String>, i32, tsc_diagnostics::Category) {
    (
        diagnostic
            .file_name
            .as_ref()
            .map(|file_name| file_name.as_string().to_string()),
        diagnostic.diagnostic.code(),
        diagnostic.diagnostic.category(),
    )
}

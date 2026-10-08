// Ported from tsc/internal/outputpaths/outputpaths_test.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// The Go file's remaining cases (TestGetBuildInfoFileNameAcrossRoots tail,
// TestGetOutputFileNameAcrossRoots, TestOutputFileNamesPreserveEmptyAndDotStems)
// are ported below in full; assertions are the Go ones, one-to-one.

use tsc_core::options_generated::CompilerOptions;
use tsc_core::tristate::Tristate;
use tsc_outputpaths::get_build_info_file_name;
use tsc_outputpaths::get_output_declaration_file_name_worker;
use tsc_outputpaths::get_output_js_file_name_worker;
use tsc_outputpaths::get_source_file_name_in_new_dir;
use tsc_tspath::{CaseSensitivity, RootedDirectoryPath, RootedFilePath};

struct OutputPathsHost {
    common_source_directory: RootedDirectoryPath,
    content_mapper_extensions: Vec<String>,
}

impl tsc_outputpaths::OutputPathsHost for OutputPathsHost {
    fn common_source_directory(&self) -> RootedDirectoryPath {
        self.common_source_directory.clone()
    }
    fn content_mapper_extensions(&self) -> &[String] {
        &self.content_mapper_extensions
    }
    fn case_sensitivity(&self) -> CaseSensitivity {
        CaseSensitivity::CaseInsensitive
    }
}

fn file(path: &str) -> RootedFilePath {
    // Go: tspath.RootedFilePath("...") — direct, unvalidated conversion.
    RootedFilePath::from(path)
}

fn directory(path: &str) -> RootedDirectoryPath {
    RootedDirectoryPath::from(path)
}

#[test]
fn get_source_file_name_in_new_dir_source_matches_common_directory() {
    let actual = get_source_file_name_in_new_dir(
        &file("/project/src"),
        &directory("/project/out"),
        &directory("/project/src"),
        CaseSensitivity::CaseSensitive,
    );
    assert_eq!(actual, file("/project/src"));
}

#[test]
fn get_source_file_name_in_new_dir_canonicalization_shrinks_common_directory() {
    // Each Kelvin sign '\u{212A}' case-folds to the single-byte 'k', so the raw
    // (non-canonicalized) commonSourceDirectory is longer, in bytes, than the source
    // file path it's a case-insensitive prefix of, even though the file path itself
    // is longer overall once its own (already-lowercase) suffix is included.
    // Slicing sourceFilePath by len(commonSourceDirectory) bytes would still panic
    // here ([14:11]); this must clamp per-rune instead, like the reference
    // implementation's substring does.
    let actual = get_source_file_name_in_new_dir(
        &file("/kkkk/a.ts"),
        &directory("/out"),
        &directory("/\u{212A}\u{212A}\u{212A}\u{212A}"),
        CaseSensitivity::CaseInsensitive,
    );
    assert_eq!(actual, file("/out/a.ts"));
}

#[test]
fn get_build_info_file_name_across_roots() {
    struct Case {
        name: &'static str,
        root_dir: &'static str,
        out_dir: &'static str,
        config_file_path: &'static str,
        expected: &'static str,
    }
    let tests = [
        Case {
            name: "same root",
            root_dir: "c:/src",
            out_dir: "c:/out",
            config_file_path: "c:/src/project/tsconfig.json",
            expected: "c:/out/project/tsconfig.tsbuildinfo",
        },
        Case {
            name: "empty stem",
            root_dir: "c:/src",
            out_dir: "c:/out",
            config_file_path: "c:/src/project/.json",
            expected: "c:/out/project/.tsbuildinfo",
        },
        Case {
            name: "dot stem",
            root_dir: "c:/src",
            out_dir: "c:/out",
            config_file_path: "c:/src/project/..json",
            expected: "c:/out/project/..tsbuildinfo",
        },
        Case {
            name: "empty filename stem without rootDir",
            root_dir: "",
            out_dir: "c:/out",
            config_file_path: "c:/src/project/.json",
            expected: "c:/out/.tsbuildinfo",
        },
        Case {
            name: "parent-dot filename stem without rootDir",
            root_dir: "",
            out_dir: "c:/out",
            config_file_path: "c:/src/project/...json",
            expected: "c:/out/...tsbuildinfo",
        },
        Case {
            name: "empty filename stem without outDir",
            root_dir: "",
            out_dir: "",
            config_file_path: "c:/src/project/.json",
            expected: "c:/src/project/.tsbuildinfo",
        },
        Case {
            name: "dot filename stem without outDir",
            root_dir: "",
            out_dir: "",
            config_file_path: "c:/src/project/..json",
            expected: "c:/src/project/..tsbuildinfo",
        },
        Case {
            name: "different drive",
            root_dir: "c:/src",
            out_dir: "c:/out",
            config_file_path: "d:/project/tsconfig.json",
            expected: "d:/project/tsconfig.tsbuildinfo",
        },
        Case {
            name: "different UNC authority",
            root_dir: "//server-a/src",
            out_dir: "//server-a/out",
            config_file_path: "//server-b/project/tsconfig.json",
            expected: "//server-b/project/tsconfig.tsbuildinfo",
        },
        Case {
            name: "different URL authority",
            root_dir: "file://server-a/src",
            out_dir: "file://server-a/out",
            config_file_path: "file://server-b/project/tsconfig.json",
            expected: "file://server-b/project/tsconfig.tsbuildinfo",
        },
    ];
    for test in tests {
        let options = CompilerOptions {
            incremental: Tristate::TSTrue,
            root_dir: directory(test.root_dir),
            out_dir: directory(test.out_dir),
            config_file_path: file(test.config_file_path),
            ..CompilerOptions::default()
        };
        let actual = get_build_info_file_name(&options, CaseSensitivity::CaseInsensitive);
        assert_eq!(actual, file(test.expected), "case {}", test.name);
    }
}

#[test]
fn get_output_file_name_across_roots() {
    let input = file("d:/shared.ts");
    let options = CompilerOptions {
        out_dir: directory("c:/dist"),
        ..CompilerOptions::default()
    };
    let host = OutputPathsHost {
        common_source_directory: directory("c:/src"),
        content_mapper_extensions: Vec::new(),
    };
    assert_eq!(
        get_output_js_file_name_worker(&input, &options, &host),
        file("d:/shared.js")
    );
    assert_eq!(
        get_output_declaration_file_name_worker(&input, &options, &host),
        file("d:/shared.d.ts")
    );
}

#[test]
fn output_file_names_preserve_empty_and_dot_stems() {
    let host = OutputPathsHost {
        common_source_directory: directory("/project/src"),
        content_mapper_extensions: vec![".css".to_string()],
    };
    let options = CompilerOptions {
        out_dir: directory("/project/out"),
        ..CompilerOptions::default()
    };
    let tests: &[(&str, &str, &str)] = &[
        ("/project/src/.ts", "/project/out/.js", "/project/out/.d.ts"),
        ("/project/src/..ts", "/project/out/..js", "/project/out/..d.ts"),
        ("/project/src/...ts", "/project/out/...js", "/project/out/...d.ts"),
        ("/project/src/.mts", "/project/out/.mjs", "/project/out/.d.mts"),
        ("/project/src/.d.ts", "/project/out/.js", "/project/out/.d.ts"),
        ("/project/src/.css", "/project/out/.css", "/project/out/.d.css.ts"),
    ];
    for (input, js, declaration) in tests {
        assert_eq!(
            get_output_js_file_name_worker(&file(input), &options, &host),
            file(js),
            "input {input}"
        );
        assert_eq!(
            get_output_declaration_file_name_worker(&file(input), &options, &host),
            file(declaration),
            "input {input}"
        );
    }
}

// Phase 0 micro benches mirroring Go's tsc/internal benchmarks (SPEC.md §15.1).
//
// Every sub-bench reproduces the Go benchmark's exact inputs, iteration body,
// and sub-bench name so bench-micro.sh can join the two outputs by name.
// Go replaces spaces with underscores in bench names; do the same.
//
// Ops:
//   tspath/CombinePaths                     (tsc/internal/tspath/path_test.go)
//   tspath/GetNormalizedAbsolutePath        (path_test.go)
//   tspath/ToFileNameLowerCase              (path_test.go)
//   tspath/HasRelativePathSegment           (path_test.go)
//   tspath/PathIsRelative                   (path_test.go)
//   tspath/RootedDirectoryPathResolveFile   (typed_paths_test.go)
//   tspath/RootedFilePathToPathKey          (typed_paths_test.go)
//   jsnum/ToInt32                          (tsc/internal/jsnum/jsnum_test.go)
//   jsnum/Exponentiate                     (jsnum_test.go)

use std::hint::black_box;
use std::time::Instant;

use tsc_jsnum::{inf, nan, Number};
use tsc_tspath::{
    combine_paths, get_normalized_absolute_path, has_relative_path_segment, path_is_relative,
    rooted_directory_path_from_normalized, rooted_file_path_from_normalized,
    to_file_name_lower_case, to_rooted_directory_path, to_rooted_file_path, to_rooted_path,
    CaseSensitivity, RootedDirectoryPath,
};

use crate::Bench;

pub fn run(args: &[String]) {
    let mut op = String::new();
    let mut samples: u32 = 10;
    let mut iters: u64 = 0; // 0 = auto-calibrate to ~50ms per timed sample
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--op" if i + 1 < args.len() => {
                op = args[i + 1].clone();
                i += 2;
            }
            "--samples" if i + 1 < args.len() => {
                samples = args[i + 1].parse().unwrap_or(10);
                i += 2;
            }
            "--iters" if i + 1 < args.len() => {
                iters = args[i + 1].parse().unwrap_or(0);
                i += 2;
            }
            other => {
                eprintln!("micro: unknown arg {other:?}");
                std::process::exit(2);
            }
        }
    }
    if op.is_empty() {
        eprintln!("micro: --op required");
        std::process::exit(2);
    }
    match op.as_str() {
        "tspath/CombinePaths" => combine_paths_op(samples, iters),
        "tspath/GetNormalizedAbsolutePath" => get_normalized_absolute_path_op(samples, iters),
        "tspath/ToFileNameLowerCase" => to_file_name_lower_case_op(samples, iters),
        "tspath/HasRelativePathSegment" => has_relative_path_segment_op(samples, iters),
        "tspath/PathIsRelative" => path_is_relative_op(samples, iters),
        "tspath/RootedDirectoryPathResolveFile" => rooted_directory_path_resolve_file_op(samples, iters),
        "tspath/RootedFilePathToPathKey" => rooted_file_path_to_path_key_op(samples, iters),
        "jsnum/ToInt32" => to_int32_op(samples, iters),
        "jsnum/Exponentiate" => exponentiate_op(samples, iters),
        other => {
            eprintln!("micro: unknown op {other}");
            std::process::exit(2);
        }
    }
}

/// Runs one sub-bench: calibrates iterations when `iters == 0`, then times
/// `samples` fixed-iteration loops and reports the median ns/op.
fn bench<F: FnMut() -> u64>(name: &str, samples: u32, iters: u64, mut f: F) {
    let iters = if iters == 0 { calibrate(&mut f) } else { iters };
    Bench {
        op: name.to_string(),
        iters,
        samples,
    }
    .run(f);
}

/// Calibrates iterations so one timed sample runs ~50 ms.
fn calibrate(f: &mut dyn FnMut() -> u64) -> u64 {
    const TARGET_NS: f64 = 50_000_000.0;
    let start = Instant::now();
    let mut n: u64 = 0;
    let mut acc: u64 = 0;
    while start.elapsed().as_nanos() < 5_000_000 && n < 1_000_000 {
        acc = acc.wrapping_add(f());
        n += 1;
    }
    black_box(acc);
    let per = start.elapsed().as_nanos() as f64 / n.max(1) as f64;
    (TARGET_NS / per).clamp(1.0, 100_000_000.0) as u64
}

/// Mirrors Go's shortenName (path_test.go): byte-prefix at 20 + "...etc".
/// Go slices bytes without UTF-8 checks; all cut points here land on ASCII,
/// so `from_utf8_lossy` is a safe stand-in.
fn shorten_name(name: &str) -> String {
    if name.len() > 20 {
        let mut s = String::from_utf8_lossy(&name.as_bytes()[..20]).into_owned();
        s.push_str("...etc");
        s
    } else {
        name.to_string()
    }
}

/// Mirrors Go's fmt.Sprintf("%v", float64) — strconv 'g' with shortest
/// precision: exponent form iff exp < -4 || exp >= 6 (eprec=6 for shortest).
/// Used only for ToInt32 sub-bench names.
fn go_float_display(f: f64) -> String {
    if f.is_nan() {
        return "NaN".to_string();
    }
    if f.is_infinite() {
        return if f.is_sign_positive() { "+Inf".to_string() } else { "-Inf".to_string() };
    }
    let sci = format!("{f:e}"); // shortest: [-]d[.ddd]e<exp>
    let Some((mantissa, exp_str)) = sci.split_once('e') else {
        return sci;
    };
    let exp: i32 = exp_str.parse().unwrap_or(0);
    if exp < -4 || exp >= 6 {
        // Go pads the exponent to two digits with an explicit sign.
        let sign = if exp < 0 { '-' } else { '+' };
        format!("{mantissa}e{sign}{:02}", exp.abs())
    } else {
        format!("{f}")
    }
}

// tspath/CombinePaths — mirrors BenchmarkCombinePaths (path_test.go:260).
fn combine_paths_op(samples: u32, iters: u64) {
    let tests: [&[&str]; 6] = [
        &["path", "to", "file.ext"],
        &["path", "dir", "..", "to", "file.ext"],
        &["/path", "to", "file.ext"],
        &["/path", "/to", "file.ext"],
        &["c:/path", "to", "file.ext"],
        &["file:///path", "to", "file.ext"],
    ];
    for test in tests {
        let name = shorten_name(&test.join("/"));
        let first = test[0];
        let rest = &test[1..];
        bench(&format!("tspath/CombinePaths/{name}"), samples, iters, || {
            black_box(combine_paths(first, rest)).len() as u64
        });
    }
}

// tspath/GetNormalizedAbsolutePath — mirrors BenchmarkGetNormalizedAbsolutePath
// (path_test.go:494), racing only the "GetNormalizedAbsolutePath" fn (not the
// "(old)" variant). One timed iteration = the whole group's test list, with
// the directory helper called inside the loop, exactly as Go does.
fn get_normalized_absolute_path_op(samples: u32, iters: u64) {
    let groups: Vec<(&str, Vec<(&str, &str)>)> = vec![
        (
            "non-normalized inputs",
            vec![
                ("/.", ""),
                ("/./", ""),
                ("/../", ""),
                ("/a/", ""),
                ("/a/.", ""),
                ("/a/foo.", ""),
                ("/a/./", ""),
                ("/a/./b", ""),
                ("/a/./b/", ""),
                ("/a/..", ""),
                ("/a/../", ""),
                ("/a/../", ""),
                ("/a/../b", ""),
                ("/a/../b/", ""),
                ("/a/..", ""),
                ("/a/..", "/"),
                ("/a/..", "b/"),
                ("/a/..", "/b"),
                ("/a/.", "b"),
                ("/a/.", "."),
            ],
        ),
        (
            "normalized inputs",
            vec![
                ("/a/b", ""),
                ("/one/two/three", ""),
                ("/users/root/project/src/foo.ts", ""),
            ],
        ),
        (
            "normalized inputs (long)",
            vec![
                ("/a/b/c/d/e/f/g/h/i/j/k/l/m/n/o/p/q/r/s/t/u/v/w/x/y/z", ""),
                (
                    "/one/two/three/four/five/six/seven/eight/nine/ten/eleven/twelve/thirteen/fourteen/fifteen/sixteen/seventeen/eighteen/nineteen/twenty",
                    "",
                ),
                (
                    "/users/root/project/src/foo/bar/baz/qux/quux/corge/grault/garply/waldo/fred/plugh/xyzzy/thud",
                    "",
                ),
                (
                    "/lorem/ipsum/dolor/sit/amet/consectetur/adipiscing/elit/sed/do/eiusmod/tempor/incididunt/ut/labore/et/dolore/magna/aliqua/ut/enim/ad/minim/veniam",
                    "",
                ),
            ],
        ),
    ];
    let root = rooted_directory_path_from_normalized("/");
    for (group, tests) in groups {
        let name = format!(
            "tspath/GetNormalizedAbsolutePath/{}/GetNormalizedAbsolutePath",
            group.replace(' ', "_")
        );
        bench(&name, samples, iters, || {
            let mut sum: u64 = 0;
            for test in &tests {
                let dir = normalized_test_directory(test.1, &root);
                sum = sum.wrapping_add(
                    black_box(get_normalized_absolute_path(test.0, &dir)).len() as u64
                );
            }
            sum
        });
    }
}

/// Mirrors Go's normalizedTestDirectory helper (path_test.go:487).
fn normalized_test_directory(path: &str, root: &RootedDirectoryPath) -> RootedDirectoryPath {
    if path.is_empty() {
        RootedDirectoryPath::default()
    } else {
        to_rooted_directory_path(path, root)
    }
}

// tspath/ToFileNameLowerCase — mirrors BenchmarkToFileNameLowerCase (path_test.go:586).
fn to_file_name_lower_case_op(samples: u32, iters: u64) {
    let tests: Vec<String> = vec![
        "/path/to/file.ext".to_string(),
        "/PATH/TO/FILE.EXT".to_string(),
        "/path/to/FILE.EXT".to_string(),
        "/user/UserName/projects/Project/file.ts".to_string(),
        "/user/UserName/projects/project\u{00DF}/file.ts".to_string(),
        "/user/UserName/projects/\u{0130}project/file.ts".to_string(),
        "/user/UserName/projects/\u{0131}/file.ts".to_string(),
        "FoO/".repeat(100),
    ];
    for test in &tests {
        let name = shorten_name(test);
        let t: &str = test;
        bench(
            &format!("tspath/ToFileNameLowerCase/{name}"),
            samples,
            iters,
            || black_box(to_file_name_lower_case(t)).len() as u64,
        );
    }
}

// tspath/HasRelativePathSegment — mirrors BenchmarkHasRelativePathSegment
// (path_test.go:682). Go's table has 9 rows; only the 4 bench:true rows run.
fn has_relative_path_segment_op(samples: u32, iters: u64) {
    let tests: Vec<String> = vec![
        "foo/bar/baz".to_string(),
        "./some/path".to_string(),
        "/foo/./bar/../../.".to_string(),
        "foo/".repeat(100) + "..",
    ];
    for test in &tests {
        let name = shorten_name(test);
        let t: &str = test;
        bench(
            &format!("tspath/HasRelativePathSegment/{name}"),
            samples,
            iters,
            || black_box(has_relative_path_segment(t)) as u64,
        );
    }
}

// tspath/PathIsRelative — mirrors BenchmarkPathIsRelative (path_test.go:748):
// the bench:true rows plus the init()-appended backslash variants of each.
fn path_is_relative_op(samples: u32, iters: u64) {
    let tests: Vec<String> = vec![
        "./foo/bar".to_string(),
        "../foo/bar".to_string(),
        format!("../{}", "foo/".repeat(100)),
        ".\\foo\\bar".to_string(),
        "..\\foo\\bar".to_string(),
        format!("..\\{}", "foo\\".repeat(100)),
    ];
    for test in &tests {
        let name = shorten_name(test);
        let t: &str = test;
        bench(
            &format!("tspath/PathIsRelative/{name}"),
            samples,
            iters,
            || black_box(path_is_relative(t)) as u64,
        );
    }
}

// tspath/RootedDirectoryPathResolveFile — mirrors typed_paths_test.go:805.
fn rooted_directory_path_resolve_file_op(samples: u32, iters: u64) {
    let base = rooted_directory_path_from_normalized("/project/src");
    bench(
        "tspath/RootedDirectoryPathResolveFile/typed_fast_path",
        samples,
        iters,
        || black_box(base.resolve_file("node_modules")).as_string().len() as u64,
    );
    bench(
        "tspath/RootedDirectoryPathResolveFile/general_rooting",
        samples,
        iters,
        || {
            black_box(to_rooted_file_path("node_modules", &base))
                .as_string()
                .len() as u64
        },
    );
}

// tspath/RootedFilePathToPathKey — mirrors typed_paths_test.go:819.
fn rooted_file_path_to_path_key_op(samples: u32, iters: u64) {
    let file_name = "/home/user/project/src/some/deep/module/file.ts";
    let normalized = rooted_file_path_from_normalized(file_name);
    let current_directory = rooted_directory_path_from_normalized("/home/user/project");
    bench(
        "tspath/RootedFilePathToPathKey/RootAndPathKey",
        samples,
        iters,
        || {
            black_box(
                CaseSensitivity::CaseInsensitive
                    .path_key(&to_rooted_path(file_name, &current_directory)),
            )
            .as_string()
            .len() as u64
        },
    );
    bench(
        "tspath/RootedFilePathToPathKey/AlreadyRooted",
        samples,
        iters,
        || {
            black_box(
                CaseSensitivity::CaseInsensitive.path_key(&normalized.as_path()),
            )
            .as_string()
            .len() as u64
        },
    );
}

// jsnum/ToInt32 — mirrors BenchmarkToInt32 (jsnum_test.go:288): the bench:true
// rows of toInt32Tests, named fmt.Sprintf("%s (%v)", name, float64(input)).
fn to_int32_op(samples: u32, iters: u64) {
    let max_safe = tsc_jsnum::MAX_SAFE_INTEGER;
    let tests: Vec<(&str, Number)> = vec![
        ("0.0", Number(0.0)),
        ("NaN", nan()),
        ("+Inf", inf(1)),
        ("-Inf", inf(-1)),
        ("MaxInt32+1", Number(2_147_483_648.0)),
        ("MinInt32-1", Number(-2_147_483_649.0)),
        ("MAX_SAFE_INTEGER", max_safe),
        ("MAX_SAFE_INTEGER-1", Number(max_safe.0 - 1.0)),
        ("MAX_SAFE_INTEGER+1", Number(max_safe.0 + 1.0)),
        ("0xDEADBEEF", Number(0xDEAD_BEEF_u32 as f64)),
        ("TypeFlagsNarrowable", Number(536_624_127.0)),
    ];
    for (name, input) in tests {
        let sub = format!("{} ({})", name, go_float_display(input.0));
        bench(&format!("jsnum/ToInt32/{sub}"), samples, iters, || {
            black_box(input.to_int32()) as u64
        });
    }
}

// jsnum/Exponentiate — mirrors BenchmarkExponentiate (jsnum_test.go:719).
fn exponentiate_op(samples: u32, iters: u64) {
    let cases: Vec<(&str, Number, Number)> = vec![
        ("2**10_exact", Number(2.0), Number(10.0)),
        ("2**53_exact", Number(2.0), Number(53.0)),
        ("10**20_bigint", Number(10.0), Number(20.0)),
        ("10**308_bigint", Number(10.0), Number(308.0)),
        ("3**34_bigint", Number(3.0), Number(34.0)),
        ("0.5**-0.5_mathpow", Number(0.5), Number(-0.5)),
    ];
    for (name, base, exponent) in cases {
        bench(&format!("jsnum/Exponentiate/{name}"), samples, iters, || {
            black_box(base.exponentiate(exponent)).0.to_bits()
        });
    }
}

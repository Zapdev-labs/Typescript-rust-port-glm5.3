// Port of tsc/internal/testutil/harnessutil @
// ec47d33c23e464a17cdf2475632cba629bee8763 — the subset the M6 conformance
// runner (tsc-testrunner) drives at PARSE+BIND depth.
//
// File map (Go → Rust): harnessutil.go's TestFile / TestConfiguration /
// NamedTestConfiguration / HarnessOptions / EnumerateFiles /
// GetFileBasedTestConfigurations (+ splitOptionValues, the description
// builder, the variation worker) / GetConfigNameFromFileName.
//
// PORT notes:
//   - `CompileFiles`/`CompileFilesEx`/`newCompilationResult` (the whole
//     cached-host + tracer + output-recorder compilation plumbing) are
//     superseded at this depth by the runner's written-tsconfig program
//     construction (the M5 program-twin route — see tsc-testrunner's
//     compiler_runner.rs); the checker/emit consumers are not ported.
//   - `SkipUnsupportedCompilerOptions` gates CHECKER-stage behavior
//     (module kinds, resolutions, targets the ported checker does not
//     support). At PARSE+BIND depth no option gates the wired stages, so
//     the runner runs every case and the skip list is not ported (it
//     plugs in with the check stage — see the Stage enum).
//   - `splitOptionValues` is the simplified literal form: comma-split and
//     literal dedupe only. Go additionally expands `*` from the option's
//     enum map (tsoptions.CommandLineOption.EnumMap) and excludes with
//     `-`/`!`; the Rust tsoptions does not expose the enum maps, so BOTH
//     conformance drivers treat `*` as a literal value and apply `-`/`!`
//     as literal exclusions (6 conformance cases use `*`; PORT-noted in
//     rust/CONFORMANCE.md).
//   - Go iterates its option/settings MAPS in random order; every ported
//     map iteration is sorted (BTreeMap) so the drivers are deterministic.
//   - recorderfs.go / sourcemap_recorder.go (the output recorder for emit
//     baselines) are emit-scope and not ported.

use std::collections::BTreeMap;
use std::path::Path;

/// Go: `type TestFile struct { UnitName, Content string }`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestFile {
    pub unit_name: String,
    pub content: String,
}

/// Go: `type TestConfiguration = map[string]string` — the port pins the
/// iteration order (sorted keys) for determinism.
pub type TestConfiguration = BTreeMap<String, String>;

/// Go: `type NamedTestConfiguration struct { Name; Config }`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamedTestConfiguration {
    pub name: String,
    pub config: TestConfiguration,
}

/// Go: `type HarnessOptions struct` — the fields the ported depth reads
/// (currentDirectory/noImplicitReferences flow through the merged test
/// configuration; the emit/sourcemap/baselineFile fields are emit-scope).
#[derive(Clone, Debug, Default)]
pub struct HarnessOptions {
    pub case_sensitivity: Option<tsc_tspath::CaseSensitivity>,
    pub no_implicit_references: bool,
    pub current_directory: String,
    pub no_types_and_symbols: bool,
}

/// The harness's own option names (Go: `harnessCommandLineOptions` +
/// `typescriptVersion`) — directives that configure the harness, never a
/// tsconfig `compilerOptions` entry.
pub const HARNESS_OPTIONS: &[&str] = &[
    "usecasesensitivefilenames",
    "baselinefile",
    "includebuiltfile",
    "filename",
    "libfiles",
    "noimplicitreferences",
    "currentdirectory",
    "symlink",
    "link",
    "notypesandsymbols",
    "fullemitpaths",
    "reportdiagnostics",
    "capturesuggestions",
    "typescriptversion",
    "runexternalcode",
];

/// Go: `EnumerateFiles(folder, testRegex, recursive)` — folder is the
/// cases tree under the test-data root; the port takes the absolute
/// `cases_root` directly (the runner's driver owns path resolution) and
/// the `\.tsx?$` file regex as a predicate over the rooted path string.
///
/// Go's walk is `os.ReadDir` (name-sorted) with inline pre-order recursion;
/// the port additionally sorts the final list by full path string so the
/// enumeration is a total order on both conformance drivers (Go's
/// `os.ReadDir` sorts per directory but interleaves directory recursion —
/// same entries, deterministic order — PORT-noted in CONFORMANCE.md).
pub fn enumerate_files<F>(cases_root: &Path, recursive: bool, is_match: F) -> Result<Vec<String>, String>
where
    F: Fn(&str) -> bool,
{
    let mut out = Vec::new();
    list_files_worker(cases_root, recursive, &is_match, &mut out)?;
    out.sort();
    Ok(out)
}

/// Go: `listFilesWorker` — reads `folder` (errors are fatal for the root,
/// propagated otherwise) and matches files against the spec.
fn list_files_worker<F>(folder: &Path, recursive: bool, is_match: &F, out: &mut Vec<String>) -> Result<(), String>
where
    F: Fn(&str) -> bool,
{
    let entries = std::fs::read_dir(folder).map_err(|err| format!("reading {}: {err}", folder.display()))?;
    let mut items: Vec<(String, std::fs::DirEntry)> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| format!("reading {}: {err}", folder.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        items.push((name, entry));
    }
    // Go: os.ReadDir returns entries sorted by file name (byte order).
    items.sort_by(|a, b| a.0.cmp(&b.0));
    for (name, entry) in items {
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            if !recursive {
                continue;
            }
            list_files_worker(&folder.join(&name), recursive, is_match, out)?;
            continue;
        }
        let path = folder.join(&name);
        let path_str = path.to_string_lossy().into_owned();
        if is_match(&path_str) {
            out.push(path_str);
        }
    }
    Ok(())
}

/// The compiler-baseline file spec: Go's `compilerBaselineRegex =
/// regexp.MustCompile("\\.tsx?$")` matched against the path string.
pub fn is_compiler_baseline_file(path: &str) -> bool {
    path.ends_with(".ts") || path.ends_with(".tsx")
}

/// Go: `GetConfigNameFromFileName` — the tsconfig/jsconfig unit detector.
pub fn get_config_name_from_file_name(file_name: &str) -> String {
    let basename_lower = tsc_tspath::get_base_file_name(file_name).to_lowercase();
    if basename_lower == "tsconfig.json" || basename_lower == "jsconfig.json" {
        return basename_lower;
    }
    String::new()
}

/// Go: `getFileBasedTestConfigurationDescription` — the variation's name
/// (keys sorted, values lowercased, `k=v` joined with ","). The port's
/// BTreeMap keys are already sorted.
pub fn get_file_based_test_configuration_description(config: &TestConfiguration) -> String {
    config
        .iter()
        .map(|(key, value)| format!("{key}={}", value.to_lowercase()))
        .collect::<Vec<_>>()
        .join(",")
}

/// Go: `GetFileBasedTestConfigurations(t, settings, varyByOptions)` — the
/// per-file configuration expansion. `settings` are the raw `// @option:`
/// values (lowercase keys); `vary_by` is the compiler vary-by set
/// (tsc-testrunner::options_generated::COMPILER_VARY_BY).
pub fn get_file_based_test_configurations(
    settings: &BTreeMap<String, String>,
    vary_by: &dyn Fn(&str) -> bool,
) -> Vec<NamedTestConfiguration> {
    // Each element: the option name followed by its values (Go's
    // `optionEntries`, ordered by sorted option name for determinism).
    let mut option_entries: Vec<(String, Vec<String>)> = Vec::new();
    let mut non_varying_options: TestConfiguration = BTreeMap::new();
    for (option, value) in settings {
        if vary_by(option) {
            let entries = split_option_values(value, option);
            if entries.len() > 1 {
                option_entries.push((option.clone(), entries));
            } else if let Some(entry) = entries.first() {
                non_varying_options.insert(option.clone(), entry.clone());
            }
        } else {
            // Variation is not supported for the option.
            non_varying_options.insert(option.clone(), value.clone());
        }
    }

    let mut configurations: Vec<NamedTestConfiguration> = Vec::new();
    if !option_entries.is_empty() {
        let mut variations: Vec<TestConfiguration> = Vec::new();
        let mut state: TestConfiguration = BTreeMap::new();
        compute_variations_worker(&option_entries, 0, &mut state, &mut variations);
        for mut varying_config in variations {
            let description = get_file_based_test_configuration_description(&varying_config);
            for (key, value) in &non_varying_options {
                varying_config.insert(key.clone(), value.clone());
            }
            configurations.push(NamedTestConfiguration {
                name: description,
                config: varying_config,
            });
        }
    } else if !non_varying_options.is_empty() {
        configurations.push(NamedTestConfiguration {
            name: String::new(),
            config: non_varying_options,
        });
    }
    configurations
}

/// Go: `computeFileBasedTestConfigurationVariationsWorker` — the cartesian
/// product in option-entry order.
fn compute_variations_worker(
    option_entries: &[(String, Vec<String>)],
    index: usize,
    variation_state: &mut TestConfiguration,
    configurations: &mut Vec<TestConfiguration>,
) {
    let Some((option_key, entries)) = option_entries.get(index) else {
        configurations.push(variation_state.clone());
        return;
    };
    for entry in entries {
        variation_state.insert(option_key.clone(), entry.clone());
        compute_variations_worker(option_entries, index + 1, variation_state, configurations);
    }
}

/// Go: `splitOptionValues` — the simplified literal form (see the module
/// header PORT note): comma-split, trim, drop empties, `-`/`!` exclusions
/// against the includes, literal dedupe preserving first occurrence.
fn split_option_values(value: &str, option: &str) -> Vec<String> {
    if value.is_empty() {
        return Vec::new();
    }
    let mut star = false;
    let mut includes: Vec<String> = Vec::new();
    let mut excludes: Vec<String> = Vec::new();
    for s in value.split(',') {
        let s = s.trim();
        if s.is_empty() {
            continue;
        }
        if s == "*" {
            star = true;
        } else if let Some(excluded) = s.strip_prefix('-').or_else(|| s.strip_prefix('!')) {
            excludes.push(excluded.to_string());
        } else {
            includes.push(s.to_string());
        }
    }
    if includes.is_empty() && !star && excludes.is_empty() {
        return Vec::new();
    }
    // PORT: `*` stays a literal include (no enum-map expansion).
    if star {
        let mut with_star = vec!["*".to_string()];
        with_star.extend(includes);
        includes = with_star;
    }
    // literal dedupe, first occurrence wins
    let mut seen: Vec<String> = Vec::new();
    for include in includes {
        if !excludes.contains(&include) && !seen.contains(&include) {
            seen.push(include);
        }
    }
    if seen.is_empty() {
        // Go panics when the variations are empty; the harness surfaces it
        // as a fatal test error. The port reports it the same way.
        panic!("Variations in test option '@{option}' resulted in an empty set.");
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn no_options_yields_no_configurations() {
        let empty: BTreeMap<String, String> = BTreeMap::new();
        assert!(get_file_based_test_configurations(&empty, &|_| false).is_empty());
    }

    #[test]
    fn non_varying_options_fold_into_a_single_unnamed_configuration() {
        let vary_by = |name: &str| name == "target";
        let configs = get_file_based_test_configurations(
            &settings(&[("nolib", "true"), ("noemit", "true")]),
            &vary_by,
        );
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].name, "");
        assert_eq!(configs[0].config.get("nolib").map(String::as_str), Some("true"));
    }

    #[test]
    fn multiple_values_expand_into_named_configurations() {
        let vary_by = |name: &str| name == "target";
        let configs = get_file_based_test_configurations(&settings(&[("target", "esnext, es2015")]), &vary_by);
        assert_eq!(configs.len(), 2);
        // Go preserves the value order ("esnext, es2015" → esnext first);
        // only the option-ENTRY iteration is sorted (Go's map order is
        // random — the port's sorted-key determinism rule).
        assert_eq!(configs[0].name, "target=esnext");
        assert_eq!(configs[1].name, "target=es2015");
        // the non-varying options merge into every configuration
        let configs = get_file_based_test_configurations(
            &settings(&[("target", "esnext, es2015"), ("nolib", "true")]),
            &vary_by,
        );
        assert_eq!(configs.len(), 2);
        for config in &configs {
            assert_eq!(config.config.get("nolib").map(String::as_str), Some("true"));
        }
    }

    #[test]
    fn exclusions_drop_values() {
        assert_eq!(
            split_option_values("esnext, es2015, -es2015", "target"),
            vec!["esnext".to_string()]
        );
        assert_eq!(split_option_values("esnext, esnext", "target"), vec!["esnext".to_string()]);
    }

    #[test]
    #[should_panic(expected = "resulted in an empty set")]
    fn fully_excluded_values_panic_like_go() {
        let _ = split_option_values("esnext, -esnext", "target");
    }

    #[test]
    fn config_name_detection() {
        assert_eq!(get_config_name_from_file_name("/.src/tsconfig.json"), "tsconfig.json");
        assert_eq!(get_config_name_from_file_name("/.src/jsconfig.json"), "jsconfig.json");
        assert_eq!(get_config_name_from_file_name("/.src/foo.ts"), "");
    }

    #[test]
    fn description_is_sorted_keys_with_lowercased_values() {
        let mut config: TestConfiguration = BTreeMap::new();
        config.insert("Target".to_string(), "ESNext".to_string());
        config.insert("alwaysstrict".to_string(), "TRUE".to_string());
        assert_eq!(
            get_file_based_test_configuration_description(&config),
            "Target=esnext,alwaysstrict=true"
        );
    }
}

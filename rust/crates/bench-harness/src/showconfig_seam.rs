//! `tsc-bench showconfig` + `tsc-bench resolvereact` — the M4 parity twins
//! (tsconfig→TSConfig pipeline; module resolution).
//!
//! Go reference: rust/bench/go-driver/main.go (`runShowConfig`,
//! `runResolveReact`). Both drivers walk the corpus with dump's walk/skip
//! rules, drive the SAME crate pipelines the tsoptions/module crate tests
//! drive, and emit byte-identical NDJSON lines — `diff` is the parity
//! verdict.
//!
//!   tsc-bench showconfig --corpus <dir> | --tsconfig <path>
//!   tsc-bench resolvereact --corpus <dir>
//!
//! showconfig call shape (both sides, mirroring
//! rust/crates/tsoptions/tests/showconfig_test.rs's `run_showconfig`):
//!
//!   text → parse_config_file_text_to_json (order-preserving)
//!       → parse_json_config_file_content (extends resolution, osvfs host)
//!       → convert_to_tsconfig → TSConfig::marshal
//!
//! resolvereact call shape (both sides, mirroring the module crates'
//! resolver tests): `new_resolver(ResolverOptions{host, options})` +
//! `resolve_module_name(spec, containingFile, ESM, None)` per specifier,
//! with the project's CompilerOptions from its tsconfig (full showconfig
//! pipeline) or the tests' default `{Bundler, ESNext, ESNext}` literal.
//!
//! PORT notes (decisions shared with the Go driver):
//!   - The resolvereact specifier list is a fixed canonical list, not
//!     AST-extracted import strings — byte-stable by construction and
//!     driveable through the identical DefaultResolver call shape on both
//!     sides (the Go module tests resolve bare specifier strings, not
//!     parsed files).
//!   - The containing file is a synthetic `<dir>/__resolvereact__.ts` (only
//!     its directory participates in resolution) and the resolution mode is
//!     fixed ESM, exactly as the Go resolver tests' `ResolveModuleName`
//!     calls pass `core.ModuleKindESNext`.
//!   - The ResolutionHost's cwd is pinned to the absolute corpus root (no
//!     process state leaks into the output).
//!   - Unreadable config files emit the fixed marker
//!     `"error":"cannot read file"` (os:: errors and io::Errors stringify
//!     differently; fixed text keeps the diff meaningful).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tsc_core::compileroptions::ResolutionModeESM;
use tsc_core::options_generated::{CompilerOptions, ModuleKind, ModuleResolutionKind, ScriptTarget};
use tsc_module::types::{PackageId, ResolutionHost, ResolvedModule};
use tsc_module::{ResolverOptions, new_resolver};
use tsc_tspath::{RootedDirectoryPath, rooted_file_path_from_absolute};
use tsc_vfs::Fs;

use crate::parse::{SKIP_DIRS, json_escape};

// ──────────────────────────────────────────────────────────────────────
// CLI + corpus walks (Go: runShowConfig / indexConfigFiles /
// indexProjectDirs — dump's walk/skip rules, globally sorted by path)
// ──────────────────────────────────────────────────────────────────────

/// Parses the shared flag grammar of both subcommands (Go `flag` semantics:
/// `--x v`, `--x=v`, `-x`; the drivers only take `--corpus`/`--tsconfig`).
fn parse_cli(
    subcommand: &str,
    args: &[String],
    flags: &[&str],
) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let (name, inline) = match args[i].split_once('=') {
            Some((n, v)) => (n.trim_start_matches('-').to_string(), Some(v.to_string())),
            None => (args[i].trim_start_matches('-').to_string(), None),
        };
        if !flags.contains(&name.as_str()) {
            return Err(format!("{subcommand}: flag provided but not defined: -{name}"));
        }
        let value = match inline {
            Some(v) => v,
            None => {
                i += 1;
                args.get(i)
                    .cloned()
                    .ok_or_else(|| format!("{subcommand}: flag needs an argument: -{name}"))?
            }
        };
        out.push((name, value));
        i += 1;
    }
    Ok(out)
}

fn walk_dir(dir: &Path, out: &mut Vec<(PathBuf, bool)>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|err| format!("walking corpus: {err}"))?;
    let mut items: Vec<(String, PathBuf, bool)> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| format!("walking corpus: {err}"))?;
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        items.push((entry.file_name().to_string_lossy().into_owned(), entry.path(), is_dir));
    }
    items.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, path, is_dir) in items {
        if is_dir {
            // Same skip rules as dump (node_modules trees stay visible to
            // the resolver FS through osvfs, but are never walked).
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|name| SKIP_DIRS.contains(&name))
            {
                continue;
            }
            walk_dir(&path, out)?;
        } else {
            out.push((path, false));
        }
    }
    Ok(())
}

/// Go: `indexConfigFiles` — every tsconfig.json/jsconfig.json under the
/// corpus, globally sorted by path (raw string sort, like `sort.Strings`).
fn index_config_files(corpus: &str) -> Result<Vec<PathBuf>, String> {
    let abs = std::path::absolute(corpus).map_err(|err| format!("resolving corpus dir: {err}"))?;
    let mut entries = Vec::new();
    walk_dir(&abs, &mut entries)?;
    let mut paths: Vec<PathBuf> = entries
        .into_iter()
        .filter(|(path, _)| {
            matches!(
                path.file_name().and_then(|n| n.to_str()),
                Some("tsconfig.json") | Some("jsconfig.json")
            )
        })
        .map(|(path, _)| path)
        .collect();
    paths.sort_by(|a, b| a.as_os_str().cmp(b.as_os_str()));
    Ok(paths)
}

/// Go: `indexProjectDirs` — the sorted list of directories that directly
/// contain a tsconfig.json, jsconfig.json or package.json.
fn index_project_dirs(corpus: &str) -> Result<Vec<PathBuf>, String> {
    let abs = std::path::absolute(corpus).map_err(|err| format!("resolving corpus dir: {err}"))?;
    let mut entries = Vec::new();
    walk_dir(&abs, &mut entries)?;
    let mut dir_set: Vec<PathBuf> = entries
        .into_iter()
        .filter(|(path, _)| {
            matches!(
                path.file_name().and_then(|n| n.to_str()),
                Some("tsconfig.json") | Some("jsconfig.json") | Some("package.json")
            )
        })
        .map(|(path, _)| path.parent().unwrap().to_path_buf())
        .collect();
    dir_set.sort();
    dir_set.dedup();
    Ok(dir_set)
}

// ──────────────────────────────────────────────────────────────────────
// showconfig (Harness 1)
// ──────────────────────────────────────────────────────────────────────

/// Entry point for `tsc-bench showconfig --corpus <dir> | --tsconfig <path>`.
pub fn run_showconfig(args: &[String]) {
    let flags = match parse_cli("showconfig", args, &["corpus", "tsconfig"]) {
        Ok(flags) => flags,
        Err(err) => {
            eprintln!("tsc-bench: {err}");
            std::process::exit(2);
        }
    };
    let mut corpus = String::new();
    let mut tsconfig = String::new();
    for (name, value) in flags {
        match name.as_str() {
            "corpus" => corpus = value,
            "tsconfig" => tsconfig = value,
            _ => unreachable!(),
        }
    }
    if corpus.is_empty() && tsconfig.is_empty() {
        eprintln!("tsc-bench showconfig: missing required --corpus <dir> or --tsconfig <path>");
        std::process::exit(2);
    }
    let config_paths: Vec<PathBuf> = if !tsconfig.is_empty() {
        let abs = std::path::absolute(&tsconfig)
            .map_err(|err| format!("resolving tsconfig path: {err}"))
            .unwrap_or_else(|err| {
                eprintln!("tsc-bench: {err}");
                std::process::exit(1);
            });
        vec![abs]
    } else {
        index_config_files(&corpus).unwrap_or_else(|err| {
            eprintln!("tsc-bench: {err}");
            std::process::exit(1);
        })
    };
    let host = tsc_vfs::osvfs::fs();
    for path in config_paths {
        let mut line = String::with_capacity(256);
        emit_showconfig_line(&mut line, &path, host.as_ref());
        println!("{line}");
    }
}

/// Go: `emitShowConfigLine` — the NDJSON parity line for one config file.
fn emit_showconfig_line(line: &mut String, path: &Path, fs: &dyn Fs) {
    let path_str = path.to_string_lossy().replace('\\', "/");
    let file_name = rooted_file_path_from_absolute(&path_str);
    line.push_str("{\"file\":\"");
    json_escape(file_name.as_string(), line);
    line.push_str("\",");
    let Ok(text) = std::fs::read_to_string(path) else {
        // Fixed marker (PORT: os/io error strings differ between drivers).
        line.push_str("\"ok\":false,\"error\":\"cannot read file\"}");
        return;
    };
    let base_path = file_name.directory();
    let path_key = fs.case_sensitivity().path_key(&file_name.as_path());
    let (json, text_diags) =
        tsc_tsoptions::tsconfigparsing::parse_config_file_text_to_json(&text);
    let parsed = tsc_tsoptions::tsconfigparsing::parse_json_config_file_content(
        &json.unwrap_or(tsc_tsoptions::CompilerOptionsValue::Null),
        fs,
        &base_path,
        None,
        &file_name,
        &[],
        None,
        None,
    );
    let error_count = text_diags.len() + parsed.errors.len();
    let tsconfig = tsc_tsoptions::showconfig::convert_to_tsconfig(&parsed, &file_name);
    let out = tsconfig.marshal();
    let _ = path_key;
    line.push_str("\"ok\":true,\"out\":\"");
    json_escape(&out, line);
    line.push_str("\",\"errors\":");
    line.push_str(&error_count.to_string());
    line.push('}');
}

// ──────────────────────────────────────────────────────────────────────
// resolvereact (Harness 2)
// ──────────────────────────────────────────────────────────────────────

/// The fixed canonical specifier list — identical to the Go driver's
/// `resolvereactSpecifiers` (order included; the shape hash folds it).
const RESOLVEREACT_SPECIFIERS: &[&str] = &[
    "fs", "node:fs", "os",
    "./x", "./x.ts", "./index", "./a/b.js", "./missing", "../y",
    "pkg", "pkg/", "pkg/sub", "pkg/package.json", "pkg/main.js",
    "@scope/pkg", "@scope/pkg/sub", "@scope/missing",
    "idx", "cjspkg",
    "selfpkg", "#dep",
];

/// Go: `resolveReactHost` — the ResolutionHost the module tests construct
/// (`resolutionTestInputs{fs, cwd}`), with cwd pinned to the corpus root.
struct ResolveReactHost {
    fs: Arc<dyn Fs>,
    cwd: RootedDirectoryPath,
}

impl ResolutionHost for ResolveReactHost {
    fn fs(&self) -> Arc<dyn Fs> {
        Arc::clone(&self.fs)
    }
    fn get_current_directory(&self) -> RootedDirectoryPath {
        self.cwd.clone()
    }
}

/// Entry point for `tsc-bench resolvereact --corpus <dir>`.
pub fn run_resolvereact(args: &[String]) {
    let flags = match parse_cli("resolvereact", args, &["corpus"]) {
        Ok(flags) => flags,
        Err(err) => {
            eprintln!("tsc-bench: {err}");
            std::process::exit(2);
        }
    };
    let mut corpus = String::new();
    for (name, value) in flags {
        if name == "corpus" {
            corpus = value;
        }
    }
    if corpus.is_empty() {
        eprintln!("tsc-bench resolvereact: missing required --corpus <dir>");
        std::process::exit(2);
    }
    let abs_corpus = std::path::absolute(&corpus)
        .unwrap_or_else(|err| {
            eprintln!("tsc-bench: resolving corpus dir: {err}");
            std::process::exit(1);
        });
    let project_dirs = index_project_dirs(&corpus).unwrap_or_else(|err| {
        eprintln!("tsc-bench: {err}");
        std::process::exit(1);
    });
    let fs = tsc_vfs::osvfs::fs();
    let host: Arc<dyn ResolutionHost> = Arc::new(ResolveReactHost {
        fs: Arc::clone(&fs),
        cwd: RootedDirectoryPath::from(abs_corpus.to_string_lossy().replace('\\', "/").as_str()),
    });

    // shape_hash accumulator (FNV-1a 64, parse's shape_hash recipe).
    let mut h: u64 = 0xcbf29ce484222325;
    let fold = |h: &mut u64, s: &str| {
        for b in s.bytes() {
            *h ^= u64::from(b);
            *h = h.wrapping_mul(0x0100_0000_01b3);
        }
    };
    let mut specs: usize = 0;
    let mut resolved_count: usize = 0;
    let mut failed_count: usize = 0;
    let mut line = String::with_capacity(256);
    for dir in &project_dirs {
        let opts = project_compiler_options(dir, fs.as_ref());
        let kind = opts.module_resolution as i32;
        let resolver = new_resolver(ResolverOptions {
            host: Arc::clone(&host),
            compiler_options: Arc::new(opts),
            typings_location: RootedDirectoryPath::default(),
            project_name: String::new(),
            extra_extensions: Vec::new(),
            package_json_cache: None,
        });
        let dir_str = dir.to_string_lossy().replace('\\', "/");
        let containing_file =
            rooted_file_path_from_absolute(&format!("{dir_str}/__resolvereact__.ts"));
        for spec in RESOLVEREACT_SPECIFIERS {
            let (result, _trace) =
                resolver.resolve_module_name(spec, &containing_file, ResolutionModeESM, None);
            let resolved: Option<&ResolvedModule> =
                result.as_ref().map(|r| r.as_ref());
            let is_resolved = ResolvedModule::is_resolved(resolved);
            specs += 1;
            let mut external = false;
            let mut pkg_id = String::new();
            if is_resolved {
                resolved_count += 1;
                let r = resolved.unwrap();
                external = r.is_external_library_import;
                // pkgId only for a non-zero PackageId (Go: `!= (PackageId{})`).
                if r.package_id != PackageId::default() {
                    pkg_id = r.package_id.to_string();
                }
            } else {
                failed_count += 1;
            }
            line.clear();
            line.push_str("{\"file\":\"");
            json_escape(containing_file.as_string(), &mut line);
            line.push_str("\",\"specifier\":\"");
            json_escape(spec, &mut line);
            line.push_str("\",\"kind\":");
            line.push_str(&kind.to_string());
            if is_resolved {
                line.push_str(",\"resolved\":\"");
                json_escape(resolved.unwrap().resolved_file_name.as_string(), &mut line);
                line.push_str("\",\"failed\":false,\"external\":");
                line.push_str(if external { "true" } else { "false" });
            } else {
                line.push_str(",\"resolved\":null,\"failed\":true,\"external\":false");
            }
            if !pkg_id.is_empty() {
                line.push_str(",\"pkgId\":\"");
                json_escape(&pkg_id, &mut line);
                line.push('"');
            } else {
                line.push_str(",\"pkgId\":null");
            }
            line.push('}');
            println!("{line}");
            // Fold the record's fields into the shape hash.
            fold(&mut h, &format!("file={}\n", containing_file.as_string()));
            fold(&mut h, &format!("specifier={spec}\n"));
            fold(&mut h, &format!("kind={kind}\n"));
            if is_resolved {
                fold(
                    &mut h,
                    &format!("resolved={}\n", resolved.unwrap().resolved_file_name.as_string()),
                );
                fold(&mut h, "failed=0\n");
                fold(&mut h, &format!("external={}\n", if external { "1" } else { "0" }));
            } else {
                fold(&mut h, "resolved=\n");
                fold(&mut h, "failed=1\n");
                fold(&mut h, "external=0\n");
            }
            fold(&mut h, &format!("pkgId={pkg_id}\n"));
        }
    }
    println!(
        "{{\"summary\":true,\"dirs\":{},\"specs\":{},\"resolved\":{},\"failed\":{},\"shape_hash\":\"{h:016x}\"}}",
        project_dirs.len(),
        specs,
        resolved_count,
        failed_count
    );
}

/// Go: `projectCompilerOptions` — the project's CompilerOptions from its
/// tsconfig.json/jsconfig.json through the same pipeline the showconfig
/// twin drives, or the module tests' default literal.
fn project_compiler_options(dir: &Path, fs: &dyn Fs) -> CompilerOptions {
    let dir_str = dir.to_string_lossy().replace('\\', "/");
    for name in ["tsconfig.json", "jsconfig.json"] {
        let file_name = rooted_file_path_from_absolute(&format!("{dir_str}/{name}"));
        if !fs.file_exists(&file_name) {
            continue;
        }
        let Some(text) = fs.read_file(&file_name) else {
            break;
        };
        let (json, _diags) = tsc_tsoptions::tsconfigparsing::parse_config_file_text_to_json(&text);
        let parsed = tsc_tsoptions::tsconfigparsing::parse_json_config_file_content(
            &json.unwrap_or(tsc_tsoptions::CompilerOptionsValue::Null),
            fs,
            &file_name.directory(),
            None,
            &file_name,
            &[],
            None,
            None,
        );
        return parsed.compiler_options().clone();
    }
    // The default literal the module tests use (resolver_test.rs's
    // `bundler_opts(ModuleKind::ESNext)`).
    CompilerOptions {
        module_resolution: ModuleResolutionKind::Bundler,
        module: ModuleKind::ESNext,
        target: ScriptTarget::ESNext,
        ..CompilerOptions::default()
    }
}

// ──────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // Hand-verified FNV-1a 64 vectors for the resolvereact shape hash
    // recipe (h = 0xcbf29ce484222325; per byte: h ^= b; h *= 0x100000001b3
    // mod 2^64). Vectors computed step-by-step with an independent
    // implementation (Python) and cross-checked against the fold closure —
    // one unresolved record's exact fold stream:
    //
    //   fold("file=/x/__resolvereact__.ts\n") → 0x8862d222e94c0742
    //   fold("specifier=fs\n")               → 0x1d3e3973a4cc1dc0
    //   fold("kind=100\n")                    → 0x36a13acdba88857a
    //   fold("resolved=\n")                   → 0xa96e568c51c6b6ed
    //   fold("failed=1\n")                    → 0x953f1df2f86085e4
    //   fold("external=0\n")                  → 0x04a51d4ad456e9fc
    //   fold("pkgId=\n")                      → 0x978f39b83afa13a0
    #[test]
    fn resolvereact_shape_hash_vectors() {
        let mut h: u64 = 0xcbf29ce484222325;
        let fold = |h: &mut u64, s: &str| {
            for b in s.bytes() {
                *h ^= u64::from(b);
                *h = h.wrapping_mul(0x0100_0000_01b3);
            }
        };
        fold(&mut h, "file=/x/__resolvereact__.ts\n");
        assert_eq!(h, 0x8862d222e94c0742);
        fold(&mut h, "specifier=fs\n");
        assert_eq!(h, 0x1d3e3973a4cc1dc0);
        fold(&mut h, "kind=100\n");
        assert_eq!(h, 0x36a13acdba88857a);
        fold(&mut h, "resolved=\n");
        assert_eq!(h, 0xa96e568c51c6b6ed);
        fold(&mut h, "failed=1\n");
        assert_eq!(h, 0x953f1df2f86085e4);
        fold(&mut h, "external=0\n");
        assert_eq!(h, 0x04a51d4ad456e9fc);
        fold(&mut h, "pkgId=\n");
        assert_eq!(h, 0x978f39b83afa13a0);
    }

    #[test]
    fn cli_flag_grammar() {
        let flags = parse_cli("showconfig", &["--corpus".to_string(), "/x".to_string()], &["corpus", "tsconfig"]).unwrap();
        assert_eq!(flags, vec![("corpus".to_string(), "/x".to_string())]);
        // --x=v and -x forms (Go flag semantics).
        let flags = parse_cli(
            "resolvereact",
            &["--corpus=/x".to_string(), "-corpus".to_string(), "y".to_string()],
            &["corpus"],
        )
        .unwrap();
        assert_eq!(
            flags,
            vec![("corpus".to_string(), "/x".to_string()), ("corpus".to_string(), "y".to_string())]
        );
        assert!(parse_cli("showconfig", &["--wat".to_string()], &["corpus"]).is_err());
        assert!(parse_cli("showconfig", &["--corpus".to_string()], &["corpus"]).is_err());
    }

    /// The walk mirrors dump's skip rules: node_modules/.git/dist/build/
    /// out/.yarn are never descended into, and both config base names are
    /// collected in the global path sort.
    #[test]
    fn config_walk_skips_and_sorts() {
        let root = std::env::temp_dir().join("tsc-bench-showconfig-walk-test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("b-proj/src")).unwrap();
        std::fs::create_dir_all(root.join("a-proj")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        std::fs::create_dir_all(root.join("dist")).unwrap();
        std::fs::write(root.join("b-proj/tsconfig.json"), "{}").unwrap();
        std::fs::write(root.join("b-proj/jsconfig.json"), "{}").unwrap();
        std::fs::write(root.join("a-proj/tsconfig.json"), "{}").unwrap();
        std::fs::write(root.join("b-proj/src/other.json"), "{}").unwrap();
        std::fs::write(root.join("node_modules/pkg/tsconfig.json"), "{}").unwrap();
        std::fs::write(root.join("dist/tsconfig.json"), "{}").unwrap();

        let configs = index_config_files(root.to_str().unwrap()).unwrap();
        let names: Vec<String> = configs
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        // Global raw-path sort: a-proj/tsconfig.json < b-proj/jsconfig.json
        // < b-proj/tsconfig.json ('j' sorts before 't').
        assert_eq!(names, ["tsconfig.json", "jsconfig.json", "tsconfig.json"]);

        let dirs = index_project_dirs(root.to_str().unwrap()).unwrap();
        let last = dirs.last().unwrap();
        assert!(ends_with_dir(last, "b-proj"));
        assert_eq!(dirs.len(), 2);

        let _ = std::fs::remove_dir_all(&root);
    }

    fn ends_with_dir(path: &Path, name: &str) -> bool {
        path.file_name().is_some_and(|n| n == name)
    }
}

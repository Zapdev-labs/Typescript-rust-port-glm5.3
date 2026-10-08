// Port of tsc/internal/tsoptions/wildcarddirectories.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT notes:
//   - `vfsmatch.IsImplicitGlob` (tsc/internal/vfs/vfsmatch — MATCHING_
//     ALGORITHM.md) is ported as a local helper with a PORT banner: the full
//     SpecMatcher (724 Go LOC) belongs in a tsc-vfs wave; the exclude
//     matcher arm of `get_wildcard_directories` therefore accepts a
//     caller-supplied predicate so the M4 surface compiles without the
//     matcher. The wildcard-directory *derivation* logic is the full port.

use tsc_tspath::{
    combine_paths, get_root_length, normalize_path, rooted_directory_path_from_absolute,
    remove_trailing_directory_separator, CaseSensitivity, PathKey, RootedDirectoryPath,
    RootedPath, DIRECTORY_SEPARATOR,
};

/// Go: the parsedcommandline watch entry — a recursive flag per directory
/// (the watch-machinery fields stay with the watch wave).
#[derive(Clone, Copy, Debug, Default)]
pub struct WildcardDirectoryWatcher {
    pub recursive: bool,
}

/// Go: `vfsmatch.IsImplicitGlob` — a path component with no extension and no
/// glob characters is implicitly `component/**/*`.
///
/// PORT: from tsc/internal/vfs/vfsmatch (belongs to the future tsc-vfs
/// surface); the full SpecMatcher is not part of the M4 wave.
pub fn is_implicit_glob(last_path_component: &str) -> bool {
    !last_path_component.contains(['.', '*', '?'])
}

/// Go: `type wildcardDirectoryMatch struct`.
#[derive(Clone, Debug)]
struct WildcardDirectoryMatch {
    path: RootedDirectoryPath,
    recursive: bool,
}

/// Go: `func getWildcardDirectories[T](include, exclude, currentDirectory, caseSensitivity) map[tspath.RootedDirectoryPath]bool`.
///
/// The Go exclude arm runs the vfsmatch SpecMatcher; the port takes the
/// decision as a `is_excluded(spec) -> bool` predicate (see PORT notes) and
/// returns the same (directory, recursive) pairs.
pub fn get_wildcard_directories<T: AsRef<str>>(
    include: &[T],
    is_excluded: impl Fn(&str) -> bool,
    current_directory: &RootedDirectoryPath,
    case_sensitivity: CaseSensitivity,
) -> Vec<(RootedDirectoryPath, WildcardDirectoryWatcher)> {
    if include.is_empty() {
        return Vec::new();
    }

    // Ordered to keep deterministic iteration; the pairs are the Go map's.
    let mut wildcard_directories: Vec<(RootedDirectoryPath, WildcardDirectoryWatcher)> = Vec::new();
    let mut wildcard_key_to_index: Vec<(PathKey, usize)> = Vec::new();
    let mut recursive_keys: Vec<PathKey> = Vec::new();

    for file in include {
        let combined = combine_paths(current_directory.as_string(), &[file.as_ref()]);
        let spec = normalize_path(&combined);
        if is_excluded(&spec) {
            continue;
        }

        if let Some(m) = get_wildcard_directory_from_spec(&spec) {
            let mut path = m.path;
            if path.as_string().is_empty() {
                path = rooted_directory_path_from_absolute(&spec[..get_root_length(&spec)]);
            }
            let key = case_sensitivity.path_key(&path.as_path());
            let recursive = m.recursive;

            let existing = wildcard_key_to_index
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, i)| *i);

            if let Some(existing_index) = existing {
                let existing_recursive = wildcard_directories[existing_index].1.recursive;
                if !existing_recursive && recursive {
                    wildcard_directories[existing_index].1.recursive = recursive;
                }
            } else {
                wildcard_directories.push((path, WildcardDirectoryWatcher { recursive }));
                wildcard_key_to_index.push((key.clone(), wildcard_directories.len() - 1));
                if recursive {
                    recursive_keys.push(key);
                }
            }
        }

        // Remove any subpaths under an existing recursively watched directory.
        if !recursive_keys.is_empty() {
            wildcard_directories = wildcard_directories
                .into_iter()
                .filter(|(path, _)| {
                    let path_key = case_sensitivity.path_key(&path.as_path());
                    !recursive_keys
                        .iter()
                        .any(|recursive_key| {
                            path_key != *recursive_key && recursive_key.contains_path(&path_key)
                        })
                })
                .collect();
            wildcard_key_to_index = wildcard_directories
                .iter()
                .enumerate()
                .map(|(index, (path, _))| (case_sensitivity.path_key(&path.as_path()), index))
                .collect();
        }
    }

    wildcard_directories
}

/// Go: `func getWildcardDirectoryFromSpec(spec string) *wildcardDirectoryMatch`.
fn get_wildcard_directory_from_spec(spec: &str) -> Option<WildcardDirectoryMatch> {
    let first_wildcard = spec.find(['*', '?']);
    if let Some(first_wildcard) = first_wildcard {
        let last_sep_before_wildcard = spec[..first_wildcard].rfind(DIRECTORY_SEPARATOR);
        if let Some(last_sep_before_wildcard) = last_sep_before_wildcard {
            let mut path = RootedDirectoryPath::default();
            let path_text = &spec[..last_sep_before_wildcard];
            if !path_text.is_empty() {
                path = rooted_directory_path_from_absolute(path_text);
            }
            let last_directory_separator_index = spec.rfind(DIRECTORY_SEPARATOR);

            // Recursive if the wildcard appears in a directory segment
            // (not just the final file segment).
            let recursive = last_directory_separator_index
                .is_some_and(|last| first_wildcard < last);

            return Some(WildcardDirectoryMatch { path, recursive });
        }
    }

    if let Some((_, last_segment)) = spec.rsplit_once(DIRECTORY_SEPARATOR) {
        if is_implicit_glob(last_segment) {
            let path =
                rooted_directory_path_from_absolute(remove_trailing_directory_separator(spec));
            return Some(WildcardDirectoryMatch {
                path,
                recursive: true,
            });
        }
    }

    None
}

/// Placeholder so the `RootedPath` import stays tied to the watcher surface.
#[allow(dead_code)]
fn _rooted_path_type(_: &RootedPath) {}

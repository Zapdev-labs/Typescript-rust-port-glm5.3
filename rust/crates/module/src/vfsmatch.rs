// Ported from tsc/internal/vfs/vfsmatch/vfsmatch.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT (placement): this is `vfs/vfsmatch`, ported INTO the module crate
// because `resolver.go`'s GetEntrypointsFromPackageJsonInfo needs
// `ReadDirectory` and the tsc-vfs crate deliberately did not port the
// vfsmatch subpackage (nothing else imports it yet). Hoist this file into
// tsc-vfs when a second consumer appears; the code below is the Go source's
// ReadDirectory machinery, section by section.
//
// Deviations (each forced by the single consumer):
// - `ReadDirectory[T PathPatternInput]` is instantiated only with `string`
//   specs from the resolver, so the generic is dropped.
// - `SpecMatcher` is not ported (no user in the module package).
// - Go's `strings.EqualFold` becomes a local Unicode simple-fold comparison
//   (path entry names only).

use tsc_collections::Set;
use tsc_tspath::{
    CaseSensitivity, DIRECTORY_SEPARATOR, RootedDirectoryPath, RootedFilePath,
    ensure_trailing_directory_separator, file_extension_is_one_of, get_directory_path,
    get_normalized_path_components, get_root_length, has_extension,
    is_encoded_dynamic_file_name, is_rooted_disk_path, normalize_path, combine_paths,
    remove_trailing_directory_separator, rooted_directory_path_from_path,
    to_rooted_directory_path,
};
use tsc_vfs::Fs;

// Usage selects the pattern compilation rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Usage {
    Files,
    Directories,
    Exclude,
}

/// UnlimitedDepth can be passed as the depth argument to indicate there is no
/// depth limit.
pub const UNLIMITED_DEPTH: i32 = i32::MAX;

/// ReadDirectory walks the file system rooted at `path`, returning the files
/// that match the include patterns and don't match the exclude patterns.
///
/// Go: `func ReadDirectory[T PathPatternInput](host vfs.FS, path, extensions, excludes, includes, depth)`.
pub fn read_directory(
    host: &dyn Fs,
    path: &RootedDirectoryPath,
    extensions: &[String],
    excludes: &[String],
    includes: &[String],
    depth: i32,
) -> Vec<RootedFilePath> {
    match_file_names(path, extensions, excludes, includes, depth, host)
}

// IsImplicitGlob checks if a path component is implicitly a glob.
// An "includes" path "foo" is implicitly a glob "foo/** /*" (without the
// space) if its last component has no extension,
// and does not contain any glob characters itself.
pub fn is_implicit_glob(last_path_component: &str) -> bool {
    !last_path_component.contains(['.', '*', '?'])
}

fn get_include_base_path<'a>(absolute: &'a str) -> std::borrow::Cow<'a, str> {
    let wildcard_offset = absolute.find(['*', '?']);
    let Some(wildcard_offset) = wildcard_offset else {
        // No "*" or "?" in the path
        if !has_extension(absolute) {
            return std::borrow::Cow::Borrowed(absolute);
        }
        let directory_path = get_directory_path(absolute);
        return std::borrow::Cow::Owned(
            remove_trailing_directory_separator(&directory_path).to_string(),
        );
    };
    let root_length = get_root_length(absolute) as i64;
    let last_index = absolute[..wildcard_offset]
        .rfind(DIRECTORY_SEPARATOR)
        .map(|i| i as i64)
        .unwrap_or(-1);
    std::borrow::Cow::Borrowed(&absolute[..last_index.max(root_length) as usize])
}

// getBasePaths computes the unique non-wildcard base paths amongst the
// provided include patterns.
fn get_base_paths(
    path: &RootedDirectoryPath,
    includes: &[String],
    case_sensitivity: CaseSensitivity,
) -> Vec<RootedDirectoryPath> {
    // Storage for our results in the form of literal paths (e.g. the paths as
    // written by the user).
    let mut base_paths: Vec<RootedDirectoryPath> = vec![path.clone()];

    if !includes.is_empty() {
        // Storage for literal base paths amongst the include patterns.
        let mut include_base_paths: Vec<RootedDirectoryPath> = Vec::new();
        for include in includes {
            // We also need to check the relative paths by converting them to
            // absolute and normalizing in case they escape the base path
            // (e.g "..\somedirectory")
            let absolute = if is_rooted_disk_path(include) {
                normalize_path(include).into_owned()
            } else {
                normalize_path(&combine_paths(path.as_string(), &[include.as_str()])).into_owned()
            };
            // Append the literal and canonical candidate base paths.
            let base = get_include_base_path(&absolute);
            include_base_paths.push(to_rooted_directory_path(&base, path));
        }

        // Sort the offsets array using either the literal or canonical path
        // representations.
        // (Go slices.SortStableFunc — Rust sort_by is stable.)
        include_base_paths.sort_by(|a, b| {
            let cmp = case_sensitivity.compare_paths(&a.as_path(), &b.as_path());
            cmp.cmp(&0)
        });

        // Iterate over each include base path and include unique base paths
        // that are not a subpath of an existing base path
        for include_base_path in &include_base_paths {
            // Go: core.Every(basePaths, ...) — none of the existing base
            // paths contains this one.
            if base_paths.iter().all(|base_path| {
                !case_sensitivity.contains_path(base_path, &include_base_path.as_path())
            }) {
                base_paths.push(include_base_path.clone());
            }
        }
    }

    base_paths
}

// globPattern is a compiled glob pattern for matching file paths without regex.
#[derive(Clone, Debug)]
struct GlobPattern {
    components: Vec<Component>, // path segments to match (e.g., ["src", "**", "*.ts"])
    is_exclude: bool,           // exclude patterns have different matching rules
    case_sensitivity: CaseSensitivity,
    exclude_min_js: bool, // for "files" patterns, exclude .min.js by default
}

// component is a single path segment in a glob pattern.
// Examples: "src" (literal), "*" (wildcard), "*.ts" (wildcard), "**" (recursive)
#[derive(Clone, Debug)]
struct Component {
    kind: ComponentKind,
    literal: String,   // for kindLiteral: the exact string to match
    segments: Vec<Segment>, // for kindWildcard: parsed wildcard pattern
    // Include patterns with wildcards skip common package folders
    // (node_modules, etc.)
    skip_package_folders: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ComponentKind {
    // exact match (e.g., "src")
    Literal,
    // contains * or ? (e.g., "*.ts")
    Wildcard,
    // ** matches zero or more directories
    DoubleAsterisk,
}

// segment is a piece of a wildcard component.
// Example: "*.ts" becomes [segStar, segLiteral(".ts")]
#[derive(Clone, Debug, PartialEq, Eq)]
struct Segment {
    kind: SegmentKind,
    literal: String, // only for segLiteral
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SegmentKind {
    // exact text
    Literal,
    // * matches any chars except /
    Star,
    // ? matches single char except /
    Question,
}

// compileGlobPattern compiles a glob spec (e.g., "src/**/*.ts") into a
// pattern. Returns None if the pattern would match nothing.
fn compile_glob_pattern(
    spec: &str,
    base_path: &RootedDirectoryPath,
    usage: Usage,
    case_sensitivity: CaseSensitivity,
) -> Option<GlobPattern> {
    let mut parts = get_normalized_path_components(spec, base_path.as_string());
    let mut case_sensitivity = case_sensitivity;
    if is_encoded_dynamic_file_name(&parts[0]) {
        case_sensitivity = CaseSensitivity::CaseSensitive;
    }

    // "src/**" without a filename matches nothing (for include patterns)
    if usage != Usage::Exclude && parts.last().is_some_and(|p| p == "**") {
        return None;
    }

    // Normalize root: "/home/" -> "/home"
    parts[0] = remove_trailing_directory_separator(&parts[0]).to_string();
    if is_encoded_dynamic_file_name(&parts[0]) {
        let root_parts: Vec<String> = parts[0].split('/').map(|s| s.to_string()).collect();
        parts = root_parts.into_iter().chain(parts.into_iter().skip(1)).collect();
    }

    // Directories implicitly match all files: "src" -> "src/**/*"
    if parts.last().is_some_and(|p| is_implicit_glob(p)) {
        parts.push("**".to_string());
        parts.push("*".to_string());
    }

    let mut p = GlobPattern {
        is_exclude: usage == Usage::Exclude,
        case_sensitivity,
        exclude_min_js: usage == Usage::Files,
        // Avoid slice growth during compilation.
        components: Vec::with_capacity(parts.len()),
    };

    for part in &parts {
        p.components
            .push(parse_component(part, usage != Usage::Exclude));
    }
    Some(p)
}

// parseComponent converts a path segment string into a component.
fn parse_component(s: &str, is_include: bool) -> Component {
    if s == "**" {
        return Component {
            kind: ComponentKind::DoubleAsterisk,
            literal: String::new(),
            segments: Vec::new(),
            skip_package_folders: false,
        };
    }
    if !s.contains(['*', '?']) {
        return Component {
            kind: ComponentKind::Literal,
            literal: s.to_string(),
            segments: Vec::new(),
            skip_package_folders: false,
        };
    }
    Component {
        kind: ComponentKind::Wildcard,
        literal: String::new(),
        segments: parse_segments(s),
        skip_package_folders: is_include,
    }
}

// parseSegments breaks "*.ts" into [segStar, segLiteral(".ts")]
fn parse_segments(s: &str) -> Vec<Segment> {
    // Preallocate based on wildcard count: each wildcard contributes 1
    // segment, and each wildcard can split literals into at most one extra
    // literal segment.
    let mut wildcards = 0;
    for b in s.bytes() {
        if b == b'*' || b == b'?' {
            wildcards += 1;
        }
    }
    let mut result = Vec::with_capacity(2 * wildcards + 1);
    let bytes = s.as_bytes();
    let mut start = 0;
    for i in 0..s.len() {
        match bytes[i] {
            b'*' | b'?' => {
                if i > start {
                    result.push(Segment {
                        kind: SegmentKind::Literal,
                        literal: s[start..i].to_string(),
                    });
                }
                if bytes[i] == b'*' {
                    result.push(Segment {
                        kind: SegmentKind::Star,
                        literal: String::new(),
                    });
                } else {
                    result.push(Segment {
                        kind: SegmentKind::Question,
                        literal: String::new(),
                    });
                }
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < s.len() {
        result.push(Segment {
            kind: SegmentKind::Literal,
            literal: s[start..].to_string(),
        });
    }
    result
}

impl GlobPattern {
    // matches returns true if path matches this pattern.
    // Parity helper; kept for API parity.
    #[allow(dead_code)]
    fn matches(&self, path: &str) -> bool {
        self.match_path_parts(path, "", 0, 0, false)
    }

    // matchesParts returns true if prefix+suffix matches this pattern.
    // This avoids allocating a combined string for common call sites where
    // prefix ends with '/'.
    fn matches_parts(&self, prefix: &str, suffix: &str) -> bool {
        self.match_path_parts(prefix, suffix, 0, 0, false)
    }

    // matchesPrefixParts returns true if files under prefix+suffix could match.
    fn matches_prefix_parts(&self, prefix: &str, suffix: &str) -> bool {
        self.match_path_parts(prefix, suffix, 0, 0, true)
    }

    // matchPathParts is like matchPath, but operates on a virtual path formed
    // by prefix+suffix. Offsets are in the combined string.
    fn match_path_parts(
        &self,
        prefix: &str,
        suffix: &str,
        path_offset: usize,
        comp_idx: usize,
        prefix_only: bool,
    ) -> bool {
        let mut path_offset = path_offset;
        let mut comp_idx = comp_idx;
        loop {
            let Some((path_part, next_offset)) = next_path_part_parts(prefix, suffix, path_offset)
            else {
                if prefix_only {
                    return true;
                }
                return self.pattern_satisfied(comp_idx);
            };

            if comp_idx >= self.components.len() {
                return self.is_exclude && !prefix_only;
            }

            let comp = &self.components[comp_idx];
            match comp.kind {
                ComponentKind::DoubleAsterisk => {
                    if self.match_path_parts(prefix, suffix, path_offset, comp_idx + 1, prefix_only)
                    {
                        return true;
                    }
                    if !self.is_exclude && (is_hidden_path(path_part) || is_package_folder(path_part))
                    {
                        return false;
                    }
                    path_offset = next_offset;
                    continue;
                }
                ComponentKind::Literal => {
                    if comp.skip_package_folders && is_package_folder(path_part) {
                        panic!("unreachable: literal components never have skipPackageFolders");
                    }
                    if !self.strings_equal(&comp.literal, path_part) {
                        return false;
                    }
                }
                ComponentKind::Wildcard => {
                    if comp.skip_package_folders && is_package_folder(path_part) {
                        return false;
                    }
                    if !self.match_wildcard(&comp.segments, path_part) {
                        return false;
                    }
                }
            }

            path_offset = next_offset;
            comp_idx += 1;
        }
    }

    // patternSatisfied checks if remaining pattern components can match empty
    // input.
    fn pattern_satisfied(&self, comp_idx: usize) -> bool {
        // A pattern is satisfied when remaining components can match empty
        // input. For both include and exclude patterns, only trailing "**"
        // components may match nothing.
        self.components[comp_idx..]
            .iter()
            .all(|c| c.kind == ComponentKind::DoubleAsterisk)
    }

    // matchWildcard matches a path component against wildcard segments.
    fn match_wildcard(&self, segs: &[Segment], s: &str) -> bool {
        // Include patterns: wildcards at start cannot match hidden files
        if !self.is_exclude
            && !segs.is_empty()
            && is_hidden_path(s)
            && (segs[0].kind == SegmentKind::Star || segs[0].kind == SegmentKind::Question)
        {
            return false;
        }

        // Fast path: single * followed by literal suffix (e.g., "*.ts")
        if segs.len() == 2
            && segs[0].kind == SegmentKind::Star
            && segs[1].kind == SegmentKind::Literal
        {
            let suffix = &segs[1].literal;
            if s.len() < suffix.len() || !self.strings_equal(suffix, &s[s.len() - suffix.len()..]) {
                return false;
            }
            return self.should_include_min_js(s, segs);
        }

        self.match_segments(segs, s) && self.should_include_min_js(s, segs)
    }

    // matchSegments matches segments against string s using an iterative
    // algorithm. This avoids exponential backtracking by tracking only the
    // last star position. The algorithm is O(n*m) where n is the string
    // length and m is pattern length.
    fn match_segments(&self, segs: &[Segment], s: &str) -> bool {
        let mut seg_idx: i64 = 0;
        let mut s_idx: usize = 0;
        let mut star_seg_idx: i64 = -1;
        let mut star_s_idx: usize = 0;

        while s_idx < s.len() {
            if (seg_idx as usize) < segs.len() {
                let seg = &segs[seg_idx as usize];
                match seg.kind {
                    SegmentKind::Literal => {
                        let end = s_idx + seg.literal.len();
                        if end <= s.len() && self.strings_equal(&seg.literal, &s[s_idx..end]) {
                            s_idx = end;
                            seg_idx += 1;
                            continue;
                        }
                    }
                    SegmentKind::Question => {
                        if s.as_bytes()[s_idx] != b'/' {
                            let size = s[s_idx..].chars().next().map(|c| c.len_utf8()).unwrap_or(0);
                            s_idx += size;
                            seg_idx += 1;
                            continue;
                        }
                    }
                    SegmentKind::Star => {
                        // Record star position for backtracking, then try
                        // matching zero chars.
                        star_seg_idx = seg_idx;
                        star_s_idx = s_idx;
                        seg_idx += 1;
                        continue;
                    }
                }
            }

            // Current segment didn't match. Backtrack to last star if
            // possible.
            if star_seg_idx >= 0 && star_s_idx < s.len() && s.as_bytes()[star_s_idx] != b'/' {
                // Star consumes one more character (rune), retry from
                // segment after star.
                let size = s[star_s_idx..]
                    .chars()
                    .next()
                    .map(|c| c.len_utf8())
                    .unwrap_or(0);
                star_s_idx += size;
                s_idx = star_s_idx;
                seg_idx = star_seg_idx + 1;
                continue;
            }

            return false;
        }

        // Consume any trailing stars.
        while (seg_idx as usize) < segs.len() && segs[seg_idx as usize].kind == SegmentKind::Star {
            seg_idx += 1;
        }
        seg_idx >= segs.len() as i64
    }

    fn should_include_min_js(&self, filename: &str, segs: &[Segment]) -> bool {
        if !self.exclude_min_js {
            return true;
        }

        // Preserve legacy behavior:
        // - When matching is case-sensitive, only the exact ".min.js" suffix
        //   is excluded by default.
        // - When matching is case-insensitive, any casing variant is
        //   excluded by default.
        if !self.has_min_js_suffix(filename) {
            return true;
        }
        // Allow when the user's pattern explicitly references the .min.
        // suffix.
        if self.pattern_mentions_min_suffix(segs) {
            return true;
        }
        false
    }

    fn has_min_js_suffix(&self, filename: &str) -> bool {
        if self.case_sensitivity.is_case_sensitive() {
            return filename.ends_with(".min.js");
        }
        const MIN_JS: &str = ".min.js";
        if filename.len() < MIN_JS.len() {
            return false;
        }
        // Avoid allocating via strings.ToLower; compare suffix
        // case-insensitively.
        go_equal_fold(&filename[filename.len() - MIN_JS.len()..], MIN_JS)
    }

    fn pattern_mentions_min_suffix(&self, segs: &[Segment]) -> bool {
        for seg in segs {
            if seg.kind != SegmentKind::Literal {
                continue;
            }
            let lit = if self.case_sensitivity.is_case_insensitive() {
                seg.literal.to_lowercase()
            } else {
                seg.literal.clone()
            };
            if lit.contains(".min.js") || lit.contains(".min.") {
                return true;
            }
        }
        false
    }

    // stringsEqual compares strings with appropriate case sensitivity.
    fn strings_equal(&self, a: &str, b: &str) -> bool {
        if self.case_sensitivity.is_case_sensitive() {
            return a == b;
        }
        go_equal_fold(a, b)
    }
}

// nextPathPart extracts the next path component from path starting at
// offset. It returns the component text and the offset just past its
// trailing separator (in the combined prefix+suffix coordinate space when
// scanned via next_path_part_parts).
fn next_path_part_single(s: &str, offset: usize) -> Option<(&str, usize)> {
    let mut offset = offset;
    if offset >= s.len() {
        return None;
    }
    if offset == 0 && !s.is_empty() && s.as_bytes()[0] == b'/' {
        return Some(("", 1));
    }
    while offset < s.len() && s.as_bytes()[offset] == b'/' {
        offset += 1;
    }
    if offset >= s.len() {
        return None;
    }
    let rest = &s[offset..];
    if let Some(idx) = rest.as_bytes().iter().position(|&b| b == b'/') {
        return Some((&rest[..idx], offset + idx));
    }
    Some((rest, s.len()))
}

fn next_path_part_parts<'a>(prefix: &'a str, suffix: &'a str, offset: usize) -> Option<(&'a str, usize)> {
    // Fast paths: keep the hot single-string scan tight.
    if suffix.is_empty() {
        return next_path_part_single(prefix, offset);
    }
    if prefix.is_empty() {
        return next_path_part_single(suffix, offset);
    }

    // For matchFilesNoRegex call sites, prefix is a directory path ending in
    // '/', and suffix is a single entry name (no '/'). That makes this
    // significantly simpler than a general-purpose "virtual concatenation"
    // scanner.

    let total_len = prefix.len() + suffix.len();
    let mut offset = offset;
    if offset >= total_len {
        return None;
    }

    // Handle leading slash (root of absolute path)
    if offset == 0 && prefix.as_bytes()[0] == b'/' {
        return Some(("", 1));
    }

    // Scan within prefix.
    if offset < prefix.len() {
        while offset < prefix.len() && prefix.as_bytes()[offset] == b'/' {
            offset += 1;
        }
        if offset < prefix.len() {
            let rest = &prefix[offset..];
            let idx = rest
                .as_bytes()
                .iter()
                .position(|&b| b == b'/')
                // idx is guaranteed for the call sites we care about because
                // prefix ends in '/' (Go slices with -1 and would panic; the
                // port treats the whole remainder as one part instead).
                .unwrap_or(rest.len());
            return Some((&rest[..idx], offset + idx));
        }
        // Fall through into suffix region.
    }

    // Scan suffix: it's a single component.
    let s_off = offset.saturating_sub(prefix.len());
    if s_off >= suffix.len() {
        return None;
    }
    Some((&suffix[s_off..], total_len))
}

// go_equal_fold mirrors Go's strings.EqualFold (Unicode simple case
// folding); Rust has no std equivalent, so fold per char.
fn go_equal_fold(a: &str, b: &str) -> bool {
    let mut ai = a.chars().flat_map(char::to_lowercase);
    let mut bi = b.chars().flat_map(char::to_lowercase);
    loop {
        match (ai.next(), bi.next()) {
            (None, None) => return true,
            (Some(x), Some(y)) if x == y => {}
            _ => return false,
        }
    }
}

// isHiddenPath checks if a path component is hidden (starts with dot).
fn is_hidden_path(name: &str) -> bool {
    !name.is_empty() && name.as_bytes()[0] == b'.'
}

// isPackageFolder checks if name is a common package folder (node_modules,
// etc.)
fn is_package_folder(name: &str) -> bool {
    go_equal_fold(name, "node_modules")
        || go_equal_fold(name, "jspm_packages")
        || go_equal_fold(name, "bower_components")
}

// globMatcher combines include and exclude patterns for file matching.
struct GlobMatcher {
    includes: Vec<GlobPattern>,
    excludes: Vec<GlobPattern>,
    had_includes: bool, // true if include specs were provided (even if none compiled)
}

fn new_glob_matcher(
    include_specs: &[String],
    exclude_specs: &[String],
    base_path: &RootedDirectoryPath,
    case_sensitivity: CaseSensitivity,
    usage: Usage,
) -> GlobMatcher {
    let mut m = GlobMatcher {
        had_includes: !include_specs.is_empty(),
        includes: Vec::with_capacity(include_specs.len()),
        excludes: Vec::with_capacity(exclude_specs.len()),
    };

    for spec in include_specs {
        if let Some(p) = compile_glob_pattern(spec, base_path, usage, case_sensitivity) {
            m.includes.push(p);
        }
    }
    for spec in exclude_specs {
        if let Some(p) = compile_glob_pattern(spec, base_path, Usage::Exclude, case_sensitivity) {
            m.excludes.push(p);
        }
    }
    m
}

impl GlobMatcher {
    // matchesFileParts checks if prefix+suffix matches against the glob
    // patterns. Returns the index of the matching include pattern and true
    // if matched, or (0, false) if not.
    fn matches_file_parts(&self, prefix: &str, suffix: &str) -> Option<usize> {
        for exclude in &self.excludes {
            if exclude.matches_parts(prefix, suffix) {
                return None;
            }
        }
        if self.includes.is_empty() {
            if self.had_includes {
                return None;
            }
            return Some(0);
        }
        for (i, include) in self.includes.iter().enumerate() {
            if include.matches_parts(prefix, suffix) {
                return Some(i);
            }
        }
        None
    }

    // matchesDirectoryParts checks if files under the directory
    // prefix+suffix could match any pattern.
    fn matches_directory_parts(&self, prefix: &str, suffix: &str) -> bool {
        for exclude in &self.excludes {
            if exclude.matches_parts(prefix, suffix) {
                return false;
            }
        }
        if self.includes.is_empty() {
            return !self.had_includes;
        }
        for include in &self.includes {
            if include.matches_prefix_parts(prefix, suffix) {
                return true;
            }
        }
        false
    }
}

// globVisitor traverses directories matching files against glob patterns.
struct GlobVisitor<'a> {
    host: &'a dyn Fs,
    file_matcher: GlobMatcher,
    directory_matcher: GlobMatcher,
    extensions: Vec<String>,
    case_sensitivity: CaseSensitivity,
    visited: Set<tsc_tspath::PathKey>,
    results: Vec<Vec<RootedFilePath>>,
}

impl GlobVisitor<'_> {
    // visit walks a directory tree, collecting files that match the glob
    // patterns. resolvedRealPath, when non-empty, is the already-resolved
    // real path for this directory (computed incrementally from the parent).
    // When empty, Realpath is called to resolve symlinks.
    fn visit(
        &mut self,
        absolute_path: &RootedDirectoryPath,
        depth: i32,
        resolved_real_path: Option<RootedDirectoryPath>,
    ) {
        // Detect symlink cycles
        let real_path = resolved_real_path.unwrap_or_else(|| {
            rooted_directory_path_from_path(self.host.realpath(&absolute_path.as_path()))
        });
        let canonical_path = self.case_sensitivity.path_key(&real_path.as_path());
        if self.visited.has(&canonical_path) {
            return;
        }
        self.visited.add(canonical_path);

        let entries = self.host.get_accessible_entries(absolute_path);

        let abs_prefix = ensure_trailing_directory_separator(absolute_path.as_string()).into_owned();

        for file in &entries.files {
            if !self.extensions.is_empty() {
                let ext_refs: Vec<&str> =
                    self.extensions.iter().map(|s| s.as_str()).collect();
                if !file_extension_is_one_of(file, &ext_refs) {
                    continue;
                }
            }
            if let Some(idx) = self.file_matcher.matches_file_parts(&abs_prefix, file) {
                self.results[idx].push(absolute_path.resolve_file(file));
            }
        }

        let mut depth = depth;
        if depth != UNLIMITED_DEPTH {
            depth -= 1;
            if depth == 0 {
                return;
            }
        }

        for dir in &entries.directories {
            if !self.directory_matcher.matches_directory_parts(&abs_prefix, dir) {
                continue;
            }
            let abs_dir = absolute_path.resolve_directory(dir);
            let mut child_real_path = None;
            if let Some(symlinks) = &entries.symlinks {
                if !symlinks.contains(dir) {
                    // Non-symlink directory: compute realpath incrementally.
                    child_real_path = Some(real_path.resolve_directory(dir));
                }
                // else: symlink directory; leave childRealPath empty to
                // force Realpath call.
            }
            // If Symlinks is nil, the FS doesn't track symlinks;
            // leave childRealPath empty to call Realpath (preserving old
            // behavior).
            self.visit(&abs_dir, depth, child_real_path);
        }
    }
}

// matchFileNames is the ReadDirectory worker.
fn match_file_names(
    path: &RootedDirectoryPath,
    extensions: &[String],
    excludes: &[String],
    includes: &[String],
    depth: i32,
    host: &dyn Fs,
) -> Vec<RootedFilePath> {
    let case_sensitivity = host.case_sensitivity();

    let file_matcher = new_glob_matcher(includes, excludes, path, case_sensitivity, Usage::Files);
    let directory_matcher = new_glob_matcher(
        includes,
        excludes,
        path,
        case_sensitivity,
        Usage::Directories,
    );

    // Go: `results: make([][]string, max(len(fileMatcher.includes), 1))` —
    // computed off the matcher's compiled includes (the matchers must be
    // constructed first, so the length is a separate expression).
    let results_len = file_matcher.includes.len().max(1);
    let mut v = GlobVisitor {
        host,
        file_matcher,
        directory_matcher,
        extensions: extensions.to_vec(),
        case_sensitivity,
        visited: Set::new(),
        results: vec![Vec::new(); results_len],
    };

    for base_path in get_base_paths(path, includes, case_sensitivity) {
        v.visit(&base_path, depth, None);
    }

    // Fast path: a single include bucket (or no includes) doesn't need
    // flattening.
    if v.results.len() == 1 {
        return v.results.into_iter().next().unwrap_or_default();
    }
    // Go: core.Flatten(v.results).
    v.results.into_iter().flatten().collect()
}

// Ported from tsc/internal/vfs/vfstest/vfstest.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT: the Go package delegates its storage to testing/fstest.MapFS (a Go
// stdlib test double) and adapts it through fs.FS/Open plumbing
// (readDirFile/convertInfo/fileInfo in the Go file exist only to satisfy the
// fs.File/fs.ReadDirFile interfaces and re-tag Sys()). The port folds the
// required fstest.MapFS semantics directly into `MapFile`/`MapFS` and
// implements `sysfs::SubFs` on MapFS, which is what iovfs consumes — the
// observable behavior (symlink resolution, synthesized dirs, casing,
// realpaths) is preserved and the Go interface-satisfaction machinery is
// dropped. fstest.TestFS consistency checks are likewise stdlib test
// infrastructure and are not ported (noted in vfstest_test.rs).

use std::any::Any;
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::io;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime};

use tsc_tspath::CaseSensitivity;

use crate::sysfs::{FileMode, SubFs, SubDirEntry};
use crate::vfs::{FileInfo, err_not_exist};

const UMASK: FileMode = FileMode::from_bits(0o022);

/// MapFile mirrors `fstest.MapFile` (what vfstest stores per path): raw data,
/// mode, modification time, and an arbitrary user attachment.
#[derive(Clone, Default)]
pub struct MapFile {
    /// File data (for symlinks, the target path).
    pub data: Vec<u8>,
    /// File mode; the zero mode is a regular file.
    pub mode: FileMode,
    /// PORT: Go's zero time.Time becomes UNIX_EPOCH.
    pub mod_time: SystemTime,
    /// Go MapFile.Sys: arbitrary attached value.
    pub sys: Option<Arc<dyn Any + Send + Sync>>,
}

impl MapFile {
    pub fn from_data(data: Vec<u8>) -> MapFile {
        MapFile { data, ..Default::default() }
    }
}

impl From<&str> for MapFile {
    fn from(s: &str) -> MapFile {
        MapFile::from_data(s.as_bytes().to_vec())
    }
}

impl From<String> for MapFile {
    fn from(s: String) -> MapFile {
        MapFile::from_data(s.into_bytes())
    }
}

impl From<Vec<u8>> for MapFile {
    fn from(data: Vec<u8>) -> MapFile {
        MapFile::from_data(data)
    }
}

impl fmt::Debug for MapFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MapFile")
            .field("data", &self.data)
            .field("mode", &self.mode)
            .field("mod_time", &self.mod_time)
            .finish_non_exhaustive()
    }
}

/// Symlink returns a MapFile that is a symbolic link to the given target.
pub fn symlink(target: &str) -> MapFile {
    MapFile {
        data: target.as_bytes().to_vec(),
        mode: FileMode::SYMLINK,
        ..Default::default()
    }
}

/// Clock mirrors Go vfstest.Clock.
pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
    fn since_start(&self) -> Duration;
}

/// ClockImpl mirrors Go vfstest.clockImpl (Now is the wall clock, not
/// start-relative).
pub struct ClockImpl {
    start: Instant,
}

impl ClockImpl {
    pub fn new() -> ClockImpl {
        ClockImpl { start: Instant::now() }
    }
}

impl Default for ClockImpl {
    fn default() -> Self {
        ClockImpl::new()
    }
}

impl Clock for ClockImpl {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }

    fn since_start(&self) -> Duration {
        self.start.elapsed()
    }
}

// MapFS is an in-memory filesystem over canonical paths (Go vfstest.MapFS,
// merged with the fstest.MapFS it embeds — see the module PORT note). The
// mutex protects the file table; a single lock is sufficient because the
// SubFs methods only read or write it.
pub struct MapFS {
    mu: RwLock<MapFsState>,
    case_sensitivity: CaseSensitivity,
    clock: Arc<dyn Clock>,
}

struct MapFsState {
    // Keys are canonical paths.
    files: HashMap<String, StoredFile>,
    // Canonical symlink path -> canonical target path.
    symlinks: HashMap<String, String>,
}

struct StoredFile {
    file: MapFile,
    // Go stores this in the fstest.MapFile Sys field (sys.realpath): the
    // original (non-canonical) input path of the entry.
    realpath: String,
}

// The error channel of getFollowingSymlinks. Go uses fs.ErrNotExist for the
// plain miss and a *brokenSymlinkError for a dangling link; both carry the
// last-queried canonical path (`at`) because the write/append paths store the
// file there (Go: `file, cp, err := m.getFollowingSymlinks(...)` reuses cp
// even when err != nil).
#[derive(Debug)]
enum LookupError {
    NotFound { at: String },
    Broken { at: String, from: String, to: String },
}

impl LookupError {
    fn at(&self) -> &str {
        match self {
            LookupError::NotFound { at } => at,
            LookupError::Broken { at, .. } => at,
        }
    }
}

impl fmt::Display for LookupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LookupError::NotFound { .. } => write!(f, "file does not exist"),
            LookupError::Broken { from, to, .. } => {
                write!(f, "broken symlink {from:?} -> {to:?}")
            }
        }
    }
}

impl std::error::Error for LookupError {}

impl From<&LookupError> for io::Error {
    fn from(err: &LookupError) -> io::Error {
        match err {
            LookupError::NotFound { .. } => err_not_exist(),
            LookupError::Broken { .. } => io::Error::other(format!("{err}")),
        }
    }
}

/// FromMap creates a new [Fs] from a map of paths to file contents.
///
/// The paths must be normalized absolute paths according to the tspath package,
/// without trailing directory separators.
/// The paths must be all POSIX-style or all Windows-style, but not both.
pub fn from_map<V: Into<MapFile>>(
    m: impl IntoIterator<Item = (String, V)>,
    case_sensitivity: CaseSensitivity,
) -> Arc<dyn crate::vfs::Fs> {
    from_map_with_clock(m, case_sensitivity, Arc::new(ClockImpl::new()))
}

/// FromMapWithClock creates a new [Fs] from a map of paths to file contents.
/// See [from_map] for the path requirements.
pub fn from_map_with_clock<V: Into<MapFile>>(
    m: impl IntoIterator<Item = (String, V)>,
    case_sensitivity: CaseSensitivity,
    clock: Arc<dyn Clock>,
) -> Arc<dyn crate::vfs::Fs> {
    let mut posix = false;
    let mut windows = false;

    let mut mfs: Vec<(String, MapFile)> = Vec::new();
    // Sorted creation to ensure times are always guaranteed to be in order.
    let mut entries: Vec<(String, MapFile)> = m
        .into_iter()
        .map(|(path, file)| (path, file.into()))
        .collect();
    entries.sort_by(|a, b| compare_paths_by_parts(&a.0, &b.0));
    for (p, mut file) in entries {
        // checkPath
        if !tsc_tspath::is_rooted_disk_path(&p) {
            panic!("non-rooted path {p:?}");
        }
        let normal = tsc_tspath::remove_trailing_directory_separator(
            tsc_tspath::normalize_path(&p).as_ref(),
        );
        if normal != p {
            panic!("non-normalized path {p:?}");
        }

        if p.starts_with('/') {
            posix = true;
        } else {
            windows = true;
        }

        // PORT: Go sets ModTime to clock.Now() for every accepted file kind
        // (string, []byte and *fstest.MapFile all take this branch).
        file.mod_time = clock.now();

        if file.mode & FileMode::SYMLINK != FileMode::EMPTY {
            // checkPath on the target, then strip the leading "/" like Go.
            let target = String::from_utf8_lossy(&file.data).into_owned();
            if !tsc_tspath::is_rooted_disk_path(&target) {
                panic!("non-rooted path {target:?}");
            }
            let normal = tsc_tspath::remove_trailing_directory_separator(
                tsc_tspath::normalize_path(&target).as_ref(),
            );
            if normal != target {
                panic!("non-normalized path {target:?}");
            }
            file.data = target.strip_prefix('/').unwrap_or(&target).as_bytes().to_vec();
        }

        let p = p.strip_prefix('/').unwrap_or(&p).to_string();
        mfs.push((p, file));
    }

    if posix && windows {
        panic!("mixed posix and windows paths");
    }

    let converted = convert_map_fs(mfs, case_sensitivity, Some(clock));
    crate::iovfs::from(converted, case_sensitivity)
}

/// convertMapFS verifies a well-formed fstest-style map (relative, canonical
/// keys) and materializes it into a [MapFS], creating all intermediate
/// directories so every entry carries its realpath (Go convertMapFS).
pub fn convert_map_fs(
    input: impl IntoIterator<Item = (String, MapFile)>,
    case_sensitivity: CaseSensitivity,
    clock: Option<Arc<dyn Clock>>,
) -> Arc<MapFS> {
    let input: Vec<(String, MapFile)> = input.into_iter().collect();
    let clock = clock.unwrap_or_else(|| Arc::new(ClockImpl::new()));
    let m = Arc::new(MapFS {
        mu: RwLock::new(MapFsState { files: HashMap::new(), symlinks: HashMap::new() }),
        case_sensitivity,
        clock: Arc::clone(&clock),
    });

    let mut state = m.mu.write().unwrap();
    // Verify that the input is well-formed.
    let mut canonical_paths: HashMap<String, String> = HashMap::new();
    for (path, _) in &input {
        let canonical = m.canonical(path);
        if let Some(other) = canonical_paths.get(&canonical) {
            // Ensure consistent panic messages
            let (lo, hi) = if path.as_str() < other.as_str() {
                (path.as_str(), other.as_str())
            } else {
                (other.as_str(), path.as_str())
            };
            panic!("duplicate path: {lo:?} and {hi:?} have the same canonical path");
        }
        canonical_paths.insert(canonical, path.clone());
    }

    // Sort the input by depth and path so we ensure parent dirs are created
    // before their children, if explicitly specified by the input.
    let mut input = input;
    input.sort_by(|a, b| compare_paths_by_parts(&a.0, &b.0));

    for (p, file) in input {
        // Create all missing intermediate directories so we can attach the realpath to each of them.
        let dir = dir_name(&p);
        if !dir.is_empty() {
            if let Err(err) = mfs_mkdir_all(&m, &mut state, &dir, 0o777) {
                panic!("failed to create intermediate directories for {p:?}: {err}");
            }
        }
        let canonical = m.canonical(&p);
        set_entry(&m, &mut state, &p, &canonical, file);
    }

    drop(state);
    m
}

// comparePathsByParts compares two slash paths by "/"-separated component.
fn compare_paths_by_parts(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let mut a = a;
    let mut b = b;
    loop {
        // strings.Cut
        let a_ok = a.contains('/');
        let b_ok = b.contains('/');
        if !a_ok || !b_ok {
            return a.cmp(b);
        }
        let (a_start, a_end) = a.split_once('/').expect("checked");
        let (b_start, b_end) = b.split_once('/').expect("checked");
        match a_start.cmp(b_start) {
            Ordering::Equal => {}
            other => return other,
        }
        a = a_end;
        b = b_end;
    }
}

// splitPath splits s at the first "/" at or after offset. When none remains,
// Go returns (s, "") — the full string, not the remainder — and callers
// (mkdirAll) rely on that to check the complete prefix.
fn split_path_at(s: &str, offset: usize) -> (&str, &str) {
    match s[offset..].find('/') {
        None => (s, ""),
        Some(idx) => (&s[..idx + offset], &s[idx + 1 + offset..]),
    }
}

fn dir_name(p: &str) -> &str {
    match p.rfind('/') {
        Some(i) => &p[..i],
        None => "",
    }
}

fn base_name(p: &str) -> &str {
    match p.rfind('/') {
        Some(i) => &p[i + 1..],
        None => p,
    }
}

impl MapFS {
    fn canonical(&self, p: &str) -> String {
        self.case_sensitivity.canonicalize(p).into_owned()
    }
}

fn get_following_symlinks<'a>(
    state: &'a MapFsState,
    p: &str,
) -> Result<(&'a StoredFile, String), LookupError> {
    get_following_symlinks_worker(state, p, "", "")
}

fn get_following_symlinks_worker<'a>(
    state: &'a MapFsState,
    p: &str,
    symlink_from: &str,
    symlink_to: &str,
) -> Result<(&'a StoredFile, String), LookupError> {
    if let Some(file) = state.files.get(p) {
        if file.file.mode & FileMode::SYMLINK == FileMode::EMPTY {
            return Ok((file, p.to_string()));
        }
    }

    if let Some(target) = state.symlinks.get(p).cloned() {
        return get_following_symlinks_worker(state, &target, p, &target);
    }

    // This could be a path underneath a symlinked directory.
    for (other, target) in state.symlinks.iter() {
        if other.len() < p.len()
            && p.starts_with(other.as_str())
            && p.as_bytes()[other.len()] == b'/'
        {
            let new_p = format!("{target}{}", &p[other.len()..]);
            return get_following_symlinks_worker(state, &new_p, other, target);
        }
    }

    if symlink_from.is_empty() {
        Err(LookupError::NotFound { at: p.to_string() })
    } else {
        Err(LookupError::Broken {
            at: p.to_string(),
            from: symlink_from.to_string(),
            to: symlink_to.to_string(),
        })
    }
}

fn set_entry(
    m: &MapFS,
    state: &mut MapFsState,
    realpath: &str,
    canonical: &str,
    file: MapFile,
) {
    if realpath.is_empty() || canonical.is_empty() {
        panic!("empty path");
    }

    if file.mode & FileMode::SYMLINK != FileMode::EMPTY {
        let target = String::from_utf8_lossy(&file.data).into_owned();
        state
            .symlinks
            .insert(canonical.to_string(), m.canonical(&target));
    }
    state.files.insert(
        canonical.to_string(),
        StoredFile { file, realpath: realpath.to_string() },
    );
}

// mkdirAll creates dir and any missing parents (Go MapFS.mkdirAll). The walk
// restarts from a symlinked parent's realpath when it resolves elsewhere.
fn mfs_mkdir_all(
    m: &MapFS,
    state: &mut MapFsState,
    p: &str,
    perm: u32,
) -> io::Result<()> {
    if p.is_empty() {
        panic!("empty path");
    }

    // Fast path; already exists.
    if let Ok((other, _)) = get_following_symlinks(state, p) {
        if !other.file.mode.is_dir() {
            return Err(io::Error::other(format!(
                "mkdir {p:?}: path exists but is not a directory"
            )));
        }
        return Ok(());
    }

    let mut to_create: Vec<String> = Vec::new();
    let mut p = p.to_string();
    let mut offset = 0usize;
    loop {
        let (dir, rest) = split_path_at(&p, offset);
        let canonical = m.canonical(dir);
        match get_following_symlinks(state, &canonical) {
            Err(err) => {
                if let LookupError::NotFound { .. } = err {
                    to_create.push(dir.to_string());
                } else {
                    // PORT: Go returns any non-ErrNotExist error (a broken
                    // symlink) unchanged.
                    return Err((&err).into());
                }
            }
            Ok((other, other_path)) => {
                if !other.file.mode.is_dir() {
                    return Err(io::Error::other(format!(
                        "mkdir {other_path:?}: path exists but is not a directory"
                    )));
                }
                if canonical != other_path {
                    // We have a symlinked parent, reset and start again.
                    p = format!("{}/{}", other.realpath, rest);
                    to_create.clear();
                    offset = 0;
                    continue;
                }
            }
        }
        if rest.is_empty() {
            break;
        }
        offset = dir.len() + 1;
    }

    for dir in to_create {
        let canonical = m.canonical(&dir);
        set_entry(
            m,
            state,
            &dir,
            &canonical,
            MapFile {
                data: Vec::new(),
                mode: FileMode::DIR | (FileMode::from_bits(perm) & !UMASK),
                mod_time: m.clock.now(),
                sys: None,
            },
        );
    }

    Ok(())
}

fn mfs_remove(state: &mut MapFsState, canonical: &str) -> io::Result<()> {
    let Some(stored) = state.files.get(canonical) else {
        // file does not exist
        return Ok(());
    };
    let was_dir = stored.file.mode.is_dir();
    state.files.remove(canonical);
    state.symlinks.remove(canonical);

    if was_dir {
        let prefix = format!("{canonical}/");
        let children: Vec<String> = state
            .files
            .keys()
            .filter(|path| path.starts_with(&prefix))
            .cloned()
            .collect();
        for path in children {
            state.files.remove(&path);
            state.symlinks.remove(&path);
        }
    }
    Ok(())
}

// isSynthesizedDir reports whether a missed lookup should synthesize a
// directory: the root ("."), or any path that is a prefix of stored entries
// (fstest synthesizes intermediate directories; convert_map_fs pre-creates
// them for input paths, so in practice only "." synthesizes).
fn is_synthesized_dir(state: &MapFsState, canonical: &str) -> bool {
    if canonical == "." {
        return true;
    }
    let prefix = format!("{canonical}/");
    state.files.keys().any(|key| key.starts_with(&prefix))
}

// childEntries lists the direct children of a canonical directory, named by
// their stored realpaths (Go's readDirFile.ReadDir re-tags each fstest
// DirEntry through the fileInfo whose Name() is the realpath base).
fn child_entries(state: &MapFsState, dir: &str) -> Vec<SubDirEntry> {
    let prefix = if dir == "." { String::new() } else { format!("{dir}/") };
    let mut children: BTreeMap<String, SubDirEntry> = BTreeMap::new();
    for key in state.files.keys() {
        let Some(rest) = key.strip_prefix(&prefix) else { continue };
        if rest.is_empty() {
            // The directory itself.
            continue;
        }
        let first = rest.split('/').next().expect("rest is nonempty");
        if !children.contains_key(first) {
            let child_canonical = format!("{prefix}{first}");
            let entry = match state.files.get(&child_canonical) {
                Some(stored) => SubDirEntry {
                    name: base_name(&stored.realpath).to_string(),
                    kind: dir_entry_kind(stored.file.mode),
                },
                // A synthesized intermediate directory.
                None => SubDirEntry { name: first.to_string(), kind: FileMode::DIR },
            };
            children.insert(first.to_string(), entry);
        }
    }
    children.into_values().collect()
}

fn dir_entry_kind(mode: FileMode) -> FileMode {
    if mode.is_dir() {
        FileMode::DIR
    } else if mode & FileMode::SYMLINK != FileMode::EMPTY {
        FileMode::SYMLINK
    } else {
        // Regular files carry no type bits; other kinds are irregular.
        FileMode::EMPTY
    }
}

// MapFileInfo is the FileInfo produced by MapFS (Go vfstest.fileInfo).
struct MapFileInfo {
    name: String,
    mode: FileMode,
    mod_time: SystemTime,
    size: i64,
    sys: Option<Arc<dyn Any + Send + Sync>>,
}

impl FileInfo for MapFileInfo {
    fn name(&self) -> &str {
        &self.name
    }

    fn size(&self) -> i64 {
        self.size
    }

    fn mode(&self) -> FileMode {
        self.mode
    }

    fn mod_time(&self) -> SystemTime {
        self.mod_time
    }

    fn is_dir(&self) -> bool {
        self.mode.is_dir()
    }

    fn sys(&self) -> Option<&(dyn Any + Send + Sync)> {
        self.sys.as_deref()
    }
}

impl MapFS {
    fn file_info_from(&self, stored: &StoredFile) -> MapFileInfo {
        MapFileInfo {
            name: base_name(&stored.realpath).to_string(),
            mode: stored.file.mode,
            mod_time: stored.file.mod_time,
            size: stored.file.data.len() as i64,
            sys: stored.file.sys.clone(),
        }
    }

    // A synthesized directory's FileInfo, as fstest produces it: a dir with
    // mode ModeDir|0755 and zero size/time.
    fn synthesized_dir_info(name: &str) -> MapFileInfo {
        MapFileInfo {
            name: name.to_string(),
            mode: FileMode::DIR | FileMode::from_bits(0o755),
            mod_time: SystemTime::UNIX_EPOCH,
            size: 0,
            sys: None,
        }
    }
}

impl SubFs for MapFS {
    fn stat(&self, name: &str) -> io::Result<Arc<dyn FileInfo>> {
        let state = self.mu.read().unwrap();
        let canonical = self.canonical(name);
        match get_following_symlinks(&state, &canonical) {
            Ok((stored, _)) => Ok(Arc::new(self.file_info_from(stored))),
            Err(LookupError::NotFound { .. }) => {
                if is_synthesized_dir(&state, &canonical) {
                    Ok(Arc::new(Self::synthesized_dir_info(name)))
                } else {
                    Err(err_not_exist())
                }
            }
            Err(err @ LookupError::Broken { .. }) => Err((&err).into()),
        }
    }

    fn read_file(&self, name: &str) -> io::Result<Vec<u8>> {
        let state = self.mu.read().unwrap();
        let canonical = self.canonical(name);
        match get_following_symlinks(&state, &canonical) {
            Ok((stored, _)) => {
                if stored.file.mode.is_dir() {
                    return Err(io::Error::other(format!("{name:?} is a directory")));
                }
                Ok(stored.file.data.clone())
            }
            Err(_) => Err(err_not_exist()),
        }
    }

    fn read_dir(&self, name: &str) -> io::Result<Vec<SubDirEntry>> {
        let state = self.mu.read().unwrap();
        let canonical = self.canonical(name);
        match get_following_symlinks(&state, &canonical) {
            Ok((stored, resolved)) => {
                if !stored.file.mode.is_dir() {
                    return Err(io::Error::other(format!("{name:?} is not a directory")));
                }
                Ok(child_entries(&state, &resolved))
            }
            Err(LookupError::NotFound { .. }) => {
                if is_synthesized_dir(&state, &canonical) {
                    Ok(child_entries(&state, &canonical))
                } else {
                    Err(err_not_exist())
                }
            }
            Err(err @ LookupError::Broken { .. }) => Err((&err).into()),
        }
    }

    fn realpath(&self, name: &str) -> io::Result<String> {
        let state = self.mu.read().unwrap();
        let canonical = self.canonical(name);
        match get_following_symlinks(&state, &canonical) {
            Ok((stored, _)) => Ok(stored.realpath.clone()),
            Err(err) => Err((&err).into()),
        }
    }

    fn write_file(&self, name: &str, data: &str) -> io::Result<()> {
        let mut state = self.mu.write().unwrap();
        // PORT: iovfs passes perm 0o666.
        mfs_write_file(self, &mut state, name, data, 0o666, false)
    }

    fn append_file(&self, name: &str, data: &str) -> io::Result<()> {
        let mut state = self.mu.write().unwrap();
        mfs_write_file(self, &mut state, name, data, 0o666, true)
    }

    fn mkdir_all(&self, name: &str) -> io::Result<()> {
        let mut state = self.mu.write().unwrap();
        mfs_mkdir_all(self, &mut state, name, 0o777)
    }

    fn remove(&self, name: &str) -> io::Result<()> {
        let mut state = self.mu.write().unwrap();
        let canonical = self.canonical(name);
        mfs_remove(&mut state, &canonical)
    }

    fn chtimes(&self, name: &str, _atime: SystemTime, mtime: SystemTime) -> io::Result<()> {
        let mut state = self.mu.write().unwrap();
        let canonical = self.canonical(name);
        let Some(stored) = state.files.get_mut(&canonical) else {
            // file does not exist
            return Err(err_not_exist());
        };
        stored.file.mod_time = mtime;
        Ok(())
    }
}

// mfsWrite implements MapFS.WriteFile and MapFS.AppendFile (perm comes from
// iovfs' fixed 0o666).
fn mfs_write_file(
    m: &MapFS,
    state: &mut MapFsState,
    path: &str,
    data: &str,
    perm: u32,
    append: bool,
) -> io::Result<()> {
    let op = if append { "append" } else { "write" };

    if !dir_name(path).is_empty() {
        let parent_canonical = m.canonical(dir_name(path));
        match get_following_symlinks(state, &parent_canonical) {
            Err(err) => return Err(io::Error::other(format!("{op} {path:?}: {err}"))),
            Ok((parent, _)) => {
                if !parent.file.mode.is_dir() {
                    return Err(io::Error::other(format!(
                        "{op} {path:?}: parent path exists but is not a directory"
                    )));
                }
            }
        }
    }

    let mut existing: &[u8] = &[];
    let mut existing_mode = FileMode::EMPTY;
    let canonical = m.canonical(path);
    let cp: String;
    match get_following_symlinks(state, &canonical) {
        Err(err) => {
            // Go tolerates fs.ErrNotExist and broken symlinks (a broken file
            // symlink is replaced through its target path).
            cp = err.at().to_string();
        }
        Ok((stored, resolved)) => {
            if !stored.file.mode.is_regular() {
                return Err(io::Error::other(format!(
                    "{op} {path:?}: path exists but is not a regular file"
                )));
            }
            existing = &stored.file.data;
            existing_mode = stored.file.mode;
            cp = resolved;
        }
    }

    let mut combined = Vec::with_capacity(existing.len() + data.len());
    if append {
        combined.extend_from_slice(existing);
    }
    combined.extend_from_slice(data.as_bytes());

    let mut mode = existing_mode;
    if mode == FileMode::EMPTY {
        mode = FileMode::from_bits(perm) & !UMASK;
    }

    set_entry(
        m,
        state,
        path,
        &cp,
        MapFile { data: combined, mod_time: m.clock.now(), mode, sys: None },
    );

    Ok(())
}

impl MapFS {
    /// MkdirAll creates a directory and any missing parents (Go
    /// MapFS.MkdirAll; port callers reach [SubFs::mkdir_all], which fixes
    /// perm at 0o777 like iovfs does).
    pub fn mkdir_all(&self, path: &str, perm: u32) -> io::Result<()> {
        let mut state = self.mu.write().unwrap();
        mfs_mkdir_all(self, &mut state, path, perm)
    }

    /// AddSymlink adds a symbolic link (Go MapFS.AddSymlink).
    pub fn add_symlink(&self, path: &str, target: &str) {
        let mut state = self.mu.write().unwrap();
        let canonical = self.canonical(path);
        set_entry(
            self,
            &mut state,
            path,
            &canonical,
            MapFile { data: target.as_bytes().to_vec(), mode: FileMode::SYMLINK, ..Default::default() },
        );
    }

    /// GetTargetOfSymlink returns the target of the symlink at path, if any
    /// (Go MapFS.GetTargetOfSymlink; the result is re-prefixed with "/").
    pub fn get_target_of_symlink(&self, path: &str) -> Option<String> {
        let path = path.strip_prefix('/').unwrap_or(path);
        let state = self.mu.read().unwrap();
        let canonical = self.canonical(path);
        let stored = state.files.get(&canonical)?;
        if stored.file.mode & FileMode::SYMLINK != FileMode::EMPTY {
            return Some(format!(
                "/{}",
                String::from_utf8_lossy(&stored.file.data)
            ));
        }
        None
    }

    /// GetModTime returns the modification time of the file at path, or the
    /// zero time when it does not exist (Go MapFS.GetModTime).
    pub fn get_mod_time(&self, path: &str) -> SystemTime {
        let path = path.strip_prefix('/').unwrap_or(path);
        let state = self.mu.read().unwrap();
        let canonical = self.canonical(path);
        state
            .files
            .get(&canonical)
            .map(|stored| stored.file.mod_time)
            .unwrap_or(SystemTime::UNIX_EPOCH)
    }

    /// GetFileInfo returns a snapshot of the file stored at path, if any.
    ///
    /// PORT: Go returns the live *fstest.MapFile pointer; the port returns a
    /// clone (MapFile is cheap to clone and the sys field is shared via Arc).
    pub fn get_file_info(&self, path: &str) -> Option<MapFile> {
        let path = path.strip_prefix('/').unwrap_or(path);
        let state = self.mu.read().unwrap();
        let canonical = self.canonical(path);
        state.files.get(&canonical).map(|stored| stored.file.clone())
    }

    /// Entries returns every stored entry with its original (realpath) path,
    /// ordered by path parts (Go MapFS.Entries iterates an iter.Seq2; the
    /// port returns a snapshot Vec, like the other accessors).
    pub fn entries(&self) -> Vec<(String, MapFile)> {
        let state = self.mu.read().unwrap();
        let mut keys: Vec<&String> = state.files.keys().collect();
        keys.sort_by(|a, b| compare_paths_by_parts(a, b));
        keys.into_iter()
            .map(|key| {
                let stored = &state.files[key];
                let mut path = stored.realpath.clone();
                if !tsc_tspath::path_is_absolute(&path) {
                    path = format!("/{path}");
                }
                (path, stored.file.clone())
            })
            .collect()
    }
}

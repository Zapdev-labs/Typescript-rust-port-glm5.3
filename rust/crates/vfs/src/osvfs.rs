// Ported from tsc/internal/vfs/osvfs/os.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT: Go runs the case-sensitivity probe at package init; the port defers
// it to a LazyLock read (first FS() call or CaseSensitivity() — equivalent
// observable behavior, no init-order hazards). Go's `defer sema.Acquire()()`
// becomes a Drop guard so the slot is also released on panic.

use std::io;
use std::io::Write;
use std::sync::{Arc, LazyLock, OnceLock};
use std::time::SystemTime;

use tsc_core::semaphore::{LimitedSemaphore, Release, Semaphore, new_limited_semaphore};
use tsc_core::version::version_major_minor;
use tsc_nativepath::{is_symlink_or_reparse_point, realpath as native_realpath};
use tsc_osutil::osutil;
use tsc_tspath::{CaseSensitivity, RootedDirectoryPath, RootedFilePath, RootedPath, combine_paths, path_is_absolute, rooted_path_from_absolute};

use crate::internal::Common;
use crate::sysfs::{FileMode, SubFs, SubDirEntry};
use crate::vfs::{Entries, Fs, FileInfo};

// Semaphore for operations that are effectively blocking syscalls.
static BLOCKING_OP_SEMA: LazyLock<LimitedSemaphore> = LazyLock::new(|| new_limited_semaphore(128));
// Semaphore for file reads.
static READ_SEMA: LazyLock<LimitedSemaphore> = LazyLock::new(|| new_limited_semaphore(128));
// Semaphore for file writes.
static WRITE_SEMA: LazyLock<LimitedSemaphore> = LazyLock::new(|| new_limited_semaphore(32));

/// SemaGuard releases the semaphore slot on drop, matching Go's deferred
/// `sema.Acquire()()` release (which also runs during panics).
struct SemaGuard {
    release: Option<Release>,
}

impl Drop for SemaGuard {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            release();
        }
    }
}

fn acquire_blocking() -> SemaGuard {
    SemaGuard { release: Some(BLOCKING_OP_SEMA.acquire()) }
}

// PORT: Go filepath.FromSlash — a no-op except on Windows.
#[cfg(windows)]
fn from_slash(path: &str) -> String {
    path.replace('/', "\\")
}

#[cfg(not(windows))]
fn from_slash(path: &str) -> String {
    path.to_string()
}

/// FS creates a new FS from the OS file system (Go osvfs.FS; the port returns
/// the same shared instance Go's package-level osVFS provides).
pub fn fs() -> Arc<dyn Fs> {
    static OS_FS: OnceLock<Arc<dyn Fs>> = OnceLock::new();
    OS_FS
        .get_or_init(|| {
            Arc::new(OsFs {
                common: Common {
                    root_for: Box::new(|root: &str| {
                        Some(Arc::new(OsDirFs { root: root.to_string() }) as Arc<dyn SubFs>)
                    }),
                    is_reparse_point: Some(Box::new(|path: &str| {
                        is_symlink_or_reparse_point(&from_slash(path))
                    })),
                },
            })
        })
        .clone()
}

struct OsFs {
    common: Common,
}

// We do this right at startup to minimize the chance that executable gets
// moved or deleted. (PORT: Go computes it at package init; LazyLock defers
// the probe to first use, which for the compiler is also effectively startup.)
static FILE_SYSTEM_CASE_SENSITIVITY: LazyLock<CaseSensitivity> =
    LazyLock::new(detect_file_system_case_sensitivity);

fn detect_file_system_case_sensitivity() -> CaseSensitivity {
    // win32/win64 are case insensitive platforms
    if cfg!(target_os = "windows") {
        return CaseSensitivity::CaseInsensitive;
    }

    if cfg!(target_arch = "wasm32") {
        // !!! Who knows; this depends on the host implementation.
        return CaseSensitivity::CaseSensitive;
    }

    // As a proxy for case-insensitivity, we check if the current executable exists under a different case.
    // This is not entirely correct, since different OSs can have differing case sensitivity in different paths,
    // but this is largely good enough for our purposes (and what sys.ts used to do with __filename).
    let exe = osutil::executable()
        .unwrap_or_else(|err| panic!("vfs: failed to get executable path: {err}"));

    // If the current executable exists under a different case, we must be case-insensitive.
    let swapped = swap_case(&exe);
    match std::fs::metadata(&swapped) {
        Err(err) => {
            if err.kind() == io::ErrorKind::NotFound {
                return CaseSensitivity::CaseSensitive;
            }
            panic!("vfs: failed to stat {swapped:?}: {err}");
        }
        Ok(_) => CaseSensitivity::CaseInsensitive,
    }
}

// Convert all lowercase chars to uppercase, and vice-versa
//
// PORT: Go uses simple Unicode case mapping (single-rune unicode.ToUpper /
// ToLower); Rust std only exposes full mappings, so runes whose full mapping
// expands (e.g. 'ß' -> "SS") are left unchanged — which is what Go's simple
// mapping does for them too.
fn swap_case(s: &str) -> String {
    fn simple_upper(r: char) -> char {
        let mut it = r.to_uppercase();
        match (it.next(), it.next()) {
            (Some(u), None) => u,
            _ => r,
        }
    }
    fn simple_lower(r: char) -> char {
        let mut it = r.to_lowercase();
        match (it.next(), it.next()) {
            (Some(l), None) => l,
            _ => r,
        }
    }
    s.chars()
        .map(|r| {
            let upper = simple_upper(r);
            if upper == r { simple_lower(r) } else { upper }
        })
        .collect()
}

impl Fs for OsFs {
    fn case_sensitivity(&self) -> CaseSensitivity {
        *FILE_SYSTEM_CASE_SENSITIVITY
    }

    fn read_file(&self, path: &RootedFilePath) -> Option<String> {
        let _release = SemaGuard { release: Some(READ_SEMA.acquire()) };
        self.common.read_file(path)
    }

    fn directory_exists(&self, path: &RootedDirectoryPath) -> bool {
        let _release = acquire_blocking();
        self.common.directory_exists(path)
    }

    fn file_exists(&self, path: &RootedFilePath) -> bool {
        let _release = acquire_blocking();
        self.common.file_exists(path)
    }

    fn get_accessible_entries(&self, path: &RootedDirectoryPath) -> Entries {
        let _release = acquire_blocking();
        self.common.get_accessible_entries(path)
    }

    fn stat(&self, path: &RootedPath) -> Option<Arc<dyn FileInfo>> {
        let _release = acquire_blocking();
        self.common.stat(path)
    }

    fn realpath(&self, path: &RootedPath) -> RootedPath {
        let _release = acquire_blocking();
        os_fs_realpath(path)
    }

    fn write_file(&self, path: &RootedFilePath, content: &str) -> io::Result<()> {
        // PORT: os.O_WRONLY|os.O_CREATE|os.O_TRUNC.
        self.write_file_ensuring_dir(path, content, /* append */ false)
    }

    fn append_file(&self, path: &RootedFilePath, content: &str) -> io::Result<()> {
        // PORT: os.O_WRONLY|os.O_CREATE|os.O_APPEND.
        self.write_file_ensuring_dir(path, content, /* append */ true)
    }

    fn remove(&self, path: &RootedPath) -> io::Result<()> {
        let _release = acquire_blocking();
        // todo: #701 add retry mechanism?
        os_remove_all(&from_slash(path.as_string()))
    }

    fn chtimes(&self, path: &RootedPath, atime: SystemTime, mtime: SystemTime) -> io::Result<()> {
        let _release = acquire_blocking();
        // PORT: Go os.Chtimes opens the file and utimensats through it;
        // File::set_times (futimens) on an O_RDWR handle matches.
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(from_slash(path.as_string()))?;
        file.set_times(
            std::fs::FileTimes::new().set_accessed(atime).set_modified(mtime),
        )
    }
}

impl OsFs {
    fn write_file_with_flag(&self, path: &str, content: &str, append: bool) -> io::Result<()> {
        let _release = SemaGuard { release: Some(WRITE_SEMA.acquire()) };

        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true);
        if append {
            options.append(true);
        } else {
            options.truncate(true);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o666);
        }

        let mut file = options.open(from_slash(path))?;
        file.write_all(content.as_bytes())
    }

    fn ensure_directory_exists(&self, directory_path: &str) -> io::Result<()> {
        let _release = acquire_blocking();
        // PORT: Go os.MkdirAll(dir, 0o777); Rust's create_dir_all applies the
        // same 0777&umask on unix.
        std::fs::create_dir_all(from_slash(directory_path))
    }

    fn write_file_ensuring_dir(
        &self,
        path: &RootedFilePath,
        content: &str,
        append: bool,
    ) -> io::Result<()> {
        let path_string = path.as_string().to_string();
        if self.write_file_with_flag(&path_string, content, append).is_ok() {
            return Ok(());
        }
        self.ensure_directory_exists(path.directory().as_string())?;
        self.write_file_with_flag(&path_string, content, append)
    }
}

fn os_fs_realpath(path: &RootedPath) -> RootedPath {
    let orig = path.clone();
    let native_path = from_slash(path.as_string());
    let native_path = match native_realpath(&native_path) {
        Ok(resolved) => resolved,
        Err(_) => return orig,
    };
    // PORT: Go calls filepath.Abs (which also cleans); EvalSymlinks output is
    // already clean, so only the relative-path fallback differs in shape.
    let native_path = if path_is_absolute(&native_path) {
        native_path
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(&native_path).to_string_lossy().into_owned(),
            Err(_) => return orig,
        }
    };
    rooted_path_from_absolute(&native_path)
}

// PORT: Go os.RemoveAll — remove the path itself (a symlink removes the link,
// not the target) and, for directories, everything below; a missing path is
// success.
fn os_remove_all(path: &str) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
        Ok(meta) => {
            if meta.is_dir() {
                std::fs::remove_dir_all(path)
            } else {
                std::fs::remove_file(path)
            }
        }
    }
}

// GetGlobalTypingsCacheLocation returns a directory to use as the global
// typings cache location.
pub fn get_global_typings_cache_location() -> String {
    // PORT: Go os.UserCacheDir — unix: $XDG_CACHE_HOME or $HOME/.cache;
    // windows: %LocalAppData%. Empty values count as unset, and failures fall
    // back to the temp dir like Go's os.TempDir().
    let cache_dir = user_cache_dir().unwrap_or_else(|| {
        std::env::temp_dir().to_string_lossy().into_owned()
    });

    let subdir = if cfg!(target_os = "windows") {
        "Microsoft/TypeScript"
    } else {
        "typescript"
    };
    combine_paths(&cache_dir, &[subdir, version_major_minor()])
}

#[cfg(unix)]
fn user_cache_dir() -> Option<String> {
    if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
        let s = xdg.to_string_lossy().into_owned();
        if !s.is_empty() {
            return Some(s);
        }
    }
    let home = std::env::var_os("HOME")?;
    let home = home.to_string_lossy().into_owned();
    if home.is_empty() {
        return None;
    }
    Some(format!("{home}/.cache"))
}

#[cfg(windows)]
fn user_cache_dir() -> Option<String> {
    let local_app_data = std::env::var_os("LocalAppData")?;
    let s = local_app_data.to_string_lossy().into_owned();
    if s.is_empty() {
        return None;
    }
    Some(s)
}

#[cfg(not(any(unix, windows)))]
fn user_cache_dir() -> Option<String> {
    None
}

// OsDirFs is the RootFor target for osvfs: an fs.FS rooted at a tspath root
// (Go os.DirFS).
struct OsDirFs {
    root: String,
}

impl OsDirFs {
    fn join(&self, name: &str) -> String {
        if name == "." {
            return self.root.clone();
        }
        let trimmed = self.root.trim_end_matches('/');
        if trimmed.is_empty() {
            format!("/{name}")
        } else {
            format!("{trimmed}/{name}")
        }
    }
}

impl SubFs for OsDirFs {
    fn stat(&self, name: &str) -> io::Result<Arc<dyn FileInfo>> {
        let path = self.join(name);
        let meta = std::fs::metadata(from_slash(&path))?;
        Ok(Arc::new(OsFileInfo {
            name: std::path::Path::new(&path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.clone()),
            size: meta.len() as i64,
            mode: os_file_mode(&meta),
            mod_time: mod_time_of(&meta),
            is_dir: meta.is_dir(),
        }))
    }

    fn read_file(&self, name: &str) -> io::Result<Vec<u8>> {
        std::fs::read(from_slash(&self.join(name)))
    }

    fn read_dir(&self, name: &str) -> io::Result<Vec<SubDirEntry>> {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(from_slash(&self.join(name)))? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let kind = if file_type.is_dir() {
                FileMode::DIR
            } else if file_type.is_symlink() {
                FileMode::SYMLINK
            } else if file_type.is_file() {
                FileMode::EMPTY
            } else {
                // PORT: sockets/devices/etc. — Go's DirEntry.Type reports
                // ModeSocket/ModeDevice/ModeCharDevice; the Common layer only
                // distinguishes dir/regular/symlink/other, so they collapse to
                // ModeIrregular here.
                FileMode::IRREGULAR
            };
            entries.push(SubDirEntry {
                // PORT: Go tolerates non-UTF-8 names; the port losses them.
                name: entry.file_name().to_string_lossy().into_owned(),
                kind,
            });
        }
        // Go fs.ReadDir sorts by filename.
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }
}

struct OsFileInfo {
    name: String,
    size: i64,
    mode: FileMode,
    mod_time: SystemTime,
    is_dir: bool,
}

impl FileInfo for OsFileInfo {
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
        self.is_dir
    }

    fn sys(&self) -> Option<&(dyn std::any::Any + Send + Sync)> {
        None
    }
}

fn os_file_mode(meta: &std::fs::Metadata) -> FileMode {
    let mut mode = FileMode::EMPTY;
    if meta.is_dir() {
        mode |= FileMode::DIR;
    }
    #[cfg(unix)]
    {
        mode |= FileMode::from_bits(std::os::unix::fs::MetadataExt::mode(meta) & 0o777);
    }
    mode
}

fn mod_time_of(meta: &std::fs::Metadata) -> SystemTime {
    meta.modified().unwrap_or(SystemTime::UNIX_EPOCH)
}

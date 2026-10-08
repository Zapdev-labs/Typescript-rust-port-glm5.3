// Ported from tsc/internal/vfs/internal/internal.go @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT: Go's internal package is unexported; `pub(crate)` mirrors that here.
// Go's `SplitPath` helper is not ported (no caller in the ported packages).

use std::collections::HashSet;
use std::sync::Arc;

use tsc_tspath::{RootedDirectoryPath, RootedFilePath, RootedPath, get_encoded_root_length};

use crate::sysfs::{FileMode, SubFs, SubDirEntry};
use crate::vfs::{Entries, FileInfo};

/// Common is the shared vfs implementation over an arbitrary `SubFs` (Go
/// `internal.Common`): callers provide `root_for` — which maps a tspath root
/// like "/" or "c:/" onto a sub-filesystem (Go's `RootFor func(root string)
/// fs.FS`, where nil means the root is unusable, e.g. a URL) — and optionally
/// `is_reparse_point` for Windows reparse-point detection.
// PORT: the boxed-closure field types mirror Go's function-typed fields; the
// complexity allow covers exactly those two fields.
#[allow(clippy::type_complexity)]
pub(crate) struct Common {
    pub root_for: Box<dyn Fn(&str) -> Option<Arc<dyn SubFs>> + Send + Sync>,
    pub is_reparse_point: Option<Box<dyn Fn(&str) -> bool + Send + Sync>>,
}

// RootLength returns the length of the path root, panicking on relative paths
// (Go internal.RootLength).
pub(crate) fn root_length(p: &str) -> usize {
    let l = get_encoded_root_length(p);
    if l == 0 {
        panic!("vfs: path {p:?} is not absolute");
    } else if l < 0 {
        // Go: `return ^l` (bitwise NOT of the encoded negative length).
        return !l as usize;
    }
    l as usize
}

impl Common {
    // RootAndPath splits path into the sub-filesystem for its root, the root
    // itself, and the path relative to that root ("." for the root itself).
    fn root_and_path(&self, path: &RootedPath) -> (Option<Arc<dyn SubFs>>, RootedDirectoryPath, String) {
        let _ = root_length(path.as_string());
        let (root, rest) = path.root_and_relative_path();
        let rest = if rest.is_empty() { ".".to_string() } else { rest.to_string() };
        ((self.root_for)(root.as_string()), root, rest)
    }

    pub(crate) fn stat(&self, path: &RootedPath) -> Option<Arc<dyn FileInfo>> {
        let (fsys, _, rest) = self.root_and_path(path);
        let fsys = fsys?;
        fsys.stat(&rest).ok()
    }

    pub(crate) fn file_exists(&self, path: &RootedFilePath) -> bool {
        match self.stat(&path.as_path()) {
            Some(stat) => !stat.is_dir(),
            None => false,
        }
    }

    pub(crate) fn directory_exists(&self, path: &RootedDirectoryPath) -> bool {
        match self.stat(&path.as_path()) {
            Some(stat) => stat.is_dir(),
            None => false,
        }
    }

    pub(crate) fn get_accessible_entries(&self, path: &RootedDirectoryPath) -> Entries {
        let mut result = Entries {
            symlinks: Some(HashSet::new()),
            ..Default::default()
        };

        fn add_to_result(result: &mut Entries, name: &str, mode: FileMode, is_link: bool) -> bool {
            if mode.is_dir() {
                result.directories.push(name.to_string());
            } else if mode.is_regular() {
                result.files.push(name.to_string());
            } else {
                return false;
            }

            if is_link {
                result
                    .symlinks
                    .get_or_insert_with(HashSet::new)
                    .insert(name.to_string());
            }
            true
        }

        for entry in self.get_entries(path) {
            let entry_type = entry.kind;

            if add_to_result(&mut result, &entry.name, entry_type, false) {
                continue;
            }

            if entry_type & FileMode::SYMLINK != FileMode::EMPTY {
                // Easy case; UNIX-like system will clearly mark symlinks.
                if let Some(stat) = self.stat(&path.resolve_file(&entry.name).as_path()) {
                    add_to_result(&mut result, &entry.name, stat.mode(), true);
                }
                continue;
            }

            if entry_type & FileMode::IRREGULAR != FileMode::EMPTY
                && let Some(is_reparse_point) = self.is_reparse_point.as_ref()
            {
                // Could be a Windows junction or other reparse point.
                // Check using the OS-specific helper.
                let full_path = path.resolve_file(&entry.name);
                if is_reparse_point(full_path.as_string()) {
                    if let Some(stat) = self.stat(&full_path.as_path()) {
                        add_to_result(&mut result, &entry.name, stat.mode(), true);
                    }
                }
                continue;
            }
        }

        result
    }

    fn get_entries(&self, path: &RootedDirectoryPath) -> Vec<SubDirEntry> {
        let (fsys, _, rest) = self.root_and_path(&path.as_path());
        let Some(fsys) = fsys else {
            return Vec::new();
        };
        fsys.read_dir(&rest).unwrap_or_default()
    }

    pub(crate) fn read_file(&self, path: &RootedFilePath) -> Option<String> {
        let (fsys, _, rest) = self.root_and_path(&path.as_path());
        let fsys = fsys?;
        let bytes = fsys.read_file(&rest).ok()?;
        decode_bytes(bytes)
    }
}

// decodeBytes decodes raw file contents: UTF-16 with BOM is transcoded, a
// UTF-8 BOM is stripped, and anything else is returned as-is.
//
// PORT: Go converts the (immutable) byte slice to a string with
// unsafe.String without copying and relies on Go strings tolerating invalid
// UTF-8; Rust String must be valid UTF-8, so non-UTF-8 contents yield ok=false
// (documented divergence; tsc input files are UTF-8/UTF-16 anyway).
pub(crate) fn decode_bytes(bytes: Vec<u8>) -> Option<String> {
    if bytes.len() >= 2 {
        match [bytes[0], bytes[1]] {
            [0xFF, 0xFE] => return Some(decode_utf16(&bytes[2..], Utf16Endian::Little)),
            [0xFE, 0xFF] => return Some(decode_utf16(&bytes[2..], Utf16Endian::Big)),
            _ => {}
        }
    }
    let s = if bytes.len() >= 3 && bytes[0] == 0xEF && bytes[1] == 0xBB && bytes[2] == 0xBF {
        &bytes[3..]
    } else {
        &bytes[..]
    };
    String::from_utf8(s.to_vec()).ok()
}

enum Utf16Endian {
    Little,
    Big,
}

// decodeUtf16 mirrors Go's decodeUtf16: binary.Read into []uint16 followed by
// utf16.Decode. An odd-length body makes binary.Read fail and Go returns "";
// the port keeps that. Unpaired surrogates become U+FFFD like utf16.Decode.
fn decode_utf16(bytes: &[u8], order: Utf16Endian) -> String {
    if bytes.len() % 2 != 0 {
        return String::new();
    }
    let mut units = Vec::with_capacity(bytes.len() / 2);
    for chunk in bytes.chunks_exact(2) {
        let unit = match order {
            Utf16Endian::Little => u16::from_le_bytes([chunk[0], chunk[1]]),
            Utf16Endian::Big => u16::from_be_bytes([chunk[0], chunk[1]]),
        };
        units.push(unit);
    }
    let mut out = String::with_capacity(units.len());
    let mut i = 0;
    while i < units.len() {
        let unit = units[i];
        if (0xD800..0xDC00).contains(&unit)
            && i + 1 < units.len()
            && (0xDC00..0xE000).contains(&units[i + 1])
        {
            let code_point =
                0x1_0000 + (((unit as u32) - 0xD800) << 10) + ((units[i + 1] as u32) - 0xDC00);
            out.push(char::from_u32(code_point).unwrap_or('\u{FFFD}'));
            i += 2;
        } else {
            out.push(char::from_u32(unit as u32).unwrap_or('\u{FFFD}'));
            i += 1;
        }
    }
    out
}

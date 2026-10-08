// Ported from the Go io/fs subset used by tsc/internal/vfs and its
// subpackages @ ec47d33c23e464a17cdf2475632cba629bee8763
//
// PORT: Go builds vfs backends on io/fs interfaces (fs.FS, fs.FileInfo,
// fs.DirEntry, fs.FileMode) and detects the RealpathFS / WritableFS
// capability interfaces from iovfs via type assertion. Rust has no stdlib
// equivalent, so this module carries the pieces the port needs:
//
//   - `FileMode` mirrors fs.FileMode (same bit layout).
//   - `SubFs` mirrors an fs.FS rooted at some root ("." is that root; names
//     never begin or end with "/"), with RealpathFS/WritableFS folded in as
//     default methods: the defaults reproduce what iovfs.From does when the
//     wrapped fs.FS does *not* implement those interfaces (identity realpath,
//     panicking writes). A `SubFs` impl that supports them just overrides
//     the methods — dynamic dispatch replaces Go's type assertion.

use std::io;
use std::ops::{BitAnd, BitOr, BitOrAssign, Not};
use std::sync::Arc;
use std::time::SystemTime;

use crate::vfs::FileInfo;

/// FileMode mirrors Go's `fs.FileMode`: a bit set describing a file's kind
/// (high bits) and permission bits (low 9 bits).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct FileMode(u32);

impl FileMode {
    pub const EMPTY: FileMode = FileMode(0);
    /// d: is a directory. Go fs.ModeDir.
    pub const DIR: FileMode = FileMode(1 << 31);
    /// T: temporary file. Go fs.ModeTemporary.
    pub const TEMPORARY: FileMode = FileMode(1 << 30);
    /// L: symbolic link. Go fs.ModeSymlink.
    pub const SYMLINK: FileMode = FileMode(1 << 29);
    /// D: device file. Go fs.ModeDevice.
    pub const DEVICE: FileMode = FileMode(1 << 28);
    /// p: named pipe (FIFO). Go fs.ModeNamedPipe.
    pub const NAMED_PIPE: FileMode = FileMode(1 << 27);
    /// S: Unix domain socket. Go fs.ModeSocket.
    pub const SOCKET: FileMode = FileMode(1 << 26);
    /// u: setuid. Go fs.ModeSetuid.
    pub const SETUID: FileMode = FileMode(1 << 25);
    /// g: setgid. Go fs.ModeSetgid.
    pub const SETGID: FileMode = FileMode(1 << 24);
    /// c: Unix character device. Go fs.ModeCharDevice.
    pub const CHAR_DEVICE: FileMode = FileMode(1 << 23);
    /// t: sticky. Go fs.ModeSticky.
    pub const STICKY: FileMode = FileMode(1 << 22);
    /// ?: non-regular file; nothing else is known. Go fs.ModeIrregular.
    pub const IRREGULAR: FileMode = FileMode(1 << 21);
    /// Go fs.ModeType: the type bits (Dir, Symlink, NamedPipe, Socket, Device,
    /// CharDevice, Irregular).
    pub const MODE_TYPE: FileMode =
        FileMode(Self::DIR.0 | Self::SYMLINK.0 | Self::NAMED_PIPE.0 | Self::SOCKET.0
            | Self::DEVICE.0 | Self::CHAR_DEVICE.0 | Self::IRREGULAR.0);
    /// Go fs.ModePerm: the Unix permission bits.
    pub const MODE_PERM: FileMode = FileMode(0o777);

    pub fn from_bits(bits: u32) -> FileMode {
        FileMode(bits)
    }

    pub fn bits(self) -> u32 {
        self.0
    }

    /// IsDir reports whether the mode describes a directory (Go FileMode.IsDir).
    pub fn is_dir(self) -> bool {
        self & Self::DIR != Self::EMPTY
    }

    /// IsRegular reports whether the mode describes a regular file (Go
    /// FileMode.IsRegular): no type bits are set.
    pub fn is_regular(self) -> bool {
        self & Self::MODE_TYPE == Self::EMPTY
    }

    /// Type returns the type bits (Go FileMode.Type).
    pub fn type_bits(self) -> FileMode {
        self & Self::MODE_TYPE
    }

    /// Perm returns the permission bits (Go FileMode.Perm).
    pub fn perm(self) -> FileMode {
        self & Self::MODE_PERM
    }
}

impl BitOr for FileMode {
    type Output = FileMode;
    fn bitor(self, rhs: FileMode) -> FileMode {
        FileMode(self.0 | rhs.0)
    }
}

impl BitOrAssign for FileMode {
    fn bitor_assign(&mut self, rhs: FileMode) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for FileMode {
    type Output = FileMode;
    fn bitand(self, rhs: FileMode) -> FileMode {
        FileMode(self.0 & rhs.0)
    }
}

impl Not for FileMode {
    type Output = FileMode;
    fn not(self) -> FileMode {
        FileMode(!self.0)
    }
}

/// SubDirEntry carries what Go's `fs.DirEntry` contributes when the port reads
/// a directory through `SubFs::read_dir`: the entry name and its type bits.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SubDirEntry {
    pub name: String,
    /// Go DirEntry.Type(): type bits only (DIR, SYMLINK, or 0/irregular).
    pub kind: FileMode,
}

/// SubFs is the fs.FS role the vfs layer is built on, plus the RealpathFS and
/// WritableFS capability interfaces from iovfs (see the module PORT note).
pub trait SubFs: Send + Sync {
    /// Go fs.Stat(fsys, name). "." is the sub-root itself.
    fn stat(&self, name: &str) -> io::Result<Arc<dyn FileInfo>>;

    /// Go fs.ReadFile(fsys, name).
    fn read_file(&self, name: &str) -> io::Result<Vec<u8>>;

    /// Go fs.ReadDir(fsys, name): entries sorted by filename.
    fn read_dir(&self, name: &str) -> io::Result<Vec<SubDirEntry>>;

    /// iovfs.RealpathFS.Realpath. The default is the identity, matching
    /// iovfs.From wrapping a non-RealpathFS.
    fn realpath(&self, name: &str) -> io::Result<String> {
        Ok(name.to_string())
    }

    /// iovfs.WritableFS.WriteFile. The default panics, matching iovfs.From's
    /// "writeFile not supported" closure for a non-WritableFS.
    fn write_file(&self, _name: &str, _data: &str) -> io::Result<()> {
        panic!("writeFile not supported");
    }

    /// iovfs.WritableFS.AppendFile. See `write_file`.
    fn append_file(&self, _name: &str, _data: &str) -> io::Result<()> {
        panic!("appendFile not supported");
    }

    /// iovfs.WritableFS.MkdirAll (perm is fixed 0o777 by the only Go caller).
    fn mkdir_all(&self, _name: &str) -> io::Result<()> {
        panic!("mkdirAll not supported");
    }

    /// iovfs.WritableFS.Remove.
    fn remove(&self, _name: &str) -> io::Result<()> {
        panic!("remove not supported");
    }

    /// iovfs.WritableFS.Chtimes.
    fn chtimes(&self, _name: &str, _atime: SystemTime, _mtime: SystemTime) -> io::Result<()> {
        panic!("chtimes not supported");
    }
}

/// PrefixSubFs mirrors Go fs.Sub: a sub-FS that prefixes every name with
/// `prefix` ("." maps to `prefix` itself).
pub struct PrefixSubFs {
    inner: Arc<dyn SubFs>,
    prefix: String,
}

impl PrefixSubFs {
    pub fn new(inner: Arc<dyn SubFs>, prefix: &str) -> PrefixSubFs {
        PrefixSubFs { inner, prefix: prefix.to_string() }
    }

    fn join(&self, name: &str) -> String {
        if name == "." {
            return self.prefix.clone();
        }
        format!("{}/{}", self.prefix, name)
    }
}

impl SubFs for PrefixSubFs {
    fn stat(&self, name: &str) -> io::Result<Arc<dyn FileInfo>> {
        self.inner.stat(&self.join(name))
    }

    fn read_file(&self, name: &str) -> io::Result<Vec<u8>> {
        self.inner.read_file(&self.join(name))
    }

    fn read_dir(&self, name: &str) -> io::Result<Vec<SubDirEntry>> {
        self.inner.read_dir(&self.join(name))
    }

    fn realpath(&self, name: &str) -> io::Result<String> {
        self.inner.realpath(&self.join(name))
    }

    fn write_file(&self, name: &str, data: &str) -> io::Result<()> {
        self.inner.write_file(&self.join(name), data)
    }

    fn append_file(&self, name: &str, data: &str) -> io::Result<()> {
        self.inner.append_file(&self.join(name), data)
    }

    fn mkdir_all(&self, name: &str) -> io::Result<()> {
        self.inner.mkdir_all(&self.join(name))
    }

    fn remove(&self, name: &str) -> io::Result<()> {
        self.inner.remove(&self.join(name))
    }

    fn chtimes(&self, name: &str, atime: SystemTime, mtime: SystemTime) -> io::Result<()> {
        self.inner.chtimes(&self.join(name), atime, mtime)
    }
}

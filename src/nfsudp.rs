//! In-core NFS server (NFSv2 + NFSv3) over UDP, dispatched by the NAT engine.
//!
//! The whole protocol stays inside the NAT: `src/net.rs` hands guest MOUNT/NFS
//! RPC datagrams to this module and injects the reply bytes back as
//! virtual-network frames — there are **no host network sockets**. The only host
//! interaction is file I/O against the user-chosen backing folder.
//!
//! Plan + open questions: `docs/nfsudp-plan.md`. Wire structs/semantics are
//! modelled on `nfsserve` (BSD-3-Clause), re-implemented synchronously here.
//!
//! This file is built bottom-up. **Increment 1 (this commit): the
//! version-agnostic backend** — the `fileid`↔path map, path containment, the
//! faked/synthetic unix attributes, and the filesystem operations. The XDR/RPC
//! layer, the v2/v3 procedure encoders, MOUNT, the NAT wiring, and the GUI land
//! on top of this.

#![allow(dead_code)] // wire layer that consumes the backend lands in later increments

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The reserved NFS `fileid` of the export root.
pub const ROOT_ID: u64 = 1;

/// File kind, mapped to NFS `ftype3` (and the v2 equivalent) by the wire layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Reg,
    Dir,
    Lnk,
    Other,
}

/// Synthetic POSIX attributes for one object. "Faked" deliberately: uid/gid are
/// fixed and the mode is a heuristic, so the export behaves identically on
/// Linux, macOS, and Windows (which has no unix uid/gid/mode at all).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attr {
    pub kind: FileKind,
    /// Full mode incl. type bits (e.g. `0o040755` for a dir) — convenience for
    /// the wire layer; the type bits come from `kind`.
    pub mode: u32,
    pub nlink: u32,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub fileid: u64,
    /// (seconds, nanoseconds) since the unix epoch.
    pub atime: (u32, u32),
    pub mtime: (u32, u32),
    pub ctime: (u32, u32),
}

/// POSIX `S_IF*` type bits, for the `mode` field.
const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;
const S_IFLNK: u32 = 0o120000;

/// The backing store: one exported directory, plus a stable `fileid`↔relative
/// path map so the guest's opaque file handles resolve back to host paths. Used
/// only from the NAT thread, so plain `&mut self` (no locking).
pub struct NfsBacking {
    /// Absolute path of the exported folder. All access is contained within it.
    root: PathBuf,
    next_id: u64,
    /// fileid → path relative to `root` (root itself = ROOT_ID = empty path).
    id_to_path: HashMap<u64, PathBuf>,
    /// reverse map so re-looking-up a path returns its existing id.
    path_to_id: HashMap<PathBuf, u64>,
    /// Default owner for faked attributes.
    uid: u32,
    gid: u32,
}

impl NfsBacking {
    /// Create a backing store over `root`. `root` should be an existing,
    /// absolute directory; access is confined to it.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let mut id_to_path = HashMap::new();
        let mut path_to_id = HashMap::new();
        id_to_path.insert(ROOT_ID, PathBuf::new());
        path_to_id.insert(PathBuf::new(), ROOT_ID);
        Self {
            root: root.into(),
            next_id: ROOT_ID + 1,
            id_to_path,
            path_to_id,
            uid: 0,
            gid: 0,
        }
    }

    /// Intern a root-relative path, returning a stable fileid for it.
    fn intern(&mut self, rel: PathBuf) -> u64 {
        if let Some(&id) = self.path_to_id.get(&rel) {
            return id;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.id_to_path.insert(id, rel.clone());
        self.path_to_id.insert(rel, id);
        id
    }

    /// The root-relative path for a fileid, if known.
    fn rel_of(&self, id: u64) -> Option<&PathBuf> {
        self.id_to_path.get(&id)
    }

    /// The absolute host path for a fileid. Guaranteed within `root` because the
    /// relative paths only ever contain validated, normal components.
    pub fn abs_of(&self, id: u64) -> Option<PathBuf> {
        self.rel_of(id).map(|rel| self.root.join(rel))
    }

    /// Whether `id` is a directory the guest can list.
    pub fn is_dir(&self, id: u64) -> bool {
        self.abs_of(id).map(|p| p.is_dir()).unwrap_or(false)
    }

    /// Synthetic attributes for `id`, or `None` if it no longer exists.
    pub fn attr(&self, id: u64) -> Option<Attr> {
        let abs = self.abs_of(id)?;
        let md = std::fs::symlink_metadata(&abs).ok()?;
        Some(self.attr_from(id, &md))
    }

    /// Resolve `name` within directory `dirid`, interning the child and
    /// returning its fileid. Rejects names that could escape the export.
    pub fn lookup(&mut self, dirid: u64, name: &[u8]) -> Option<u64> {
        let comp = valid_component(name)?;
        let rel = self.rel_of(dirid)?.join(&comp);
        let abs = self.root.join(&rel);
        if !abs.symlink_metadata().is_ok() {
            return None;
        }
        Some(self.intern(rel))
    }

    /// List `dirid`, returning `(name, fileid, attr)` for each entry. `.` / `..`
    /// are not included (the wire layer synthesizes them if a client needs them).
    pub fn readdir(&mut self, dirid: u64) -> Option<Vec<(Vec<u8>, u64, Attr)>> {
        let dir_rel = self.rel_of(dirid)?.clone();
        let abs = self.root.join(&dir_rel);
        let mut out = Vec::new();
        for ent in std::fs::read_dir(&abs).ok()? {
            let ent = ent.ok()?;
            let name = name_bytes(&ent.file_name());
            // Skip anything that wouldn't round-trip as a safe component.
            if valid_component(&name).is_none() {
                continue;
            }
            let rel = dir_rel.join(ent.file_name());
            let id = self.intern(rel);
            if let Some(attr) = self.attr(id) {
                out.push((name, id, attr));
            }
        }
        Some(out)
    }

    /// Read up to `count` bytes at `offset` from file `id`. Returns the data and
    /// whether end-of-file was reached.
    pub fn read(&self, id: u64, offset: u64, count: u32) -> Option<(Vec<u8>, bool)> {
        use std::io::{Read, Seek, SeekFrom};
        let abs = self.abs_of(id)?;
        let mut f = std::fs::File::open(&abs).ok()?;
        let len = f.metadata().ok()?.len();
        f.seek(SeekFrom::Start(offset)).ok()?;
        let want = count as usize;
        let mut buf = vec![0u8; want];
        let mut got = 0;
        while got < want {
            match f.read(&mut buf[got..]) {
                Ok(0) => break,
                Ok(n) => got += n,
                Err(_) => break,
            }
        }
        buf.truncate(got);
        let eof = offset.saturating_add(got as u64) >= len;
        Some((buf, eof))
    }

    /// Write `data` at `offset` to file `id`, returning the post-write attrs.
    pub fn write(&mut self, id: u64, offset: u64, data: &[u8]) -> Option<Attr> {
        use std::io::{Seek, SeekFrom, Write};
        let abs = self.abs_of(id)?;
        let mut f = std::fs::OpenOptions::new().write(true).open(&abs).ok()?;
        f.seek(SeekFrom::Start(offset)).ok()?;
        f.write_all(data).ok()?;
        f.flush().ok()?;
        self.attr(id)
    }

    /// Create an empty regular file `name` in `dirid`; returns its fileid.
    pub fn create(&mut self, dirid: u64, name: &[u8]) -> Option<u64> {
        let comp = valid_component(name)?;
        let rel = self.rel_of(dirid)?.join(&comp);
        let abs = self.root.join(&rel);
        std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(&abs).ok()?;
        Some(self.intern(rel))
    }

    /// Create directory `name` in `dirid`; returns its fileid.
    pub fn mkdir(&mut self, dirid: u64, name: &[u8]) -> Option<u64> {
        let comp = valid_component(name)?;
        let rel = self.rel_of(dirid)?.join(&comp);
        std::fs::create_dir(self.root.join(&rel)).ok()?;
        Some(self.intern(rel))
    }

    /// Remove file `name` from `dirid`.
    pub fn remove(&mut self, dirid: u64, name: &[u8]) -> bool {
        self.remove_with(dirid, name, false)
    }

    /// Remove directory `name` from `dirid`.
    pub fn rmdir(&mut self, dirid: u64, name: &[u8]) -> bool {
        self.remove_with(dirid, name, true)
    }

    fn remove_with(&mut self, dirid: u64, name: &[u8], dir: bool) -> bool {
        let Some(comp) = valid_component(name) else { return false };
        let Some(parent) = self.rel_of(dirid) else { return false };
        let rel = parent.join(&comp);
        let abs = self.root.join(&rel);
        let ok = if dir { std::fs::remove_dir(&abs) } else { std::fs::remove_file(&abs) }.is_ok();
        if ok {
            if let Some(id) = self.path_to_id.remove(&rel) {
                self.id_to_path.remove(&id);
            }
        }
        ok
    }

    /// Rename `from_name` in `from_dir` to `to_name` in `to_dir`.
    pub fn rename(&mut self, from_dir: u64, from_name: &[u8], to_dir: u64, to_name: &[u8]) -> bool {
        let (Some(fc), Some(tc)) = (valid_component(from_name), valid_component(to_name)) else {
            return false;
        };
        let (Some(fp), Some(tp)) = (self.rel_of(from_dir).cloned(), self.rel_of(to_dir).cloned()) else {
            return false;
        };
        let from_rel = fp.join(&fc);
        let to_rel = tp.join(&tc);
        if std::fs::rename(self.root.join(&from_rel), self.root.join(&to_rel)).is_err() {
            return false;
        }
        // Re-point the moved id at its new path so its handle stays valid.
        if let Some(id) = self.path_to_id.remove(&from_rel) {
            self.id_to_path.insert(id, to_rel.clone());
            self.path_to_id.insert(to_rel, id);
        }
        true
    }

    // ── synthetic attribute construction ────────────────────────────────────

    fn attr_from(&self, id: u64, md: &std::fs::Metadata) -> Attr {
        let ft = md.file_type();
        let (kind, type_bits, base) = if ft.is_dir() {
            (FileKind::Dir, S_IFDIR, 0o755)
        } else if ft.is_symlink() {
            (FileKind::Lnk, S_IFLNK, 0o777)
        } else if ft.is_file() {
            (FileKind::Reg, S_IFREG, file_perm(md))
        } else {
            (FileKind::Other, S_IFREG, 0o644)
        };
        Attr {
            kind,
            mode: type_bits | base,
            nlink: if kind == FileKind::Dir { 2 } else { 1 },
            uid: self.uid,
            gid: self.gid,
            size: md.len(),
            fileid: id,
            atime: systime(md.accessed().ok()),
            mtime: systime(md.modified().ok()),
            ctime: systime(md.modified().ok()), // no portable ctime; reuse mtime
        }
    }
}

/// Permission bits for a regular file: 0644, plus the execute bits if the host
/// marks it executable (unix only; on other platforms files are 0644).
fn file_perm(md: &std::fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if md.permissions().mode() & 0o111 != 0 {
            return 0o755;
        }
    }
    let _ = md;
    0o644
}

/// Convert a `SystemTime` into `(secs, nsecs)` since the unix epoch (0 if before
/// the epoch or unavailable).
fn systime(t: Option<SystemTime>) -> (u32, u32) {
    t.and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| (d.as_secs() as u32, d.subsec_nanos()))
        .unwrap_or((0, 0))
}

/// Validate a single path component coming off the wire. Rejects anything that
/// could escape the export (`..`, `.`, empty, embedded separators or NUL).
/// Returns the component as an `OsString`-bearing `PathBuf` fragment.
fn valid_component(name: &[u8]) -> Option<PathBuf> {
    if name.is_empty() || name == b"." || name == b".." {
        return None;
    }
    if name.iter().any(|&b| b == b'/' || b == b'\\' || b == 0) {
        return None;
    }
    let s = os_string_from_bytes(name)?;
    let pb = PathBuf::from(&s);
    // Defense in depth: the parsed fragment must be exactly one normal component.
    let mut comps = pb.components();
    match (comps.next(), comps.next()) {
        (Some(std::path::Component::Normal(_)), None) => Some(pb),
        _ => None,
    }
}

/// Bytes → OS string. On unix, filenames are arbitrary bytes; on other targets
/// they must be valid UTF-8 (a documented cross-platform limitation).
fn os_string_from_bytes(b: &[u8]) -> Option<std::ffi::OsString> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        return Some(std::ffi::OsStr::from_bytes(b).to_os_string());
    }
    #[cfg(not(unix))]
    {
        std::str::from_utf8(b).ok().map(std::ffi::OsString::from)
    }
}

/// An OS filename → bytes (inverse of `os_string_from_bytes`).
fn name_bytes(name: &std::ffi::OsStr) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        return name.as_bytes().to_vec();
    }
    #[cfg(not(unix))]
    {
        name.to_string_lossy().into_owned().into_bytes()
    }
}

/// Path containment check used by tests and as a sanity assert.
pub fn within(root: &Path, candidate: &Path) -> bool {
    candidate.starts_with(root)
}

// ── XDR (RFC 1014) + Sun RPC (RFC 1057) wire layer ──────────────────────────
//
// Increment 2: the transport-agnostic encode/decode used by both NFSv2 and v3.
// One UDP datagram carries exactly one RPC message (no TCP record marking).

const MSG_CALL: u32 = 0;
const MSG_REPLY: u32 = 1;
const RPC_VERSION: u32 = 2;
const REPLY_ACCEPTED: u32 = 0;
const AUTH_NULL: u32 = 0;

/// RPC accept-status values (RFC 1057). We allow every host, so auth never
/// fails; these cover protocol-level outcomes only.
pub mod accept {
    pub const SUCCESS: u32 = 0;
    pub const PROG_UNAVAIL: u32 = 1;
    pub const PROG_MISMATCH: u32 = 2;
    pub const PROC_UNAVAIL: u32 = 3;
    pub const GARBAGE_ARGS: u32 = 4;
    pub const SYSTEM_ERR: u32 = 5;
}

/// Big-endian, 4-byte-aligned XDR encoder.
#[derive(Default)]
pub struct Xdr {
    buf: Vec<u8>,
}
impl Xdr {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }
    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }
    pub fn i32(&mut self, v: i32) {
        self.u32(v as u32);
    }
    pub fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }
    pub fn bool(&mut self, v: bool) {
        self.u32(v as u32);
    }
    /// Length-prefixed opaque/string, padded to a 4-byte boundary.
    pub fn opaque(&mut self, b: &[u8]) {
        self.u32(b.len() as u32);
        self.fixed(b);
    }
    /// Fixed-length bytes (no length prefix), padded to 4 bytes.
    pub fn fixed(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
        let pad = (4 - (b.len() % 4)) % 4;
        self.buf.extend(std::iter::repeat(0u8).take(pad));
    }
    pub fn len(&self) -> usize {
        self.buf.len()
    }
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }
}

/// Big-endian XDR decode cursor over a borrowed datagram.
pub struct Cur<'a> {
    b: &'a [u8],
    pos: usize,
}
impl<'a> Cur<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Self { b, pos: 0 }
    }
    pub fn u32(&mut self) -> Option<u32> {
        let e = self.pos.checked_add(4)?;
        if e > self.b.len() {
            return None;
        }
        let v = u32::from_be_bytes(self.b[self.pos..e].try_into().ok()?);
        self.pos = e;
        Some(v)
    }
    pub fn u64(&mut self) -> Option<u64> {
        let e = self.pos.checked_add(8)?;
        if e > self.b.len() {
            return None;
        }
        let v = u64::from_be_bytes(self.b[self.pos..e].try_into().ok()?);
        self.pos = e;
        Some(v)
    }
    pub fn i32(&mut self) -> Option<i32> {
        self.u32().map(|v| v as i32)
    }
    /// Length-prefixed opaque/string (returns the bytes; consumes the pad).
    pub fn opaque(&mut self) -> Option<&'a [u8]> {
        let len = self.u32()? as usize;
        self.fixed(len)
    }
    /// Fixed-length opaque of `len` bytes, consuming the pad to a 4-byte boundary.
    pub fn fixed(&mut self, len: usize) -> Option<&'a [u8]> {
        let e = self.pos.checked_add(len)?;
        if e > self.b.len() {
            return None;
        }
        let s = &self.b[self.pos..e];
        let pad = (4 - (len % 4)) % 4;
        self.pos = (e + pad).min(self.b.len()); // tolerate a missing trailing pad
        Some(s)
    }
    pub fn skip(&mut self, n: usize) -> Option<()> {
        let e = self.pos.checked_add(n)?;
        if e > self.b.len() {
            return None;
        }
        self.pos = e;
        Some(())
    }
    /// Skip one RPC opaque_auth (`flavor` + length-prefixed `body`).
    fn skip_auth(&mut self) -> Option<()> {
        let _flavor = self.u32()?;
        self.opaque()?;
        Some(())
    }
    pub fn remaining(&self) -> &'a [u8] {
        &self.b[self.pos.min(self.b.len())..]
    }
}

/// A parsed RPC CALL header. Credentials are skipped — every host is allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RpcCall {
    pub xid: u32,
    pub prog: u32,
    pub vers: u32,
    pub proc_num: u32,
}

/// Parse the RPC CALL header from one UDP datagram, returning the header and a
/// cursor positioned at the procedure arguments. `None` if it isn't a v2 CALL.
pub fn parse_call(msg: &[u8]) -> Option<(RpcCall, Cur)> {
    let mut c = Cur::new(msg);
    let xid = c.u32()?;
    if c.u32()? != MSG_CALL {
        return None;
    }
    if c.u32()? != RPC_VERSION {
        return None;
    }
    let prog = c.u32()?;
    let vers = c.u32()?;
    let proc_num = c.u32()?;
    c.skip_auth()?; // cred
    c.skip_auth()?; // verf
    Some((RpcCall { xid, prog, vers, proc_num }, c))
}

/// Begin an accepted RPC reply (AUTH_NULL verifier), ready for the caller to
/// append the procedure result. `accept_stat` is one of [`accept`].
pub fn reply(xid: u32, accept_stat: u32) -> Xdr {
    let mut x = Xdr::new();
    x.u32(xid);
    x.u32(MSG_REPLY);
    x.u32(REPLY_ACCEPTED);
    x.u32(AUTH_NULL); // verifier flavor
    x.u32(0); // verifier body length
    x.u32(accept_stat);
    x
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Unique temp export dir (no external tempfile crate).
    fn temp_export() -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "iris_nfs_test_{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn root_is_a_dir_with_id_1() {
        let root = temp_export();
        let b = NfsBacking::new(&root);
        assert_eq!(b.abs_of(ROOT_ID).unwrap(), root);
        assert!(b.is_dir(ROOT_ID));
        let a = b.attr(ROOT_ID).unwrap();
        assert_eq!(a.kind, FileKind::Dir);
        assert_eq!(a.fileid, ROOT_ID);
        assert_eq!(a.uid, 0);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn lookup_intern_and_attrs() {
        let root = temp_export();
        std::fs::write(root.join("hello.txt"), b"hi there").unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        let mut b = NfsBacking::new(&root);

        let fid = b.lookup(ROOT_ID, b"hello.txt").unwrap();
        assert_eq!(b.lookup(ROOT_ID, b"hello.txt").unwrap(), fid, "stable id");
        let a = b.attr(fid).unwrap();
        assert_eq!(a.kind, FileKind::Reg);
        assert_eq!(a.size, 8);
        assert_eq!(a.mode & 0o170000, S_IFREG);

        let did = b.lookup(ROOT_ID, b"sub").unwrap();
        assert!(b.is_dir(did));
        assert!(b.lookup(ROOT_ID, b"nope").is_none());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn read_write_roundtrip() {
        let root = temp_export();
        let mut b = NfsBacking::new(&root);
        let fid = b.create(ROOT_ID, b"f.bin").unwrap();
        let attr = b.write(fid, 0, b"abcdefgh").unwrap();
        assert_eq!(attr.size, 8);
        let (data, eof) = b.read(fid, 2, 3).unwrap();
        assert_eq!(data, b"cde");
        assert!(!eof);
        let (rest, eof) = b.read(fid, 5, 100).unwrap();
        assert_eq!(rest, b"fgh");
        assert!(eof);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn readdir_lists_children() {
        let root = temp_export();
        std::fs::write(root.join("a"), b"1").unwrap();
        std::fs::write(root.join("b"), b"22").unwrap();
        std::fs::create_dir(root.join("d")).unwrap();
        let mut b = NfsBacking::new(&root);
        let mut names: Vec<Vec<u8>> = b.readdir(ROOT_ID).unwrap().into_iter().map(|(n, _, _)| n).collect();
        names.sort();
        assert_eq!(names, vec![b"a".to_vec(), b"b".to_vec(), b"d".to_vec()]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn mkdir_remove_rename() {
        let root = temp_export();
        let mut b = NfsBacking::new(&root);
        let d = b.mkdir(ROOT_ID, b"dir").unwrap();
        assert!(b.is_dir(d));
        let f = b.create(d, b"x").unwrap();
        assert!(b.attr(f).is_some());
        assert!(b.rename(d, b"x", ROOT_ID, b"y"));
        assert!(root.join("y").exists());
        assert!(!root.join("dir/x").exists());
        assert!(b.remove(ROOT_ID, b"y"));
        assert!(b.rmdir(ROOT_ID, b"dir"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rejects_traversal_components() {
        assert!(valid_component(b"..").is_none());
        assert!(valid_component(b".").is_none());
        assert!(valid_component(b"").is_none());
        assert!(valid_component(b"a/b").is_none());
        assert!(valid_component(b"a\\b").is_none());
        assert!(valid_component(b"ok.txt").is_some());

        // lookup must refuse to escape the export root.
        let root = temp_export();
        let mut b = NfsBacking::new(&root);
        assert!(b.lookup(ROOT_ID, b"..").is_none());
        assert!(b.lookup(ROOT_ID, b"../etc").is_none());
        std::fs::remove_dir_all(&root).ok();
    }

    // ── wire layer (increment 2) ────────────────────────────────────────────

    #[test]
    fn xdr_roundtrip() {
        let mut x = Xdr::new();
        x.u32(0xdead_beef);
        x.u64(0x0102_0304_0506_0708);
        x.opaque(b"abc"); // 4 (len) + 3 + 1 pad = 8
        let bytes = x.into_bytes();
        assert_eq!(bytes.len(), 4 + 8 + 8);
        let mut c = Cur::new(&bytes);
        assert_eq!(c.u32(), Some(0xdead_beef));
        assert_eq!(c.u64(), Some(0x0102_0304_0506_0708));
        assert_eq!(c.opaque(), Some(&b"abc"[..]));
        assert_eq!(c.u32(), None); // exhausted
    }

    #[test]
    fn parse_rpc_call_skips_auth() {
        let mut x = Xdr::new();
        x.u32(0x1122_3344); // xid
        x.u32(0); // CALL
        x.u32(2); // rpcvers
        x.u32(100003); // NFS program
        x.u32(3); // v3
        x.u32(1); // GETATTR
        x.u32(0); x.u32(0); // cred: AUTH_NULL, len 0
        x.u32(0); x.u32(0); // verf: AUTH_NULL, len 0
        x.u32(0xCAFE_BABE); // one arg word
        let msg = x.into_bytes();
        let (call, mut args) = parse_call(&msg).unwrap();
        assert_eq!(call, RpcCall { xid: 0x1122_3344, prog: 100003, vers: 3, proc_num: 1 });
        assert_eq!(args.u32(), Some(0xCAFE_BABE), "cursor lands on the args");
    }

    #[test]
    fn parse_rejects_non_call_and_bad_version() {
        let mut reply = Xdr::new();
        reply.u32(1); reply.u32(1); // xid, REPLY (not CALL)
        assert!(parse_call(&reply.into_bytes()).is_none());

        let mut badver = Xdr::new();
        badver.u32(1); badver.u32(0); badver.u32(3); // CALL but rpcvers=3
        assert!(parse_call(&badver.into_bytes()).is_none());
    }

    #[test]
    fn reply_header_bytes() {
        let bytes = reply(0x1122_3344, accept::SUCCESS).into_bytes();
        let mut c = Cur::new(&bytes);
        assert_eq!(c.u32(), Some(0x1122_3344)); // xid
        assert_eq!(c.u32(), Some(1)); // REPLY
        assert_eq!(c.u32(), Some(0)); // MSG_ACCEPTED
        assert_eq!(c.u32(), Some(0)); // verf flavor AUTH_NULL
        assert_eq!(c.u32(), Some(0)); // verf len
        assert_eq!(c.u32(), Some(0)); // accept_stat SUCCESS
    }
}

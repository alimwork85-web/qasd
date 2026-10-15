//! Parallel whole-disk enumeration on Windows.
//!
//! `FindFirstFileExW` with `FindExInfoBasic` and `FIND_FIRST_EX_LARGE_FETCH`
//! returns a folder's entries in big batches with name, attributes, size and
//! mtime attached, so there is no per-file stat (the role getattrlistbulk
//! plays in fsearch on macOS). Folders fan out over a rayon pool. Symbolic
//! links, junctions and volume mount points are entries, never followed.
//!
//! The tree's root `/` is synthetic: its children are the fixed drives
//! (`sys::drives`), so `C:\x` is `/C:/x` inside.

use crate::sys;
use rayon::Scope;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, GetLastError, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    FIND_FIRST_EX_LARGE_FETCH, FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW, FindNextFileW, WIN32_FIND_DATAW,
};

pub const NONE: u32 = u32::MAX;

/// Folders never to open (from `Options::skip`).
pub static SKIP: std::sync::OnceLock<Vec<Vec<u8>>> = std::sync::OnceLock::new();

/// Folders a scan was refused (access denied). Recorded only while paths
/// are tracked, i.e. while SKIP is set.
pub static DENIED: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());

/// Entries the first run's whole-disk crawl has listed so far.
pub static LISTED: AtomicUsize = AtomicUsize::new(0);

/// Work a search is waiting on (listing or reading one folder): while any
/// runs, background work (the whole-disk crawl, content builds) waits in
/// `yield_to_searches`, so the search gets the cores and the disk to itself.
static HOLD: AtomicUsize = AtomicUsize::new(0);

/// Holds background work off while it lives (see `HOLD`).
pub struct Hold(());

impl Hold {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Hold {
        HOLD.fetch_add(1, Ordering::Relaxed);
        Hold(())
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        HOLD.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Background work's turn: wait while a `Hold` lives, but at most 2 s, so
/// searches back to back never stall it for good.
pub fn yield_to_searches() {
    let t = std::time::Instant::now();
    while HOLD.load(Ordering::Relaxed) > 0 && t.elapsed() < std::time::Duration::from_secs(2) {
        std::thread::sleep(std::time::Duration::from_micros(500));
    }
}

pub fn blocked(path: &[u8]) -> bool {
    SKIP.get().is_some_and(|v| v.iter().any(|s| path.starts_with(s) && (path.len() == s.len() || path[s.len()] == b'/')))
}

pub const KIND_FILE: u8 = 0;
pub const KIND_DIR: u8 = 1;
pub const KIND_LINK: u8 = 2;
pub const KIND_OTHER: u8 = 3;

/// Entry flag bit: FILE_ATTRIBUTE_HIDDEN.
pub const FLAG_HIDDEN: u8 = 1 << 2;
/// Directory that is a mount point we did not descend into.
pub const FLAG_MOUNT: u8 = 1 << 3;

const ATTR_HIDDEN: u32 = 0x2;
const ATTR_DIRECTORY: u32 = 0x10;
const ATTR_DEVICE: u32 = 0x40;
const ATTR_REPARSE: u32 = 0x400;
const TAG_MOUNT_POINT: u32 = 0xA000_0003;
const TAG_SYMLINK: u32 = 0xA000_000C;
const TAG_APPEXECLINK: u32 = 0x8000_001B;

#[derive(Clone, Copy)]
pub struct RawEnt {
    pub name_off: u32,
    pub name_len: u16,
    /// kind in the low 2 bits, FLAG_* above.
    pub kind: u8,
    pub size: u64,
    pub mtime: u32,
    /// Temp id of the listing for this directory, or NONE.
    pub child: u32,
}

#[derive(Default)]
pub struct Listing {
    pub id: u32,
    pub names: Vec<u8>,
    pub ents: Vec<RawEnt>,
}

struct Ctx {
    next_id: AtomicU32,
    out: Vec<Mutex<Vec<Listing>>>,
    /// The whole-disk crawl: counts LISTED, yields to searches.
    disk: bool,
}

/// Scan `root` recursively. Listing id 0 is `root` itself. A whole drive
/// (one plugged in while running) comes from its MFT when we may.
pub fn scan(root: &[u8], threads: usize) -> Vec<Listing> {
    let r = crate::live::normalize(root);
    if r.len() == 3 && r[2] == b':' && crate::sys::elevated() && std::env::var_os("QASD_NO_MFT").is_none() {
        let t = std::time::Instant::now();
        if let Some(ls) = crate::mft::scan_volume(&r) {
            let n: usize = ls.iter().map(|l| l.ents.len()).sum();
            eprintln!("{} {} listed {n} entries by MFT in {:.2?}", crate::query::now_secs(), sys::display(&r), t.elapsed());
            return ls;
        }
    }
    scan_with(root, threads, false)
}

/// The first run's listing of every drive (see `LISTED`, `HOLD`): each NTFS
/// drive read straight from its master file table when we may (`mft`,
/// administrator), else crawled folder by folder.
pub fn scan_disk(threads: usize) -> Vec<Listing> {
    if !crate::sys::elevated() || std::env::var_os("QASD_NO_MFT").is_some() {
        return scan_with(b"/", threads, true);
    }
    let mut root = Listing::default();
    let mut out = Vec::new();
    let mut next = 1u32;
    for d in sys::drives() {
        let t = std::time::Instant::now();
        let (part, how) = match crate::mft::scan_volume(&d) {
            Some(p) => (p, "MFT"),
            None => (scan_with(&d, threads, true), "crawl"),
        };
        let n: usize = part.iter().map(|l| l.ents.len()).sum();
        if how == "MFT" {
            LISTED.fetch_add(n, Ordering::Relaxed);
        }
        eprintln!("{} {} listed {n} entries by {how} in {:.2?}", crate::query::now_secs(), sys::display(&d), t.elapsed());
        // The drive's listings, renumbered after those so far; its root
        // (listing 0) becomes the root's entry for the drive.
        let base = next;
        next += part.len() as u32;
        add(&mut root, &d[1..], KIND_DIR, 0, 0);
        root.ents.last_mut().unwrap().child = base;
        for mut l in part {
            l.id += base;
            for e in l.ents.iter_mut().filter(|e| e.child != NONE) {
                e.child += base;
            }
            out.push(l);
        }
    }
    out.push(root);
    out
}

fn scan_with(root: &[u8], threads: usize, disk: bool) -> Vec<Listing> {
    let root = crate::live::normalize(root);
    let mut l = Listing::default();
    let ok = !blocked(&root) && list_into(&root, &mut l);
    if disk {
        yield_to_searches();
    }
    // A folder holding no folder (most new ones) needs no pool.
    if !ok || !l.ents.iter().any(|e| e.kind & 3 == KIND_DIR && e.kind & FLAG_MOUNT == 0) {
        if disk {
            LISTED.fetch_add(l.ents.len(), Ordering::Relaxed);
        }
        return vec![l];
    }
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).start_handler(|_| crate::no_materialize()).build().unwrap();
    let ctx = Ctx { next_id: AtomicU32::new(1), out: (0..threads + 1).map(|_| Mutex::new(Vec::new())).collect(), disk };
    pool.scope(|s| descend(s, root, l, &ctx));
    let parts: Vec<Vec<Listing>> = ctx.out.into_iter().map(|m| m.into_inner().unwrap()).collect();
    let mut out = Vec::with_capacity(parts.iter().map(Vec::len).sum());
    for p in parts {
        out.extend(p);
    }
    out
}

/// List a single directory (no recursion). Subdirectories come back with
/// `child == NONE`. Used by the live updater.
pub fn list_one(path: &[u8]) -> Option<Listing> {
    if blocked(path) {
        return None;
    }
    let mut l = Listing::default();
    list_into(path, &mut l).then_some(l)
}

/// Push `l`, the listing of `path`, and scan its folders.
fn descend<'s>(s: &Scope<'s>, path: Vec<u8>, mut l: Listing, ctx: &'s Ctx) {
    let mut kids = Vec::new();
    for e in l.ents.iter_mut() {
        if e.kind & 3 == KIND_DIR && e.kind & FLAG_MOUNT == 0 {
            let name = &l.names[e.name_off as usize..e.name_off as usize + e.name_len as usize];
            let child = crate::live::join(&path, name);
            if blocked(&child) {
                continue;
            }
            e.child = ctx.next_id.fetch_add(1, Ordering::Relaxed);
            kids.push((child, e.child));
        }
    }
    push(l, ctx);
    for (child, id) in kids {
        s.spawn(move |s| {
            if ctx.disk {
                yield_to_searches();
            }
            let mut l = Listing { id, names: Vec::new(), ents: Vec::new() };
            if !list_into(&child, &mut l) {
                if unsafe { GetLastError() } == ERROR_ACCESS_DENIED && SKIP.get().is_some_and(|v| !v.is_empty()) {
                    DENIED.lock().unwrap().push(child);
                }
                push(l, ctx);
                return;
            }
            descend(s, child, l, ctx);
        });
    }
}

/// Is the folder at `path` a mount point we don't descend into? Drive roots
/// are the tree's own folders; mount points inside are reparse points,
/// which are listed as links.
pub fn is_mount(_path: &[u8]) -> bool {
    false
}

fn push(l: Listing, ctx: &Ctx) {
    if ctx.disk {
        LISTED.fetch_add(l.ents.len(), Ordering::Relaxed);
    }
    let slot = rayon::current_thread_index().unwrap_or(ctx.out.len() - 1);
    ctx.out[slot].lock().unwrap().push(l);
}

fn add(l: &mut Listing, name: &[u8], kind: u8, size: u64, mtime: u32) {
    if name.is_empty() || name.len() > u16::MAX as usize {
        return;
    }
    l.ents.push(RawEnt { name_off: l.names.len() as u32, name_len: name.len() as u16, kind, size, mtime, child: NONE });
    l.names.extend_from_slice(name);
}

/// Returns false if the directory could not be opened.
fn list_into(path: &[u8], l: &mut Listing) -> bool {
    if path == b"/" {
        for d in sys::drives() {
            add(l, &d[1..], KIND_DIR, 0, 0);
        }
        return true;
    }
    let mut w = sys::wide(path);
    w.pop();
    if w.last() != Some(&(b'\\' as u16)) {
        w.push(b'\\' as u16);
    }
    w.extend([b'*' as u16, 0]);
    let mut fd: WIN32_FIND_DATAW = unsafe { std::mem::zeroed() };
    let h = unsafe {
        FindFirstFileExW(w.as_ptr(), FindExInfoBasic, (&raw mut fd).cast(), FindExSearchNameMatch, std::ptr::null(), FIND_FIRST_EX_LARGE_FETCH)
    };
    if h == INVALID_HANDLE_VALUE {
        return false;
    }
    let mut name = Vec::with_capacity(64);
    loop {
        let n = fd.cFileName.iter().position(|&c| c == 0).unwrap_or(fd.cFileName.len());
        let wn = &fd.cFileName[..n];
        if !(wn == [b'.' as u16] || wn == [b'.' as u16, b'.' as u16]) {
            name.clear();
            name.extend(sys::from_wide(wn));
            let a = fd.dwFileAttributes;
            let mut kind = if a & ATTR_REPARSE != 0 && matches!(fd.dwReserved0, TAG_SYMLINK | TAG_MOUNT_POINT | TAG_APPEXECLINK) {
                KIND_LINK
            } else if a & ATTR_DIRECTORY != 0 {
                KIND_DIR
            } else if a & ATTR_DEVICE != 0 {
                KIND_OTHER
            } else {
                KIND_FILE
            };
            if a & ATTR_HIDDEN != 0 {
                kind |= FLAG_HIDDEN;
            }
            let size = if kind & 3 == KIND_FILE { ((fd.nFileSizeHigh as u64) << 32) | fd.nFileSizeLow as u64 } else { 0 };
            let mtime = sys::filetime_secs(fd.ftLastWriteTime.dwLowDateTime, fd.ftLastWriteTime.dwHighDateTime);
            add(l, &name, kind, size, mtime);
        }
        if unsafe { FindNextFileW(h, &mut fd) } == 0 {
            break;
        }
    }
    unsafe { FindClose(h) };
    true
}

/// Attributes of one path without following a final link, as a listing
/// entry would carry them: (kind, size, mtime).
pub fn stat(path: &[u8]) -> Option<(u8, u64, u32)> {
    if path == b"/" || (path.len() == 3 && path[2] == b':') {
        return std::fs::metadata(sys::os(path)).ok().map(|_| (KIND_DIR, 0, 0));
    }
    let mut w = sys::wide(path);
    let mut fd: WIN32_FIND_DATAW = unsafe { std::mem::zeroed() };
    // FindFirstFile on the path itself gives the entry as its folder lists it.
    let h = unsafe { FindFirstFileExW(w.as_mut_ptr(), FindExInfoBasic, (&raw mut fd).cast(), FindExSearchNameMatch, std::ptr::null(), 0) };
    if h == INVALID_HANDLE_VALUE {
        return None;
    }
    unsafe { FindClose(h) };
    let a = fd.dwFileAttributes;
    let mut kind = if a & ATTR_REPARSE != 0 && matches!(fd.dwReserved0, TAG_SYMLINK | TAG_MOUNT_POINT | TAG_APPEXECLINK) {
        KIND_LINK
    } else if a & ATTR_DIRECTORY != 0 {
        KIND_DIR
    } else {
        KIND_FILE
    };
    if a & ATTR_HIDDEN != 0 {
        kind |= FLAG_HIDDEN;
    }
    let size = if kind & 3 == KIND_FILE { ((fd.nFileSizeHigh as u64) << 32) | fd.nFileSizeLow as u64 } else { 0 };
    Some((kind, size, sys::filetime_secs(fd.ftLastWriteTime.dwLowDateTime, fd.ftLastWriteTime.dwHighDateTime)))
}

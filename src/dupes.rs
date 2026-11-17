//! `dupe:`: files with the same content. Sizes come from the index, so only
//! files sharing a size with another are read: first their first and last
//! 64 KiB, then, for those still alike, the whole file. Hashes are kept per
//! (path, size, mtime), so the next keystroke in the window reads nothing.

use crate::content::open_regular;
use crate::live::Live;
use crate::query::{Query, Searcher};
use crate::walk::KIND_FILE;
use rayon::prelude::*;
use std::collections::HashMap;
use std::hash::Hasher;
use std::io::{Read, Seek, SeekFrom};
use std::sync::Mutex;

/// Smaller copies free little and flood the list.
pub const MIN_SIZE: u64 = 1 << 20;
/// Bytes hashed from each end before a whole file is read.
const EDGE: u64 = 64 << 10;
/// Files looked at, at most.
const MAX_FILES: usize = 400_000;
/// Groups listed, at most, the ones wasting the most space first.
pub const MAX_GROUPS: usize = 200;
/// Folders whose files the user does not manage: a "copy" there belongs to
/// a package or a tool, and removing it breaks that.
const SKIP: &[&[u8]] = &[b"node_modules", b"$recycle.bin", b"system volume information", b"__pycache__", b".git", b"appdata"];
/// The same for these right under a drive's root.
const SKIP_TOP: &[&[u8]] = &[b"windows", b"program files", b"program files (x86)", b"programdata"];

pub struct Cand {
    pub path: Vec<u8>,
    pub kind: u8,
    pub size: u64,
    pub mtime: u32,
}

/// Files `q` names (its words and filters) that may have a copy: at least
/// MIN_SIZE, outside SKIP, sharing their size with another.
pub fn candidates(live: &Live, q: &Query) -> Vec<Cand> {
    let mut q = Query { dupes: false, kind: Some(KIND_FILE), limit: MAX_FILES, ..q.clone() };
    q.size.0 = q.size.0.max(MIN_SIZE);
    let mut c: Vec<Cand> = Searcher { live }
        .search(&q)
        .into_iter()
        .map(|h| match h.over {
            Some(path) => {
                let o = live.over[&path];
                Cand { path, kind: o.kind, size: o.size, mtime: o.mtime }
            }
            None => {
                let i = h.idx as usize;
                let mut path = Vec::new();
                live.base.path(i, &mut path);
                Cand { path, kind: live.base.kind(i), size: live.base.size_of(i), mtime: live.base.mtime(i) }
            }
        })
        .filter(|c| !skipped(&c.path))
        .collect();
    let mut sizes: HashMap<u64, u32> = HashMap::new();
    for c in &c {
        *sizes.entry(c.size).or_default() += 1;
    }
    c.retain(|c| sizes[&c.size] > 1);
    c
}

/// `path` in the index's form, `/C:/x/y`.
fn skipped(path: &[u8]) -> bool {
    let top = path.split(|&b| b == b'/').nth(2).is_some_and(|t| SKIP_TOP.iter().any(|s| t.eq_ignore_ascii_case(s)));
    top || path.split(|&b| b == b'/').any(|part| SKIP.iter().any(|s| part.eq_ignore_ascii_case(s)))
}

/// The groups of `cands` with the same content, the most space wasted
/// first, each sorted by path; files that cannot be read (in use, or only
/// in the cloud) are left out.
pub fn groups(cands: Vec<Cand>) -> Vec<Vec<Cand>> {
    let by_edges = split(cands, |c| hash(c, false));
    let mut out: Vec<Vec<Cand>> = by_edges
        .into_iter()
        // A file no longer than both edges was read whole already.
        .flat_map(|g| if g[0].size <= 2 * EDGE { vec![g] } else { split(g, |c| hash(c, true)) })
        .collect();
    out.sort_by_key(|g| std::cmp::Reverse(g[0].size * (g.len() as u64 - 1)));
    out.truncate(MAX_GROUPS);
    for g in &mut out {
        g.sort_by(|a, b| a.path.cmp(&b.path));
    }
    out
}

/// `cands` split by (size, `key`), keeping groups of two or more.
fn split(cands: Vec<Cand>, key: impl Fn(&Cand) -> Option<u64> + Sync) -> Vec<Vec<Cand>> {
    let keyed: Vec<(Option<u64>, Cand)> = cands.into_par_iter().map(|c| (key(&c), c)).collect();
    let mut m: HashMap<(u64, u64), Vec<Cand>> = HashMap::new();
    for (k, c) in keyed {
        if let Some(k) = k {
            m.entry((c.size, k)).or_default().push(c);
        }
    }
    m.into_values().filter(|g| g.len() > 1).collect()
}

type Key = (Vec<u8>, u64, u32, bool);
static CACHE: Mutex<Option<HashMap<Key, u64>>> = Mutex::new(None);

/// A hash of `c`'s two edges, or (`whole`) of all of it, cached.
fn hash(c: &Cand, whole: bool) -> Option<u64> {
    let key = (c.path.clone(), c.size, c.mtime, whole);
    if let Some(h) = CACHE.lock().unwrap().as_ref().and_then(|m| m.get(&key)) {
        return Some(*h);
    }
    let h = read_hash(&c.path, c.size, whole)?;
    let mut g = CACHE.lock().unwrap();
    let m = g.get_or_insert_with(HashMap::new);
    // shortcut: forgets everything past 1M files instead of the oldest; an LRU if that shows.
    if m.len() > 1_000_000 {
        m.clear();
    }
    m.insert(key, h);
    Some(h)
}

fn read_hash(path: &[u8], size: u64, whole: bool) -> Option<u64> {
    let mut f = open_regular(path)?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mut buf = vec![0u8; 1 << 20];
    if whole || size <= 2 * EDGE {
        loop {
            let n = f.read(&mut buf).ok()?;
            if n == 0 {
                break;
            }
            h.write(&buf[..n]);
        }
    } else {
        let e = EDGE as usize;
        f.read_exact(&mut buf[..e]).ok()?;
        f.seek(SeekFrom::Start(size - EDGE)).ok()?;
        f.read_exact(&mut buf[e..2 * e]).ok()?;
        h.write(&buf[..2 * e]);
    }
    Some(h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(dir: &std::path::Path, name: &str, data: &[u8]) -> Cand {
        let p = dir.join(name);
        std::fs::write(&p, data).unwrap();
        Cand { path: crate::sys::bytes(&p), kind: KIND_FILE, size: data.len() as u64, mtime: 1 }
    }

    /// Same size alone is not a copy; same content under another name is;
    /// a difference past both edges still tells two files apart.
    #[test]
    fn groups_by_content() {
        let dir = std::env::temp_dir().join(format!("qasd-dupes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        let mut mid = a.clone();
        mid[1_500_000] ^= 1;
        let mut other = a.clone();
        other[0] ^= 1;
        let cands = vec![cand(&dir, "a.bin", &a), cand(&dir, "copy of a.bin", &a), cand(&dir, "mid.bin", &mid), cand(&dir, "other.bin", &other)];
        let g = groups(cands);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(g.len(), 1);
        let names: Vec<String> = g[0].iter().map(|c| String::from_utf8_lossy(&c.path).rsplit(['/', '\\']).next().unwrap().to_string()).collect();
        assert_eq!(names, ["a.bin", "copy of a.bin"]);
    }

    #[test]
    fn skips_managed_folders() {
        assert!(skipped(b"/C:/p/node_modules/x/a.js"));
        assert!(skipped(b"/C:/$Recycle.Bin/S-1/a"));
        assert!(skipped(b"/C:/Program Files/x/a.dll"));
        assert!(skipped(b"/C:/Users/me/AppData/Local/x/a"));
        assert!(!skipped(b"/C:/Users/me/Pictures/a.jpg"));
        assert!(!skipped(b"/D:/Projects/windows/a.bin"));
    }
}

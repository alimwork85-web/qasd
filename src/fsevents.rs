//! Disk change events on Windows, in the shape the engine expects from
//! FSEvents on macOS: batches of folder paths whose contents changed.
//!
//! One thread per fixed drive waits in `ReadDirectoryChangesW` on the drive
//! root, whole subtree. Each change names a file; its folder is what gets
//! relisted. A buffer overflow (the drive changed faster than we read) is
//! reported as a replayable drop: the engine replays from its last good event.
//!
//! Event ids are milliseconds since the unix epoch. A stream asked to start
//! in the past (a restart, an overflow) replays the change journal (`usn`)
//! for the folders changed since; where that fails, it begins with a
//! "history unavailable" root event: the engine relists the folders changed
//! since its last sync (`relist_changed`), then `HISTORY_DONE`.

use crate::sys;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME,
    FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE, FILE_NOTIFY_INFORMATION, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING, ReadDirectoryChangesW,
};
use windows_sys::Win32::System::IO::CancelIoEx;

pub const MUST_SCAN_SUBDIRS: u32 = 0x1;
pub const USER_DROPPED: u32 = 0x2;
pub const KERNEL_DROPPED: u32 = 0x4;
pub const HISTORY_DONE: u32 = 0x10;

pub struct Event {
    pub path: Vec<u8>,
    pub flags: u32,
    pub id: u64,
    /// Which `watch` delivered it (its `stream` number).
    pub stream: u32,
}

/// The newest event id: now, in ms.
pub fn current_id() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

struct Drive {
    handle: HANDLE,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// A running stream; dropping it stops it.
pub struct Stream {
    stop: Arc<AtomicBool>,
    drives: Vec<Drive>,
}

// Handles are only used to cancel and close, once, on drop.
unsafe impl Send for Stream {}
unsafe impl Sync for Stream {}

impl Drop for Stream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        for d in &mut self.drives {
            unsafe { CancelIoEx(d.handle, std::ptr::null()) };
            if let Some(t) = d.thread.take() {
                let _ = t.join();
            }
            unsafe { CloseHandle(d.handle) };
        }
    }
}

/// Watch every fixed drive from `since` (an event id). Batches of folder
/// events, stamped with `stream`, arrive on `tx` until the stream is dropped.
pub fn watch(since: u64, _latency: f64, tx: Sender<Vec<Event>>, stream: u32) -> Stream {
    let stop = Arc::new(AtomicBool::new(false));
    let mut drives = Vec::new();
    for root in sys::drives() {
        let w = sys::wide(&root);
        let h = unsafe {
            CreateFileW(
                w.as_ptr(),
                FILE_LIST_DIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                std::ptr::null_mut(),
            )
        };
        if h == INVALID_HANDLE_VALUE {
            continue;
        }
        let (tx, stop2, hs) = (tx.clone(), stop.clone(), h as usize);
        let thread = std::thread::Builder::new()
            .name(format!("qasd-watch-{}", root[1] as char))
            .spawn(move || follow(hs as HANDLE, root, tx, stop2, stream))
            .ok();
        drives.push(Drive { handle: h, thread });
    }
    // Starting in the past: replay what changed meanwhile from each drive's
    // change journal (`usn`), on its own thread; live events flow already
    // (relisting a folder twice is harmless). A drive whose journal can't
    // say makes the engine relist what changed since its last sync.
    let now = current_id();
    if since != 0 && since + 1000 < now {
        std::thread::spawn(move || {
            let t = std::time::Instant::now();
            let mut dirs = Vec::new();
            let all = sys::drives().iter().all(|d| match crate::usn::changed_since(d, since) {
                Some(v) => {
                    dirs.extend(v);
                    true
                }
                None => false,
            });
            if all {
                eprintln!("{} change journal: {} folders changed since {since}, found in {:.2?}", crate::query::now_secs(), dirs.len(), t.elapsed());
                let _ = tx.send(dirs.into_iter().map(|path| Event { path, flags: 0, id: now, stream }).collect());
            } else {
                let _ = tx.send(vec![Event { path: b"/".to_vec(), flags: MUST_SCAN_SUBDIRS, id: since, stream }]);
            }
            let _ = tx.send(vec![Event { path: b"/".to_vec(), flags: HISTORY_DONE, id: now, stream }]);
        });
    } else {
        let _ = tx.send(vec![Event { path: b"/".to_vec(), flags: HISTORY_DONE, id: now, stream }]);
    }
    Stream { stop, drives }
}

fn follow(h: HANDLE, root: Vec<u8>, tx: Sender<Vec<Event>>, stop: Arc<AtomicBool>, stream: u32) {
    sys::priority_interactive();
    // DWORD-aligned, 1 MB: a burst of renames fits; 64 KB is the limit only
    // over the network.
    let mut buf = vec![0u64; (1 << 20) / 8];
    // Case as the index stores it, per folder named by a change.
    let mut cases: std::collections::HashMap<Vec<u8>, Vec<u8>> = std::collections::HashMap::new();
    while !stop.load(Ordering::SeqCst) {
        let mut n = 0u32;
        let ok = unsafe {
            ReadDirectoryChangesW(
                h,
                buf.as_mut_ptr().cast(),
                (buf.len() * 8) as u32,
                1,
                FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME | FILE_NOTIFY_CHANGE_SIZE | FILE_NOTIFY_CHANGE_LAST_WRITE,
                &mut n,
                std::ptr::null_mut(),
                None,
            )
        } != 0;
        if stop.load(Ordering::SeqCst) {
            return;
        }
        if !ok {
            // Cancelled, or the drive went away.
            std::thread::sleep(std::time::Duration::from_millis(500));
            continue;
        }
        let id = current_id();
        if n == 0 {
            // Overflow: changes were lost here, but the change journal kept
            // them: the engine replays from its last good event once the
            // burst is over (a new stream from then, `watch`).
            let _ = tx.send(vec![Event { path: b"/".to_vec(), flags: MUST_SCAN_SUBDIRS | USER_DROPPED, id, stream }]);
            continue;
        }
        let mut dirs: Vec<Vec<u8>> = Vec::new();
        let base = buf.as_ptr() as *const u8;
        let mut off = 0usize;
        loop {
            // Safety: the kernel wrote a chain of records within `n` bytes.
            let rec = unsafe { &*(base.add(off) as *const FILE_NOTIFY_INFORMATION) };
            let len = rec.FileNameLength as usize / 2;
            let name = unsafe { std::slice::from_raw_parts(rec.FileName.as_ptr(), len) };
            let rel = sys::from_wide(name);
            let rel: Vec<u8> = rel.iter().map(|&c| if c == b'\\' { b'/' } else { c }).collect();
            let full = crate::live::join(&root, &rel);
            let dir = match full.iter().rposition(|&b| b == b'/') {
                Some(c) if c > root.len() => full[..c].to_vec(),
                _ => root.clone(),
            };
            if !dirs.contains(&dir) {
                dirs.push(dir);
            }
            if rec.NextEntryOffset == 0 {
                break;
            }
            off += rec.NextEntryOffset as usize;
            if off >= n as usize {
                break;
            }
        }
        if cases.len() > 4096 {
            cases.clear();
        }
        let batch = dirs
            .into_iter()
            .map(|d| {
                let path = match cases.get(&d) {
                    Some(p) => p.clone(),
                    None => {
                        let p = sys::true_case(&d).unwrap_or_else(|| d.clone());
                        cases.insert(d, p.clone());
                        p
                    }
                };
                Event { path, flags: 0, id, stream }
            })
            .collect();
        if tx.send(batch).is_err() {
            return;
        }
    }
}

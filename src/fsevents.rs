//! Disk change events on Windows, in the shape the engine expects from
//! FSEvents on macOS: batches of folder paths whose contents changed.
//!
//! One thread per drive waits in `ReadDirectoryChangesW` on the drive root,
//! whole subtree. A hidden window hears drives being plugged in or removed
//! (`WM_DEVICECHANGE`): a new drive is watched and indexed, a gone one
//! dropped, and a drive Windows is about to eject is let go of first, so
//! "Safely Remove" never finds it in use. Each change names a file; its folder is what gets
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
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::IO::CancelIoEx;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DBT_DEVTYP_HANDLE, DEV_BROADCAST_HANDLE, DEV_BROADCAST_HDR, DEVICE_NOTIFY_WINDOW_HANDLE, DefWindowProcW, DestroyWindow,
    DispatchMessageW, GWLP_USERDATA, GetMessageW, GetWindowLongPtrW, HDEVNOTIFY, KillTimer, MSG, PostMessageW, PostQuitMessage,
    RegisterClassW, RegisterDeviceNotificationW, SetTimer, SetWindowLongPtrW, TranslateMessage, UnregisterDeviceNotification, WM_APP,
    WM_DEVICECHANGE, WM_TIMER, WNDCLASSW,
};

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

/// One drive being followed: its root handle and the thread reading it.
struct Watcher {
    handle: HANDLE,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// So Windows tells us before ejecting the drive (`DBT_DEVICEQUERYREMOVE`).
    notify: HDEVNOTIFY,
}

impl Watcher {
    fn start(root: Vec<u8>, tx: Sender<Vec<Event>>, stream: u32, hwnd: HWND) -> Option<Watcher> {
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
            return None;
        }
        let mut filter: DEV_BROADCAST_HANDLE = unsafe { std::mem::zeroed() };
        filter.dbch_size = size_of::<DEV_BROADCAST_HANDLE>() as u32;
        filter.dbch_devicetype = DBT_DEVTYP_HANDLE;
        filter.dbch_handle = h;
        let notify = unsafe { RegisterDeviceNotificationW(hwnd, (&raw const filter).cast(), DEVICE_NOTIFY_WINDOW_HANDLE) };
        let stop = Arc::new(AtomicBool::new(false));
        let (stop2, hs) = (stop.clone(), h as usize);
        let thread =
            std::thread::Builder::new().name(format!("qasd-watch-{}", root[1] as char)).spawn(move || follow(hs as HANDLE, root, tx, stop2, stream)).ok();
        Some(Watcher { handle: h, stop, thread, notify })
    }

    /// Stop following and let go of the drive (so it can be ejected).
    fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        unsafe { CancelIoEx(self.handle, std::ptr::null()) };
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        unsafe {
            if !self.notify.is_null() {
                UnregisterDeviceNotification(self.notify);
            }
            CloseHandle(self.handle);
        }
    }
}

/// The device thread's state: a hidden window that hears drives come and
/// go, and a watcher per drive.
struct Devices {
    tx: Sender<Vec<Event>>,
    stream: u32,
    hwnd: HWND,
    watchers: Vec<(Vec<u8>, Watcher)>,
}

impl Devices {
    /// Follow exactly the drives there are now. A drive added or gone is
    /// told to the engine as a change of `/`: it relists the drives, scans
    /// a new one and drops a gone one's entries.
    fn sync(&mut self, announce: bool) {
        let want = sys::drives();
        let mut changed = false;
        let mut i = 0;
        while i < self.watchers.len() {
            if want.contains(&self.watchers[i].0) {
                i += 1;
            } else {
                let (root, w) = self.watchers.remove(i);
                eprintln!("{} drive {} gone", crate::query::now_secs(), sys::display(&root));
                w.stop();
                changed = true;
            }
        }
        for root in want {
            if self.watchers.iter().any(|(r, _)| *r == root) {
                continue;
            }
            if let Some(w) = Watcher::start(root.clone(), self.tx.clone(), self.stream, self.hwnd) {
                if announce {
                    eprintln!("{} drive {} added", crate::query::now_secs(), sys::display(&root));
                }
                self.watchers.push((root, w));
                changed = true;
            }
        }
        if changed && announce {
            let _ = self.tx.send(vec![Event { path: b"/".to_vec(), flags: 0, id: current_id(), stream: self.stream }]);
        }
    }

    /// Windows asks to eject the drive this handle is on: let go of it now.
    fn release(&mut self, h: HANDLE) {
        if let Some(i) = self.watchers.iter().position(|(_, w)| w.handle == h) {
            let (root, w) = self.watchers.remove(i);
            eprintln!("{} drive {} being ejected: let go", crate::query::now_secs(), sys::display(&root));
            w.stop();
        }
    }

    fn stop_all(&mut self) {
        for (_, w) in self.watchers.drain(..) {
            w.stop();
        }
    }
}

const WM_QUIT_STREAM: u32 = WM_APP + 1;
const DBT_DEVICEARRIVAL: usize = 0x8000;
const DBT_DEVICEQUERYREMOVE: usize = 0x8001;
const DBT_DEVICEQUERYREMOVEFAILED: usize = 0x8002;
const DBT_DEVICEREMOVECOMPLETE: usize = 0x8004;
const DBT_DEVTYP_VOLUME: u32 = 2;

unsafe extern "system" fn device_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let d = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut Devices;
    if d.is_null() {
        return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
    }
    // Safety: set before any message we handle; only this thread uses it.
    let d = unsafe { &mut *d };
    match msg {
        WM_DEVICECHANGE => {
            let hdr = lp as *const DEV_BROADCAST_HDR;
            let kind = if hdr.is_null() { 0 } else { unsafe { (*hdr).dbch_devicetype } };
            match wp {
                DBT_DEVICEARRIVAL | DBT_DEVICEREMOVECOMPLETE if kind == DBT_DEVTYP_VOLUME => d.sync(true),
                DBT_DEVICEQUERYREMOVE if kind == DBT_DEVTYP_HANDLE => d.release(unsafe { (*(lp as *const DEV_BROADCAST_HANDLE)).dbch_handle }),
                DBT_DEVICEQUERYREMOVEFAILED if kind == DBT_DEVTYP_HANDLE => d.sync(true),
                _ => {}
            }
            1
        }
        WM_TIMER => {
            // Belt and braces: a missed device message is caught within 5 s.
            d.sync(true);
            0
        }
        WM_QUIT_STREAM => {
            d.stop_all();
            unsafe {
                KillTimer(hwnd, 1);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                DestroyWindow(hwnd);
                PostQuitMessage(0);
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

/// The thread that owns the hidden window and the watchers. Sends its
/// window back once ready.
fn device_thread(tx: Sender<Vec<Event>>, stream: u32, ready: std::sync::mpsc::Sender<usize>) {
    let class: Vec<u16> = "QasdDeviceWatch".encode_utf16().chain([0]).collect();
    let hinst = unsafe { GetModuleHandleW(std::ptr::null()) };
    let wc = WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(device_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinst,
        hIcon: std::ptr::null_mut(),
        hCursor: std::ptr::null_mut(),
        hbrBackground: std::ptr::null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: class.as_ptr(),
    };
    // (Registered once per process; a second stream finds it there.)
    unsafe { RegisterClassW(&wc) };
    // Top-level and never shown: volume arrivals are broadcast to top-level
    // windows only, not to message-only ones.
    let hwnd = unsafe { CreateWindowExW(0, class.as_ptr(), std::ptr::null(), 0, 0, 0, 0, 0, std::ptr::null_mut(), std::ptr::null_mut(), hinst, std::ptr::null()) };
    let _ = ready.send(hwnd as usize);
    if hwnd.is_null() {
        return;
    }
    let mut d = Box::new(Devices { tx, stream, hwnd, watchers: Vec::new() });
    d.sync(false);
    let p = Box::into_raw(d);
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, p as isize);
        SetTimer(hwnd, 1, 5000, None);
    }
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    drop(unsafe { Box::from_raw(p) });
}

/// A running stream; dropping it stops it and lets go of every drive.
pub struct Stream {
    hwnd: usize,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Stream {
    fn drop(&mut self) {
        if self.hwnd != 0 {
            unsafe { PostMessageW(self.hwnd as HWND, WM_QUIT_STREAM, 0, 0) };
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Is `drive` (`/E:`) removable media (a USB stick, a memory card)?
fn removable(drive: &[u8]) -> bool {
    let root: Vec<u16> = format!("{}:\\", drive[1] as char).encode_utf16().chain([0]).collect();
    unsafe { windows_sys::Win32::Storage::FileSystem::GetDriveTypeW(root.as_ptr()) == 2 }
}

/// Watch every drive from `since` (an event id); drives plugged in or
/// unplugged meanwhile are picked up. Batches of folder events, stamped
/// with `stream`, arrive on `tx` until the stream is dropped.
pub fn watch(since: u64, _latency: f64, tx: Sender<Vec<Event>>, stream: u32) -> Stream {
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let tx2 = tx.clone();
    let thread = std::thread::Builder::new().name("qasd-devices".into()).spawn(move || device_thread(tx2, stream, ready_tx)).ok();
    let hwnd = ready_rx.recv().unwrap_or(0);
    // Starting in the past: replay what changed meanwhile from each drive's
    // change journal (`usn`), on its own thread; live events flow already
    // (relisting a folder twice is harmless). A removable drive without a
    // journal (FAT, exFAT) is rescanned whole; any other drive whose journal
    // can't say makes the engine relist what changed since its last sync.
    let now = current_id();
    if since != 0 && since + 1000 < now {
        std::thread::spawn(move || {
            let t = std::time::Instant::now();
            let (mut dirs, mut rescan, mut all) = (Vec::new(), Vec::new(), true);
            for d in sys::drives() {
                match crate::usn::changed_since(&d, since) {
                    Some(v) => dirs.extend(v),
                    None if removable(&d) => rescan.push(d),
                    None => all = false,
                }
            }
            if all {
                eprintln!("{} change journal: {} folders changed since {since}, found in {:.2?}", crate::query::now_secs(), dirs.len(), t.elapsed());
                let mut batch: Vec<Event> = dirs.into_iter().map(|path| Event { path, flags: 0, id: now, stream }).collect();
                batch.extend(rescan.into_iter().map(|path| Event { path, flags: MUST_SCAN_SUBDIRS, id: now, stream }));
                // The drives themselves may have changed while we were away.
                batch.push(Event { path: b"/".to_vec(), flags: 0, id: now, stream });
                let _ = tx.send(batch);
            } else {
                let _ = tx.send(vec![Event { path: b"/".to_vec(), flags: MUST_SCAN_SUBDIRS, id: since, stream }]);
            }
            let _ = tx.send(vec![Event { path: b"/".to_vec(), flags: HISTORY_DONE, id: now, stream }]);
        });
    } else {
        let _ = tx.send(vec![Event { path: b"/".to_vec(), flags: HISTORY_DONE, id: now, stream }]);
    }
    Stream { hwnd, thread }
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
                        // (Through a subst drive the real path is on another
                        // drive: keep the one the change came from.)
                        let p = sys::true_case(&d).filter(|p| p.get(..3) == d.get(..3)).unwrap_or_else(|| d.clone());
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

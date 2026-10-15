//! The Windows layer under the engine.
//!
//! The engine (ported from fsearch) keeps every path as bytes in POSIX
//! form: one tree rooted at `/` whose children are the drives. So
//! `C:\Users\me\a.txt` is `/C:/Users/me/a.txt` inside, which keeps `in:` a
//! byte range and every `/` split working. Bytes are WTF-8 (what `OsStr`
//! holds on Windows), so any NTFS name round-trips.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_NAME_NORMALIZED, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, GetFinalPathNameByHandleW, OPEN_EXISTING, VOLUME_NAME_DOS,
};

/// Internal path -> the OS path. `/C:` is the drive's root, `C:\`.
pub fn os(p: &[u8]) -> PathBuf {
    let rest = p.strip_prefix(b"/").unwrap_or(p);
    let mut b: Vec<u8> = rest.iter().map(|&c| if c == b'/' { b'\\' } else { c }).collect();
    if b.len() == 2 && b[1] == b':' {
        b.push(b'\\');
    }
    if b.is_empty() {
        b.push(b'\\');
    }
    // Safety: `p` is WTF-8 (from `bytes` or UTF-8 input); swapping one ASCII
    // byte for another keeps it so.
    PathBuf::from(unsafe { OsStr::from_encoded_bytes_unchecked(&b) })
}

/// OS path -> internal path. Drops the `\\?\` prefix, upper-cases the drive
/// letter and trailing separators.
pub fn bytes(p: impl AsRef<OsStr>) -> Vec<u8> {
    let raw = p.as_ref().as_encoded_bytes();
    let raw = raw.strip_prefix(br"\\?\").unwrap_or(raw);
    if let Some(unc) = raw.strip_prefix(br"UNC\") {
        let mut v = b"//".to_vec();
        v.extend(unc.iter().map(|&c| if c == b'\\' { b'/' } else { c }));
        return v;
    }
    let mut v = Vec::with_capacity(raw.len() + 1);
    v.push(b'/');
    v.extend(raw.iter().map(|&c| if c == b'\\' { b'/' } else { c }));
    if v.len() >= 3 && v[2] == b':' {
        v[1] = v[1].to_ascii_uppercase();
    }
    while v.len() > 1 && v.last() == Some(&b'/') {
        v.pop();
    }
    v
}

/// Internal path as the user sees it: `C:\Users\me`.
pub fn display(p: &[u8]) -> String {
    if p == b"/" {
        return "\\".into();
    }
    os(p).to_string_lossy().into_owned()
}

/// Is this an internal path (`/C:/...`, `/C:`, `/`)?
pub fn is_internal(s: &str) -> bool {
    let b = s.as_bytes();
    b == b"/" || (b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' && b.get(3).is_none_or(|&c| c == b'/'))
}

/// A path typed by the user (`C:\x`, `c:/x`, `/C:/x`, relative) -> internal.
pub fn from_user(s: &str) -> Vec<u8> {
    let fixed;
    let s = if s.starts_with('/') && s.contains('\\') {
        fixed = s.replace('\\', "/");
        fixed.as_str()
    } else {
        s
    };
    if is_internal(s) {
        return crate::live::normalize(s.as_bytes());
    }
    let p = Path::new(s);
    let p = if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir().map(|d| d.join(p)).unwrap_or_else(|_| p.to_path_buf()) };
    bytes(p)
}

/// The real path (actual case, links resolved), internal; `p` as given if it
/// cannot be opened.
pub fn real(p: &[u8]) -> Vec<u8> {
    if p == b"/" || p.is_empty() {
        return b"/".to_vec();
    }
    std::fs::canonicalize(os(p)).map_or_else(|_| p.to_vec(), bytes)
}

/// UTF-16, NUL terminated, with the `\\?\` long-path prefix.
pub fn wide(p: &[u8]) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    let o = os(p);
    let mut w: Vec<u16> = r"\\?\".encode_utf16().collect();
    w.extend(o.as_os_str().encode_wide());
    w.push(0);
    w
}

/// UTF-16 name -> WTF-8 bytes.
pub fn from_wide(w: &[u16]) -> Vec<u8> {
    use std::os::windows::ffi::OsStringExt;
    std::ffi::OsString::from_wide(w).into_encoded_bytes()
}

/// The real-case internal path of an existing folder or file, by its
/// handle (`GetFinalPathNameByHandleW`). Change notifications carry the
/// case the changing program used; the index holds the stored one.
pub fn true_case(p: &[u8]) -> Option<Vec<u8>> {
    let w = wide(p);
    let h = unsafe {
        CreateFileW(
            w.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return None;
    }
    let r = final_path(h);
    unsafe { CloseHandle(h) };
    r
}

pub(crate) fn final_path(h: HANDLE) -> Option<Vec<u8>> {
    let mut buf = vec![0u16; 512];
    loop {
        let n = unsafe { GetFinalPathNameByHandleW(h, buf.as_mut_ptr(), buf.len() as u32, FILE_NAME_NORMALIZED | VOLUME_NAME_DOS) } as usize;
        if n == 0 {
            return None;
        }
        if n < buf.len() {
            use std::os::windows::ffi::OsStringExt;
            return Some(bytes(std::ffi::OsString::from_wide(&buf[..n])));
        }
        buf.resize(n + 1, 0);
    }
}

/// Delete a file by name, whatever it is: `std::fs::remove_file` can't
/// remove an AF_UNIX socket file (a reparse point it fails to open).
pub fn delete(p: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    let w: Vec<u16> = p.as_os_str().encode_wide().chain([0]).collect();
    unsafe { windows_sys::Win32::Storage::FileSystem::DeleteFileW(w.as_ptr()) != 0 }
}

/// FILETIME (100 ns since 1601) -> unix seconds, clamped to u32.
pub fn filetime_secs(lo: u32, hi: u32) -> u32 {
    let t = ((hi as u64) << 32) | lo as u64;
    (t / 10_000_000).saturating_sub(11_644_473_600).min(u32::MAX as u64) as u32
}

/// The user's home, internal (`/C:/Users/me`).
pub fn home() -> String {
    let h = std::env::var_os("USERPROFILE").map(PathBuf::from).or_else(|| std::env::home_dir()).unwrap_or_else(|| PathBuf::from("C:\\"));
    String::from_utf8_lossy(&bytes(h)).into_owned()
}

/// Thread priorities standing in for macOS QoS classes.
pub fn priority_interactive() {
    set_priority(windows_sys::Win32::System::Threading::THREAD_PRIORITY_ABOVE_NORMAL);
}
pub fn priority_initiated() {
    set_priority(windows_sys::Win32::System::Threading::THREAD_PRIORITY_NORMAL);
}
pub fn priority_utility() {
    set_priority(windows_sys::Win32::System::Threading::THREAD_PRIORITY_BELOW_NORMAL);
}
pub fn priority_background() {
    set_priority(windows_sys::Win32::System::Threading::THREAD_MODE_BACKGROUND_BEGIN);
}

fn set_priority(p: i32) {
    use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority};
    unsafe { SetThreadPriority(GetCurrentThread(), p) };
}

/// Non-blocking exclusive lock on a whole file (like `flock(LOCK_EX|LOCK_NB)`).
/// Held until the handle closes.
pub fn try_lock(f: &std::fs::File) -> bool {
    f.try_lock().is_ok()
}

/// Is a process with this id running?
pub fn alive(pid: u32) -> bool {
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h.is_null() {
        return false;
    }
    let mut code = 0u32;
    let ok = unsafe { GetExitCodeProcess(h, &mut code) } != 0;
    unsafe { CloseHandle(h) };
    ok && code == 259 // STILL_ACTIVE
}

/// End a process.
pub fn kill(pid: u32) -> bool {
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
    let h = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
    if h.is_null() {
        return false;
    }
    let ok = unsafe { TerminateProcess(h, 0) } != 0;
    unsafe { CloseHandle(h) };
    ok
}

/// Running with administrator rights (an elevated token)?
pub fn elevated() -> bool {
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    static E: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *E.get_or_init(|| unsafe {
        let mut t: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut t) == 0 {
            return false;
        }
        let mut e = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut n = 0u32;
        let ok = GetTokenInformation(t, TokenElevation, (&raw mut e).cast(), size_of::<TOKEN_ELEVATION>() as u32, &mut n) != 0;
        CloseHandle(t);
        ok && e.TokenIsElevated != 0
    })
}

/// Physical memory in bytes.
pub fn memory_bytes() -> u64 {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    m.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
    if unsafe { GlobalMemoryStatusEx(&mut m) } == 0 { 8 << 30 } else { m.ullTotalPhys }
}

/// Drives to index, as internal roots (`/C:`): fixed ones (internal disks,
/// most USB hard drives) and removable ones with media in them (USB sticks,
/// memory cards). Not network shares or optical discs. `QASD_DRIVES=C,D`
/// overrides; `QASD_NO_REMOVABLE` leaves removable ones out.
pub fn drives() -> Vec<Vec<u8>> {
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW};
    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;
    if let Ok(v) = std::env::var("QASD_DRIVES") {
        return v.split([',', ';', ' ']).filter(|s| !s.is_empty()).map(|s| format!("/{}:", &s[..1].to_ascii_uppercase()).into_bytes()).collect();
    }
    let removable = std::env::var_os("QASD_NO_REMOVABLE").is_none();
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .filter(|i| {
            let root: Vec<u16> = format!("{}:\\", (b'A' + i) as char).encode_utf16().chain([0]).collect();
            match unsafe { GetDriveTypeW(root.as_ptr()) } {
                DRIVE_FIXED => true,
                // A card reader with no card in it answers nothing.
                DRIVE_REMOVABLE if removable => unsafe {
                    GetVolumeInformationW(root.as_ptr(), std::ptr::null_mut(), 0, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), 0) != 0
                },
                _ => false,
            }
        })
        .map(|i| format!("/{}:", (b'A' + i) as char).into_bytes())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        assert_eq!(os(b"/C:/Users/me"), PathBuf::from(r"C:\Users\me"));
        assert_eq!(os(b"/C:"), PathBuf::from(r"C:\"));
        assert_eq!(bytes(r"C:\Users\me\"), b"/C:/Users/me");
        assert_eq!(bytes(r"\\?\c:\Users"), b"/C:/Users");
        assert_eq!(bytes(r"C:\"), b"/C:");
        assert_eq!(display(b"/C:/a/b"), r"C:\a\b");
        assert!(is_internal("/C:/x") && is_internal("/D:") && !is_internal("C:/x"));
        assert_eq!(from_user("/C:/x/"), b"/C:/x");
    }
}

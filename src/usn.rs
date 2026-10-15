//! What changed on a drive since a moment, from its NTFS change journal
//! (USN journal): the folders to relist after a restart or after change
//! notifications overflowed, instead of checking every folder's mtime.
//!
//! Read without administrator rights through a handle to the drive's root
//! folder (`FSCTL_READ_UNPRIVILEGED_USN_JOURNAL`, Windows 10 1709+). Each
//! record names a changed file and its parent folder by file id; a folder
//! id becomes a path through `OpenFileById` and `GetFinalPathNameByHandleW`.

use std::collections::HashSet;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_DESCRIPTOR, FILE_ID_DESCRIPTOR_0, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FileIdType, OPEN_EXISTING, OpenFileById,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::{FSCTL_QUERY_USN_JOURNAL, FSCTL_READ_UNPRIVILEGED_USN_JOURNAL, READ_USN_JOURNAL_DATA_V0, USN_JOURNAL_DATA_V0};

const BUF: usize = 1 << 20;

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// FILETIME -> unix ms.
fn ms(ft: i64) -> u64 {
    (ft as u64 / 10_000).saturating_sub(11_644_473_600_000)
}

struct Journal {
    root: Handle,
    id: u64,
    first: i64,
    next: i64,
}

fn open(drive: &[u8]) -> Option<Journal> {
    let letter = *drive.get(1)? as char;
    let w: Vec<u16> = format!("{letter}:\\").encode_utf16().chain([0]).collect();
    let h = unsafe {
        CreateFileW(
            w.as_ptr(),
            0,
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
    let root = Handle(h);
    let mut jd: USN_JOURNAL_DATA_V0 = unsafe { std::mem::zeroed() };
    let mut n = 0u32;
    let ok = unsafe {
        DeviceIoControl(root.0, FSCTL_QUERY_USN_JOURNAL, std::ptr::null(), 0, (&raw mut jd).cast(), size_of::<USN_JOURNAL_DATA_V0>() as u32, &mut n, std::ptr::null_mut())
    };
    (ok != 0).then_some(Journal { root, id: jd.UsnJournalID, first: jd.FirstUsn, next: jd.NextUsn })
}

/// One record seen in the journal.
struct Rec {
    usn: i64,
    ms: u64,
    parent: u64,
}

impl Journal {
    /// Records from `start` on, one buffer's worth; and where the next read
    /// starts. Empty at the end.
    fn read(&self, start: i64, buf: &mut [u8]) -> Option<(Vec<Rec>, i64)> {
        let rd = READ_USN_JOURNAL_DATA_V0 { StartUsn: start, ReasonMask: 0xFFFF_FFFF, ReturnOnlyOnClose: 0, Timeout: 0, BytesToWaitFor: 0, UsnJournalID: self.id };
        let mut n = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                self.root.0,
                FSCTL_READ_UNPRIVILEGED_USN_JOURNAL,
                (&raw const rd).cast(),
                size_of::<READ_USN_JOURNAL_DATA_V0>() as u32,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                &mut n,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 || n < 8 {
            return None;
        }
        let n = n as usize;
        let next = i64::from_le_bytes(buf[..8].try_into().unwrap());
        let mut out = Vec::new();
        let mut at = 8;
        while at + 60 <= n {
            let b = &buf[at..];
            let len = u32::from_le_bytes(b[..4].try_into().unwrap()) as usize;
            let major = u16::from_le_bytes([b[4], b[5]]);
            if len == 0 || at + len > n {
                break;
            }
            // V2: 64-bit ids. (A V3 record carries 128-bit ids; NTFS uses the low 64.)
            let (parent, usn, ts) = match major {
                2 => (u64::from_le_bytes(b[16..24].try_into().unwrap()), i64::from_le_bytes(b[24..32].try_into().unwrap()), i64::from_le_bytes(b[32..40].try_into().unwrap())),
                3 if len >= 76 => (u64::from_le_bytes(b[24..32].try_into().unwrap()), i64::from_le_bytes(b[40..48].try_into().unwrap()), i64::from_le_bytes(b[48..56].try_into().unwrap())),
                _ => {
                    at += len;
                    continue;
                }
            };
            out.push(Rec { usn, ms: ms(ts), parent });
            at += len;
        }
        Some((out, next))
    }

    /// The first record at or after `usn`.
    fn first_at(&self, usn: i64, buf: &mut [u8]) -> Option<Rec> {
        self.read(usn, buf).and_then(|(r, _)| r.into_iter().next())
    }

    /// A folder's path from its file id.
    fn path_of(&self, id: u64) -> Option<Vec<u8>> {
        let d = FILE_ID_DESCRIPTOR { dwSize: size_of::<FILE_ID_DESCRIPTOR>() as u32, Type: FileIdType, Anonymous: FILE_ID_DESCRIPTOR_0 { FileId: id as i64 } };
        let h = unsafe {
            OpenFileById(self.root.0, &d, FILE_READ_ATTRIBUTES, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, std::ptr::null(), FILE_FLAG_BACKUP_SEMANTICS)
        };
        if h == INVALID_HANDLE_VALUE {
            return None;
        }
        let h = Handle(h);
        crate::sys::final_path(h.0)
    }
}

/// The folders on `drive` (`/C:`) whose contents changed since `since` (unix
/// ms), as index paths. None when the journal can't say: none on this
/// drive, or it doesn't reach back that far (it wrapped, or was recreated).
pub fn changed_since(drive: &[u8], since: u64) -> Option<Vec<Vec<u8>>> {
    let j = open(drive)?;
    let mut buf = vec![0u8; BUF];
    // The journal must hold the moment itself.
    let oldest = j.first_at(j.first, &mut buf)?;
    if oldest.ms > since {
        return None;
    }
    // Binary search for a record before `since`, then read on from it.
    let (mut lo, mut hi) = (j.first, j.next);
    while hi - lo > (BUF as i64) {
        let mid = lo + (hi - lo) / 2;
        match j.first_at(mid, &mut buf) {
            Some(r) if r.ms < since => lo = r.usn.max(lo + 1).min(mid),
            Some(_) => hi = mid,
            None => hi = mid,
        }
    }
    let mut parents: HashSet<u64> = HashSet::new();
    let mut at = lo;
    loop {
        let Some((recs, next)) = j.read(at, &mut buf) else { break };
        if recs.is_empty() || next <= at {
            break;
        }
        parents.extend(recs.iter().filter(|r| r.ms + 2000 >= since).map(|r| r.parent));
        at = next;
        if at >= j.next {
            break;
        }
    }
    // Folders gone since don't resolve: their own parent's record covers them.
    let mut dirs: Vec<Vec<u8>> = parents.into_iter().filter_map(|id| j.path_of(id)).collect();
    dirs.sort();
    dirs.dedup();
    Some(dirs)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file written now shows up as its folder changing since a moment ago.
    #[test]
    fn sees_a_new_file() {
        let d = std::env::temp_dir().join(format!("qasd-usn-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let d = crate::sys::bytes(d.canonicalize().unwrap());
        let since = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
        std::fs::write(crate::sys::os(&d).join("x.txt"), "x").unwrap();
        let drive = d[..3].to_vec();
        let Some(dirs) = changed_since(&drive, since) else {
            eprintln!("no readable journal on {}", String::from_utf8_lossy(&drive));
            return;
        };
        let _ = std::fs::remove_dir_all(crate::sys::os(&d));
        assert!(dirs.contains(&d), "{:?}", dirs.iter().map(|p| String::from_utf8_lossy(p).into_owned()).collect::<Vec<_>>());
    }
}

//! Named pipes: the daemon's transport (a unix socket in fsearch).
//!
//! Handles are opened for overlapped I/O, so one thread can read while
//! another writes on a clone (`stdio`). Only the current user (and SYSTEM)
//! may connect, whether or not the daemon runs elevated; remote clients are
//! refused.
//!
//! A pipe has no half-close: a one-shot client sends its line and reads the
//! answer; the daemon sees the end when the client closes.

use std::io::{self, Read, Write};
use std::path::Path;
use windows_sys::Win32::Foundation::{
    CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_PIPE_BUSY,
    ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows_sys::Win32::Security::{GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, PIPE_ACCESS_DUPLEX, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, OPEN_EXISTING, ReadFile, WriteFile};
use windows_sys::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
    PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, PeekNamedPipe, WaitNamedPipeW,
};
use windows_sys::Win32::System::Threading::{CreateEventW, GetCurrentProcess, OpenProcessToken};

/// The pipe of the daemon serving `dir`: `\\.\pipe\qasd-<hash of dir>`.
pub fn name(dir: &Path) -> String {
    // FNV-1a of the folder, lower-cased: one daemon per data dir.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in dir.to_string_lossy().to_lowercase().bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100_0000_01b3);
    }
    format!(r"\\.\pipe\qasd-{h:016x}")
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// One end of a connected pipe.
pub struct Conn(HANDLE);

// A handle is just a reference to a kernel object.
unsafe impl Send for Conn {}
unsafe impl Sync for Conn {}

impl Drop for Conn {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

impl Conn {
    /// Connect to the pipe `name`; NotFound when nobody listens.
    pub fn connect(name: &str) -> io::Result<Conn> {
        let w = wide(name);
        for _ in 0..50 {
            let h = unsafe {
                CreateFileW(w.as_ptr(), GENERIC_READ | GENERIC_WRITE, 0, std::ptr::null(), OPEN_EXISTING, FILE_FLAG_OVERLAPPED, std::ptr::null_mut())
            };
            if h != INVALID_HANDLE_VALUE {
                return Ok(Conn(h));
            }
            if unsafe { GetLastError() } != ERROR_PIPE_BUSY {
                return Err(io::Error::last_os_error());
            }
            // Every instance is taken this instant: the listener makes another.
            unsafe { WaitNamedPipeW(w.as_ptr(), 100) };
        }
        Err(io::Error::new(io::ErrorKind::TimedOut, "pipe busy"))
    }

    pub fn try_clone(&self) -> io::Result<Conn> {
        let mut h: HANDLE = std::ptr::null_mut();
        let p = unsafe { GetCurrentProcess() };
        if unsafe { DuplicateHandle(p, self.0, p, &mut h, 0, 0, DUPLICATE_SAME_ACCESS) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Conn(h))
    }

    /// A pipe can't close one direction, so "nothing more to send" is an
    /// empty line (the daemon skips empty lines; see `idle`).
    pub fn shutdown(&self, _how: std::net::Shutdown) -> io::Result<()> {
        (&mut &*self).write_all(b"\n")
    }

    /// Connected, with nothing sent yet (a session between requests)?
    pub fn idle(&self) -> bool {
        let mut avail = 0u32;
        let ok = unsafe { PeekNamedPipe(self.0, std::ptr::null_mut(), 0, std::ptr::null_mut(), &mut avail, std::ptr::null_mut()) } != 0;
        ok && avail == 0
    }

    fn io(&self, read: bool, ptr: *mut u8, len: usize) -> io::Result<usize> {
        let ev = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        if ev.is_null() {
            return Err(io::Error::last_os_error());
        }
        let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
        ov.hEvent = ev;
        let len = len.min(u32::MAX as usize) as u32;
        let ok = unsafe {
            if read { ReadFile(self.0, ptr, len, std::ptr::null_mut(), &mut ov) } else { WriteFile(self.0, ptr, len, std::ptr::null_mut(), &mut ov) }
        };
        let mut n = 0u32;
        let r = if ok == 0 && unsafe { GetLastError() } != ERROR_IO_PENDING {
            Err(unsafe { GetLastError() })
        } else if unsafe { GetOverlappedResult(self.0, &ov, &mut n, 1) } == 0 {
            Err(unsafe { GetLastError() })
        } else {
            Ok(n as usize)
        };
        unsafe { CloseHandle(ev) };
        match r {
            Ok(n) => Ok(n),
            // The other end closed: end of stream.
            Err(ERROR_BROKEN_PIPE | ERROR_PIPE_NOT_CONNECTED | ERROR_NO_DATA) if read => Ok(0),
            Err(e) => Err(io::Error::from_raw_os_error(e as i32)),
        }
    }
}

impl Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.io(true, buf.as_mut_ptr(), buf.len())
    }
}

impl Read for &Conn {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.io(true, buf.as_mut_ptr(), buf.len())
    }
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.io(false, buf.as_ptr() as *mut u8, buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Write for &Conn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.io(false, buf.as_ptr() as *mut u8, buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The listening side: hands out one connected `Conn` per client.
pub struct Listener {
    name: Vec<u16>,
    sd: PSECURITY_DESCRIPTOR,
    first: bool,
    /// The instance created by `bind` (holding the name), until accepted on.
    pending: HANDLE,
}

unsafe impl Send for Listener {}

impl Drop for Listener {
    fn drop(&mut self) {
        if !self.pending.is_null() {
            unsafe { CloseHandle(self.pending) };
        }
        if !self.sd.is_null() {
            unsafe { LocalFree(self.sd) };
        }
    }
}

impl Listener {
    /// Fails if another process already serves `name`.
    pub fn bind(name: &str) -> io::Result<Listener> {
        let mut l = Listener { name: wide(name), sd: std::ptr::null_mut(), first: true, pending: std::ptr::null_mut() };
        // Full access for SYSTEM and the user; nobody else (not even another
        // user's elevated process through "Administrators").
        let sddl = match user_sid() {
            Some(sid) => format!("D:P(A;;GA;;;SY)(A;;GA;;;{sid})"),
            None => "D:P(A;;GA;;;SY)(A;;GA;;;OW)".into(),
        };
        let w = wide(&sddl);
        if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(w.as_ptr(), SDDL_REVISION_1, &mut l.sd, std::ptr::null_mut()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // Claim the name now, so a second daemon fails here.
        l.pending = l.instance()?;
        Ok(l)
    }

    fn instance(&mut self) -> io::Result<HANDLE> {
        let sa = SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: self.sd, bInheritHandle: 0 };
        let first = if std::mem::take(&mut self.first) { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 };
        let h = unsafe {
            CreateNamedPipeW(
                self.name.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | first,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                64 * 1024,
                64 * 1024,
                0,
                &sa,
            )
        };
        if h == INVALID_HANDLE_VALUE { Err(io::Error::last_os_error()) } else { Ok(h) }
    }

    /// Wait for the next client.
    pub fn accept(&mut self) -> io::Result<Conn> {
        let h = if self.pending.is_null() { self.instance()? } else { std::mem::replace(&mut self.pending, std::ptr::null_mut()) };
        let ev = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
        let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
        ov.hEvent = ev;
        let mut ok = unsafe { ConnectNamedPipe(h, &mut ov) } != 0;
        if !ok {
            match unsafe { GetLastError() } {
                ERROR_PIPE_CONNECTED => ok = true,
                ERROR_IO_PENDING => {
                    let mut n = 0u32;
                    ok = unsafe { GetOverlappedResult(h, &ov, &mut n, 1) } != 0;
                }
                _ => {}
            }
        }
        let err = io::Error::last_os_error();
        unsafe { CloseHandle(ev) };
        if ok {
            Ok(Conn(h))
        } else {
            unsafe { CloseHandle(h) };
            Err(err)
        }
    }

    pub fn incoming(&mut self) -> impl Iterator<Item = io::Result<Conn>> + '_ {
        std::iter::repeat_with(move || self.accept())
    }
}

/// The current user's SID, as a string (S-1-5-21-...).
fn user_sid() -> Option<String> {
    unsafe {
        let mut t: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut t) == 0 {
            return None;
        }
        let mut buf = vec![0u64; 64];
        let mut n = 0u32;
        let ok = GetTokenInformation(t, TokenUser, buf.as_mut_ptr().cast(), (buf.len() * 8) as u32, &mut n) != 0;
        CloseHandle(t);
        if !ok {
            return None;
        }
        let tu = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut s: windows_sys::core::PWSTR = std::ptr::null_mut();
        if ConvertSidToStringSidW(tu.User.Sid, &mut s) == 0 {
            return None;
        }
        let len = (0..).take_while(|&i| *s.add(i) != 0).count();
        let out = String::from_utf16_lossy(std::slice::from_raw_parts(s, len));
        LocalFree(s.cast());
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};

    #[test]
    fn round_trip_and_second_bind_fails() {
        let n = name(&std::env::temp_dir().join(format!("qasd-pipe-{}", std::process::id())));
        let mut l = Listener::bind(&n).unwrap();
        assert!(Listener::bind(&n).is_err());
        let t = std::thread::spawn(move || {
            let c = l.accept().unwrap();
            let mut line = String::new();
            BufReader::new(&c).read_line(&mut line).unwrap();
            let mut w = c.try_clone().unwrap();
            w.write_all(format!("echo {line}").as_bytes()).unwrap();
        });
        let mut c = Conn::connect(&n).unwrap();
        c.write_all(b"hello\n").unwrap();
        let mut line = String::new();
        BufReader::new(&c).read_line(&mut line).unwrap();
        assert_eq!(line, "echo hello\n");
        t.join().unwrap();
        // The daemon side closed: end of stream.
        assert_eq!(c.read(&mut [0u8; 4]).unwrap(), 0);
    }
}

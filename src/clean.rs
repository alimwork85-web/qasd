//! Cleaning up: which folders are known caches (safe to delete: the
//! program that made them makes them again), and deleting with care.
//!
//! A folder is a cache by where it is, never by its name: a fixed list of
//! known places under %LOCALAPPDATA% and the Windows folder. Any folder
//! that merely happens to be called "Cache" is not one.

/// Known caches, under %LOCALAPPDATA%; `*` stands for any one folder (a
/// browser profile).
const LOCAL: &[&str] = &[
    r"Temp",
    r"Google\Chrome\User Data\*\Cache",
    r"Google\Chrome\User Data\*\Code Cache",
    r"Google\Chrome\User Data\*\GPUCache",
    r"Google\Chrome\User Data\ShaderCache",
    r"Google\Chrome\User Data\GrShaderCache",
    r"Microsoft\Edge\User Data\*\Cache",
    r"Microsoft\Edge\User Data\*\Code Cache",
    r"Microsoft\Edge\User Data\*\GPUCache",
    r"Microsoft\Edge\User Data\ShaderCache",
    r"Microsoft\Edge\User Data\GrShaderCache",
    r"BraveSoftware\Brave-Browser\User Data\*\Cache",
    r"BraveSoftware\Brave-Browser\User Data\*\Code Cache",
    r"BraveSoftware\Brave-Browser\User Data\*\GPUCache",
    r"BraveSoftware\Brave-Browser\User Data\ShaderCache",
    r"BraveSoftware\Brave-Browser\User Data\GrShaderCache",
    r"Mozilla\Firefox\Profiles\*\cache2",
    r"Microsoft\Windows\INetCache",
    r"D3DSCache",
    r"CrashDumps",
];

/// Known caches under the Windows folder.
const WINDOWS: &[&str] = &[r"Temp"];

/// %LOCALAPPDATA% and the Windows folder, as Windows shows them.
fn roots() -> (String, String) {
    let local = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| format!(r"{}\AppData\Local", crate::sys::display(crate::sys::home().as_bytes())));
    let windir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    (local, windir)
}

/// If `path` (C:\x\y) is a known cache or inside one: the length of the
/// cache folder's own path (equal to `path.len()` for the folder itself).
pub fn cache_root(path: &str) -> Option<usize> {
    let (local, windir) = roots();
    cache_root_in(path, &local, &windir)
}

fn cache_root_in(path: &str, local: &str, windir: &str) -> Option<usize> {
    let path = path.trim_end_matches('\\');
    for (root, list) in [(local, LOCAL), (windir, WINDOWS)] {
        let root = root.trim_end_matches('\\');
        let Some(rest) = strip_prefix_ci(path, root).and_then(|r| r.strip_prefix('\\')) else { continue };
        let segs: Vec<&str> = rest.split('\\').collect();
        for pat in list {
            let want: Vec<&str> = pat.split('\\').collect();
            if segs.len() >= want.len() && want.iter().zip(&segs).all(|(w, s)| *w == "*" || w.eq_ignore_ascii_case(s)) {
                let len = root.len() + want.len() + segs[..want.len()].iter().map(|s| s.len()).sum::<usize>();
                return Some(len);
            }
        }
    }
    None
}

fn strip_prefix_ci<'a>(s: &'a str, p: &str) -> Option<&'a str> {
    (s.len() >= p.len() && s.is_char_boundary(p.len()) && s[..p.len()].eq_ignore_ascii_case(p)).then(|| &s[p.len()..])
}

/// Is `path` a known cache folder, or inside one?
pub fn is_cache(path: &str) -> bool {
    cache_root(path).is_some()
}

/// How a path would be deleted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Way {
    /// To the Recycle Bin: it can be restored.
    Recycle,
    /// Inside a known cache: gone for good (the Recycle Bin would free nothing).
    Permanent,
    /// A known cache folder itself: what is in it goes for good, the
    /// folder stays (its program expects it).
    EmptyCache,
    /// A symbolic link or junction: the link only, never what it points to.
    Link,
}

/// What a deletion did.
#[derive(Debug, Default)]
pub struct Report {
    /// Files removed for good, and their bytes.
    pub files: u64,
    pub bytes: u64,
    /// What could not be removed (in use, or no access): skipped.
    pub skipped: Vec<String>,
    /// The whole thing failed.
    pub error: Option<String>,
}

/// The folders deleting must never touch, as Windows shows them: these
/// themselves (`exact`), and these with everything inside (`deep`).
struct Guard {
    exact: Vec<String>,
    deep: Vec<String>,
}

fn guard() -> Guard {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let home = crate::sys::display(crate::sys::home().as_bytes());
    let mut exact: Vec<String> = ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432", "ProgramData", "PUBLIC", "APPDATA", "LOCALAPPDATA"].iter().filter_map(|k| env(k)).collect();
    for sub in ["", r"\AppData", r"\AppData\Local", r"\AppData\Roaming", r"\AppData\LocalLow"] {
        exact.push(format!("{home}{sub}"));
    }
    if let Some(users) = std::path::Path::new(&home).parent() {
        exact.push(users.to_string_lossy().into_owned());
    }
    let deep = vec![env("SystemRoot").unwrap_or_else(|| r"C:\Windows".into())];
    Guard { exact, deep }
}

/// May `path` (C:\x\y, as shown) be deleted, and how? Err: why not, in
/// Arabic, for the person.
pub fn check(path: &str) -> Result<Way, String> {
    let long = long_name(path).ok_or_else(|| "لم يُعثر على المسار".to_string())?;
    let md = std::fs::symlink_metadata(&long).map_err(|_| "لم يُعثر على المسار".to_string())?;
    check_in(&long, is_link(&md), &guard(), cache_root(&long))
}

fn check_in(path: &str, link: bool, g: &Guard, cache: Option<usize>) -> Result<Way, String> {
    let p = path.trim_end_matches('\\');
    let b = p.as_bytes();
    // Only full local paths: C:\...
    if b.len() < 2 || !b[0].is_ascii_alphabetic() || b[1] != b':' || (b.len() > 2 && b[2] != b'\\') {
        return Err("مسار غير مدعوم".into());
    }
    if p.len() <= 2 {
        return Err("جذر القرص محمي".into());
    }
    let segs: Vec<&str> = p[3..].split('\\').collect();
    if segs.iter().any(|s| s.is_empty() || *s == "." || *s == "..") {
        return Err("مسار غير مدعوم".into());
    }
    // A short name (PROGRA~1) still there after expanding: refuse, don't guess.
    let short = |s: &str| s.len() <= 12 && s.split('.').next().and_then(|n| n.rsplit_once('~')).is_some_and(|(_, d)| !d.is_empty() && d.bytes().all(|c| c.is_ascii_digit()));
    if segs.iter().any(|s| short(s)) {
        return Err("اسم مختصر لم يُعرف اسمه الكامل".into());
    }
    if segs.iter().any(|s| s.eq_ignore_ascii_case("$Recycle.Bin") || s.eq_ignore_ascii_case("System Volume Information")) {
        return Err("مجلد نظام محمي".into());
    }
    if g.exact.iter().any(|e| e.trim_end_matches('\\').eq_ignore_ascii_case(p)) {
        return Err("مجلد أساسي في النظام محمي".into());
    }
    let in_windows = g.deep.iter().any(|d| strip_prefix_ci(p, d.trim_end_matches('\\')).is_some_and(|r| r.is_empty() || r.starts_with('\\')));
    match cache {
        Some(n) if n >= p.len() && !link => Ok(Way::EmptyCache),
        Some(_) if !link => Ok(Way::Permanent),
        // Inside Windows: only its known caches (Windows\Temp).
        _ if in_windows => Err("مجلد Windows محمي".into()),
        _ if link => Ok(Way::Link),
        _ => Ok(Way::Recycle),
    }
}

/// The long form of every part of `path` (PROGRA~1 -> Program Files).
fn long_name(path: &str) -> Option<String> {
    use windows_sys::Win32::Storage::FileSystem::GetLongPathNameW;
    let w: Vec<u16> = path.encode_utf16().chain([0]).collect();
    let mut buf = vec![0u16; 1024];
    let n = unsafe { GetLongPathNameW(w.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    (n > 0 && n < buf.len()).then(|| String::from_utf16_lossy(&buf[..n]))
}

/// A symbolic link, junction or other reparse point: never gone into.
fn is_link(md: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    md.file_type().is_symlink() || md.file_attributes() & 0x400 != 0 // FILE_ATTRIBUTE_REPARSE_POINT
}

/// Remove a link itself: a junction or folder link with remove_dir, a file link with remove_file.
fn remove_link(p: &std::path::Path, md: &std::fs::Metadata) -> std::io::Result<()> {
    use std::os::windows::fs::MetadataExt;
    if md.file_attributes() & 0x10 != 0 { std::fs::remove_dir(p) } else { std::fs::remove_file(p) } // FILE_ATTRIBUTE_DIRECTORY
}

/// Delete `path` the way `check` said.
pub fn delete(path: &str, way: Way) -> Report {
    let mut r = Report::default();
    let p = std::path::Path::new(path);
    match way {
        Way::Recycle => r.error = recycle(path).err(),
        Way::Link => {
            let gone = std::fs::symlink_metadata(p).and_then(|md| remove_link(p, &md));
            r.error = gone.err().map(|e| e.to_string());
        }
        Way::Permanent => remove_tree(p, false, &mut r),
        Way::EmptyCache => remove_tree(p, true, &mut r),
    }
    r
}

/// To the Recycle Bin, as Explorer does (no dialogs of its own).
pub fn recycle(path: &str) -> Result<(), String> {
    use windows_sys::Win32::UI::Shell::*;
    let from: Vec<u16> = path.encode_utf16().chain([0, 0]).collect();
    let mut op: SHFILEOPSTRUCTW = unsafe { std::mem::zeroed() };
    op.wFunc = FO_DELETE;
    op.pFrom = from.as_ptr();
    op.fFlags = (FOF_ALLOWUNDO | FOF_NO_UI) as u16;
    let rc = unsafe { SHFileOperationW(&mut op) };
    if rc != 0 || op.fAnyOperationsAborted != 0 || std::path::Path::new(path).exists() {
        return Err(format!("تعذّر النقل إلى سلة المحذوفات: قد يكون ملف فيه مفتوحاً الآن (رمز {rc})"));
    }
    Ok(())
}

/// Remove a folder's contents (and the folder, unless `keep`) for good,
/// file by file: what can't go (in use) is skipped and listed. Links are
/// removed as links, never followed.
fn remove_tree(p: &std::path::Path, keep: bool, r: &mut Report) {
    let Ok(md) = std::fs::symlink_metadata(p) else { return };
    let skip = |r: &mut Report, _: std::io::Error| r.skipped.push(p.display().to_string());
    if is_link(&md) {
        if let Err(e) = remove_link(p, &md) {
            skip(r, e);
        }
        return;
    }
    if md.is_dir() {
        let before = r.skipped.len();
        match std::fs::read_dir(p) {
            Ok(list) => {
                for e in list.flatten() {
                    remove_tree(&e.path(), false, r);
                }
            }
            Err(e) => return skip(r, e),
        }
        if !keep
            && r.skipped.len() == before
            && let Err(e) = std::fs::remove_dir(p)
        {
            skip(r, e);
        }
        return;
    }
    let len = md.len();
    let mut gone = std::fs::remove_file(p);
    if gone.is_err() && md.permissions().readonly() {
        let mut perm = md.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perm.set_readonly(false);
        let _ = std::fs::set_permissions(p, perm);
        gone = std::fs::remove_file(p);
    }
    match gone {
        Ok(()) => {
            r.files += 1;
            r.bytes += len;
        }
        Err(e) => skip(r, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const L: &str = r"C:\Users\u\AppData\Local";
    const W: &str = r"C:\Windows";

    #[test]
    fn known_places_only() {
        let t = |p: &str| cache_root_in(p, L, W);
        assert_eq!(t(r"C:\Users\u\AppData\Local\Temp"), Some(r"C:\Users\u\AppData\Local\Temp".len()));
        assert_eq!(t(r"c:\users\u\appdata\local\temp\x\y.tmp"), Some(r"C:\Users\u\AppData\Local\Temp".len()));
        let chrome = r"C:\Users\u\AppData\Local\Google\Chrome\User Data\Profile 1\Code Cache";
        assert_eq!(t(chrome), Some(chrome.len()));
        assert!(t(r"C:\Users\u\AppData\Local\Mozilla\Firefox\Profiles\ab.default\cache2\entries").is_some());
        assert!(t(r"C:\Windows\Temp").is_some());
        // Named Cache, but not a known place.
        assert_eq!(t(r"C:\Users\u\Documents\Cache"), None);
        assert_eq!(t(r"C:\Users\u\AppData\Local\SomeApp\Cache"), None);
        // The parents of a cache are not caches.
        assert_eq!(t(r"C:\Users\u\AppData\Local"), None);
        assert_eq!(t(r"C:\Users\u\AppData\Local\Google\Chrome\User Data\Default"), None);
        assert_eq!(t(r"C:\Users\u\AppData\Local\Tempest"), None);
        assert_eq!(t(r"C:\Windows\System32"), None);
    }

    fn g() -> Guard {
        let exact = [r"C:\Program Files", r"C:\Users\u", r"C:\Users\u\AppData", r"C:\Users\u\AppData\Local", r"C:\Users"];
        Guard { exact: exact.iter().map(|s| s.to_string()).collect(), deep: vec![W.into()] }
    }

    #[test]
    fn protections_hold() {
        let c = |p: &str| check_in(p, false, &g(), cache_root_in(p, L, W));
        for bad in [
            r"C:\",
            r"C:",
            r"c:\program files",
            r"C:\Users",
            r"C:\Users\u",
            r"C:\Users\u\AppData\",
            r"C:\Users\u\AppData\Local",
            r"C:\Windows",
            r"C:\Windows\System32\drivers",
            r"D:\$Recycle.Bin\S-1-5",
            r"C:\System Volume Information",
            r"C:\PROGRA~1",
            r"C:\Users\u\..\x",
            r"\\server\share\x",
            r"relative\x",
        ] {
            assert!(c(bad).is_err(), "{bad} must be refused");
        }
        assert_eq!(c(r"C:\Users\u\Documents\old.zip"), Ok(Way::Recycle));
        assert_eq!(c(r"C:\Users\u\AppData\Local\SomeApp"), Ok(Way::Recycle));
        assert_eq!(c(r"C:\Program Files\SomeApp"), Ok(Way::Recycle));
        assert_eq!(c(r"C:\Users\u\AppData\Local\Temp"), Ok(Way::EmptyCache));
        assert_eq!(c(r"C:\Users\u\AppData\Local\Temp\x"), Ok(Way::Permanent));
        assert_eq!(c(r"C:\Windows\Temp\x.log"), Ok(Way::Permanent));
        // A ~ that is not a short name.
        assert_eq!(c(r"C:\Users\u\my~notes.txt"), Ok(Way::Recycle));
        assert_eq!(check_in(r"C:\Users\u\link", true, &g(), None), Ok(Way::Link));
        assert!(check_in(r"C:\Windows\link", true, &g(), None).is_err());
    }

    /// Permanent removal on a scratch folder: links are not followed, files in use are skipped.
    #[test]
    fn remove_tree_scratch() {
        use std::os::windows::fs::OpenOptionsExt;
        let base = std::env::temp_dir().join(format!("qasd-clean-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (victim, outside) = (base.join("victim"), base.join("outside"));
        std::fs::create_dir_all(victim.join("sub")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(victim.join("a.txt"), b"12345").unwrap();
        std::fs::write(victim.join("sub").join("b.txt"), b"123").unwrap();
        std::fs::write(outside.join("keep.txt"), b"keep").unwrap();
        // A junction inside the victim pointing outside it.
        let ok = std::process::Command::new("cmd").args(["/c", "mklink", "/J"]).arg(victim.join("jn")).arg(&outside).output().unwrap().status.success();
        assert!(ok, "mklink /J");
        // A file held open with no sharing: in use.
        let held = victim.join("held.txt");
        std::fs::write(&held, b"x").unwrap();
        let lock = std::fs::OpenOptions::new().read(true).share_mode(0).open(&held).unwrap();
        let mut r = Report::default();
        remove_tree(&victim, true, &mut r);
        assert_eq!((r.files, r.bytes), (2, 8));
        assert_eq!(r.skipped.len(), 1, "{:?}", r.skipped);
        assert!(outside.join("keep.txt").exists(), "a junction was followed");
        assert!(!victim.join("jn").exists() && !victim.join("sub").exists());
        assert!(victim.exists(), "kept");
        drop(lock);
        let mut r = Report::default();
        remove_tree(&victim, false, &mut r);
        assert!(r.skipped.is_empty() && !victim.exists());
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// The real check: short names are expanded before the guard looks.
    #[test]
    fn short_names_expand() {
        let pf = std::env::var("ProgramFiles").unwrap();
        assert!(check(&pf).is_err());
        if std::path::Path::new(r"C:\PROGRA~1").exists() {
            assert!(check(r"C:\PROGRA~1").is_err());
        }
        assert!(check(r"C:\").is_err());
    }
}

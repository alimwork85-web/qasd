//! Installed programs, as Windows' "Installed apps" lists them: the
//! Uninstall keys in the registry (HKLM 64- and 32-bit, HKCU). Store
//! (MSIX) apps are not there and are left out.
//!
//! Removing one runs its own uninstaller (`UninstallString`), as Windows
//! does. What it may leave behind is only suggested, never deleted here.

use windows_sys::Win32::System::Registry::*;

const UNINSTALL: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall";

#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub name: String,
    pub publisher: String,
    pub version: String,
    /// Bytes, as the installer estimated them.
    pub size: Option<u64>,
    /// The file its icon is in (an .exe or .ico), if it says.
    pub icon: Option<String>,
    /// Where it was installed, if it says.
    pub location: Option<String>,
    /// The command that removes it.
    pub uninstall: String,
    /// Which key it is (`HKLM64\<sub>`, `HKLM32\<sub>`, `HKCU\<sub>`): to
    /// tell whether it is still installed.
    pub key: String,
}

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// The three places programs register, with how to open each.
fn hives() -> [(HKEY, u32, &'static str); 3] {
    [(HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY, "HKLM64"), (HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY, "HKLM32"), (HKEY_CURRENT_USER, 0, "HKCU")]
}

fn get_str(h: HKEY, sub: &str, name: &str) -> Option<String> {
    let (sub, name) = (w(sub), w(name));
    let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ;
    let mut n = 0u32;
    if unsafe { RegGetValueW(h, sub.as_ptr(), name.as_ptr(), flags, std::ptr::null_mut(), std::ptr::null_mut(), &mut n) } != 0 || n < 4 {
        return None;
    }
    let mut buf = vec![0u16; n as usize / 2 + 1];
    let mut n = (buf.len() * 2) as u32;
    if unsafe { RegGetValueW(h, sub.as_ptr(), name.as_ptr(), flags, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut n) } != 0 {
        return None;
    }
    let s = String::from_utf16_lossy(&buf[..(n as usize / 2).saturating_sub(1)]);
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn get_dword(h: HKEY, sub: &str, name: &str) -> Option<u32> {
    let (sub, name) = (w(sub), w(name));
    let (mut v, mut n) = (0u32, 4u32);
    (unsafe { RegGetValueW(h, sub.as_ptr(), name.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), (&raw mut v).cast(), &mut n) } == 0).then_some(v)
}

/// Every installed program with a name and an uninstaller, by name.
/// Updates, system components and duplicates (one program in two views)
/// are left out.
pub fn list() -> Vec<Program> {
    let mut out: Vec<Program> = Vec::new();
    for (root, view, tag) in hives() {
        let mut h: HKEY = std::ptr::null_mut();
        if unsafe { RegOpenKeyExW(root, w(UNINSTALL).as_ptr(), 0, KEY_READ | view, &mut h) } != 0 {
            continue;
        }
        let mut i = 0;
        loop {
            let mut name = [0u16; 512];
            let mut n = name.len() as u32;
            let rc = unsafe { RegEnumKeyExW(h, i, name.as_mut_ptr(), &mut n, std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
            i += 1;
            if rc == 259 {
                break; // ERROR_NO_MORE_ITEMS
            }
            if rc != 0 {
                continue;
            }
            let sub = String::from_utf16_lossy(&name[..n as usize]);
            let (Some(display), Some(uninstall)) = (get_str(h, &sub, "DisplayName"), get_str(h, &sub, "UninstallString")) else { continue };
            if get_dword(h, &sub, "SystemComponent") == Some(1) || get_str(h, &sub, "ParentKeyName").is_some() || get_str(h, &sub, "ReleaseType").is_some_and(|t| t.contains("Update") || t == "Hotfix") {
                continue;
            }
            let icon = get_str(h, &sub, "DisplayIcon").map(|s| icon_file(&s)).filter(|s| !s.is_empty());
            let p = Program {
                name: display,
                publisher: get_str(h, &sub, "Publisher").unwrap_or_default(),
                version: get_str(h, &sub, "DisplayVersion").unwrap_or_default(),
                size: get_dword(h, &sub, "EstimatedSize").filter(|&k| k > 0).map(|k| k as u64 * 1024),
                icon,
                location: get_str(h, &sub, "InstallLocation").map(|s| s.trim_matches('"').trim_end_matches('\\').to_string()).filter(|s| s.len() > 3),
                uninstall,
                key: format!(r"{tag}\{sub}"),
            };
            if !out.iter().any(|o| o.name == p.name && o.version == p.version) {
                out.push(p);
            }
        }
        unsafe { RegCloseKey(h) };
    }
    out.sort_by_key(|p| p.name.to_lowercase());
    out
}

/// `"C:\x\app.exe",0` -> `C:\x\app.exe`.
fn icon_file(s: &str) -> String {
    let s = s.trim();
    let s = match s.rsplit_once(',') {
        Some((f, n)) if n.trim().trim_start_matches('-').bytes().all(|c| c.is_ascii_digit()) => f,
        _ => s,
    };
    s.trim().trim_matches('"').to_string()
}

/// Is the program with this key still registered?
pub fn installed(key: &str) -> bool {
    list().iter().any(|p| p.key == key)
}

/// An UninstallString as (program, arguments), for ShellExecute: a quoted
/// path, or everything up to the first `.exe`.
pub fn split_command(cmd: &str) -> (String, String) {
    let cmd = cmd.trim();
    if let Some(rest) = cmd.strip_prefix('"')
        && let Some(end) = rest.find('"')
    {
        return (rest[..end].to_string(), rest[end + 1..].trim().to_string());
    }
    match cmd.to_ascii_lowercase().find(".exe") {
        Some(i) => (cmd[..i + 4].to_string(), cmd[i + 4..].trim().to_string()),
        None => match cmd.split_once(' ') {
            Some((f, a)) => (f.to_string(), a.trim().to_string()),
            None => (cmd.to_string(), String::new()),
        },
    }
}

/// The program's name without its version or architecture:
/// "7-Zip 23.01 (x64)" -> "7-Zip".
fn bare_name(name: &str) -> String {
    let name = name.split(" (").next().unwrap_or(name);
    let name = name.split(" - ").next().unwrap_or(name);
    let mut words: Vec<&str> = name.split_whitespace().collect();
    while words.len() > 1 && words.last().is_some_and(|w| w.bytes().any(|c| c.is_ascii_digit()) || ["x64", "x86", "64-bit", "32-bit"].contains(&w.to_ascii_lowercase().as_str())) {
        words.pop();
    }
    words.join(" ")
}

/// Folders the program may have left behind (it is gone from the list
/// by now): named after it in %LOCALAPPDATA%, %APPDATA% and %ProgramData%
/// (or under its publisher's folder there), and its install folder if it
/// is still there. Only suggestions: the person chooses.
pub fn leftovers(p: &Program) -> Vec<String> {
    let roots: Vec<String> = ["LOCALAPPDATA", "APPDATA", "ProgramData"].iter().filter_map(|k| std::env::var(k).ok()).collect();
    leftovers_in(p, &roots, &list())
}

fn leftovers_in(p: &Program, roots: &[String], still: &[Program]) -> Vec<String> {
    let bare = bare_name(&p.name);
    let publisher = bare_name(&p.publisher).trim_end_matches(['.', ',']).to_string();
    let mut names = vec![bare.clone()];
    // "Mozilla Firefox" by Mozilla: also "Firefox" (in Mozilla\).
    if let Some(rest) = bare.strip_prefix(&format!("{publisher} ")).filter(|_| !publisher.is_empty()) {
        names.push(rest.to_string());
    }
    names.retain(|n| n.chars().count() >= 3);
    // A name another installed program also goes by is not this one's.
    let taken = |n: &str| still.iter().any(|o| o.key != p.key && (bare_name(&o.name).eq_ignore_ascii_case(n) || o.location.as_deref().is_some_and(|l| l.rsplit('\\').next().is_some_and(|d| d.eq_ignore_ascii_case(n)))));
    let mut out: Vec<String> = Vec::new();
    for root in roots {
        for n in &names {
            if taken(n) {
                continue;
            }
            let mut cands = vec![format!(r"{root}\{n}")];
            if !publisher.is_empty() {
                cands.push(format!(r"{root}\{publisher}\{n}"));
            }
            out.extend(cands.into_iter().filter(|c| std::path::Path::new(c).is_dir()));
        }
    }
    if let Some(loc) = &p.location
        && std::path::Path::new(loc).is_dir()
        && !still.iter().any(|o| o.key != p.key && o.location.as_deref().is_some_and(|l| l.eq_ignore_ascii_case(loc)))
    {
        out.push(loc.clone());
    }
    // Never anything protected (a root, Program Files itself...).
    out.retain(|c| crate::clean::check(c).is_ok());
    out.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_split() {
        assert_eq!(split_command(r#""C:\Program Files\X\unins000.exe" /S"#), (r"C:\Program Files\X\unins000.exe".into(), "/S".into()));
        assert_eq!(split_command(r"MsiExec.exe /X{1234}"), ("MsiExec.exe".into(), "/X{1234}".into()));
        assert_eq!(split_command(r"C:\Program Files\X\uninst.exe --remove"), (r"C:\Program Files\X\uninst.exe".into(), "--remove".into()));
        assert_eq!(icon_file(r#""C:\x\app.exe",0"#), r"C:\x\app.exe");
        assert_eq!(icon_file(r"C:\x\app.ico"), r"C:\x\app.ico");
    }

    #[test]
    fn bare_names() {
        assert_eq!(bare_name("7-Zip 23.01 (x64)"), "7-Zip");
        assert_eq!(bare_name("Mozilla Firefox (x64 en-US)"), "Mozilla Firefox");
        assert_eq!(bare_name("Python 3.14.0 (64-bit)"), "Python");
        assert_eq!(bare_name("Notepad++"), "Notepad++");
    }

    #[test]
    fn leftovers_scratch() {
        let base = std::env::temp_dir().join(format!("qasd-left-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (local, roaming) = (base.join("Local"), base.join("Roaming"));
        for d in [local.join("Firefox"), local.join("Mozilla").join("Firefox"), roaming.join("Mozilla Firefox"), local.join("Other"), base.join("inst")] {
            std::fs::create_dir_all(d).unwrap();
        }
        let s = |p: &std::path::Path| p.to_string_lossy().into_owned();
        let me = Program { name: "Mozilla Firefox (x64 en-US)".into(), publisher: "Mozilla".into(), version: "1".into(), size: None, icon: None, location: Some(s(&base.join("inst"))), uninstall: String::new(), key: "HKCU\\ff".into() };
        let mut found = leftovers_in(&me, &[s(&local), s(&roaming)], &[]);
        found.sort();
        let mut want = vec![s(&local.join("Firefox")), s(&local.join("Mozilla").join("Firefox")), s(&roaming.join("Mozilla Firefox")), s(&base.join("inst"))];
        want.sort();
        assert_eq!(found, want);
        // Another installed program named Firefox: its folders aren't offered.
        let other = Program { name: "Firefox".into(), key: "HKCU\\other".into(), location: None, ..me.clone() };
        let found = leftovers_in(&me, &[s(&local), s(&roaming)], &[other]);
        assert!(!found.iter().any(|f| f.ends_with(r"\Firefox")), "{found:?}");
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// The real registry: at least one program, each with a name and an uninstaller.
    #[test]
    fn lists_real_programs() {
        let all = list();
        assert!(!all.is_empty());
        assert!(all.iter().all(|p| !p.name.is_empty() && !p.uninstall.is_empty()));
    }
}

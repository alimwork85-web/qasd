//! Cleaning up: which folders are known caches (safe to delete: the
//! program that made them makes them again).
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
}

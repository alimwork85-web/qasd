//! The Start menu's apps (Store and desktop alike): what
//! `shell:AppsFolder` lists, by the names Windows shows (in its language),
//! and finding them by name. Shared by the window, the CLI and MCP.

/// An app: opening `shell:AppsFolder\<id>` starts it.
#[derive(Debug, Clone, PartialEq)]
pub struct App {
    pub name: String,
    pub id: String,
}

impl App {
    /// What to open to start it.
    pub fn target(&self) -> String {
        format!(r"shell:AppsFolder\{}", self.id)
    }
}

/// Every app in the Start menu.
pub fn list() -> Vec<App> {
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree};
    use windows::Win32::UI::Shell::{BHID_EnumItems, FOLDERID_AppsFolder, IEnumShellItems, IShellItem, KF_FLAG_DEFAULT, SHGetKnownFolderItem, SIGDN, SIGDN_NORMALDISPLAY, SIGDN_PARENTRELATIVEPARSING};
    let name = |it: &IShellItem, k: SIGDN| unsafe {
        let p = it.GetDisplayName(k).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as _));
        s
    };
    let mut out = Vec::new();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let Ok(folder) = SHGetKnownFolderItem::<_, IShellItem>(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None) else { return out };
        let Ok(list) = folder.BindToHandler::<_, IEnumShellItems>(None, &BHID_EnumItems) else { return out };
        loop {
            let mut one = [None];
            let mut n = 0;
            if list.Next(&mut one, Some(&mut n)).is_err() || n == 0 {
                break;
            }
            let Some(it) = one[0].take() else { break };
            if let (Some(shown), Some(id)) = (name(&it, SIGDN_NORMALDISPLAY), name(&it, SIGDN_PARENTRELATIVEPARSING)) {
                // Some names carry direction marks ("\u{200e}الرسام").
                let shown: String = shown.chars().filter(|c| !matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}')).collect();
                out.push(App { name: shown.trim().to_string(), id });
            }
        }
    }
    out
}

/// Indexes of the names `q` finds, best first: the words as typed, their
/// other Arabic spellings (ة/ه, أ/ا...) and the other keyboard layout.
pub fn rank(names: &[&str], q: &str) -> Vec<usize> {
    let Ok(query) = crate::Query::parse(q, &crate::sys::home()) else { return Vec::new() };
    let mut forms = vec![query.tokens.clone()];
    forms.extend(query.alt.iter().cloned());
    if !query.layout.is_empty() {
        forms.push(query.layout.clone());
    }
    let forms: Vec<crate::Query> = forms.into_iter().map(|tokens| crate::Query { tokens, alt: Vec::new(), layout: Vec::new(), ..query.clone() }).collect();
    let mut hits: Vec<(i32, usize)> = names
        .iter()
        .enumerate()
        .filter_map(|(i, n)| {
            let p = format!("/{n}");
            forms.iter().filter_map(|f| f.match_path(p.as_bytes(), crate::walk::KIND_FILE, 0, 0)).max().map(|s| (s, i))
        })
        .collect();
    hits.sort_by(|x, y| y.0.cmp(&x.0).then_with(|| names[x.1].len().cmp(&names[y.1].len())));
    hits.into_iter().map(|(_, i)| i).collect()
}

/// The apps `q` finds, best first, at most `max`: by the name Windows
/// shows, then by the English words in the app's id, so `notepad` finds
/// المفكرة (Microsoft.WindowsNotepad_...).
pub fn find<'a>(apps: &'a [App], q: &str, max: usize) -> Vec<&'a App> {
    let names: Vec<&str> = apps.iter().map(|a| a.name.as_str()).collect();
    let mut order = rank(&names, q);
    let words = query_words(q);
    if !words.is_empty() {
        let mut more: Vec<usize> = (0..apps.len()).filter(|i| !order.contains(i) && id_matches(&id_words(&apps[*i].id), &words)).collect();
        more.sort_by_key(|&i| apps[i].name.len());
        order.extend(more);
    }
    order.into_iter().take(max).map(|i| &apps[i]).collect()
}

/// Words too common in app ids to tell one app from another.
const GENERIC: &[&str] = &["windows", "microsoft", "exe", "app", "apps", "com", "msc", "lnk", "url", "inc", "corp", "desktop", "client"];

/// The English words in an app's id, lowercase:
/// `Microsoft.WindowsNotepad_8wekyb3d8bbwe!App` -> [notepad];
/// `{GUID}\WindowsPowerShell\v1.0\powershell.exe` -> [power, powershell, shell].
fn id_words(id: &str) -> Vec<String> {
    // A packaged app: its family name, before the publisher hash.
    let id = id.split('!').next().unwrap_or(id);
    let id = if id.contains('\\') { id } else { id.split('_').next().unwrap_or(id) };
    let mut out: Vec<String> = Vec::new();
    for part in id.split(['.', '\\', '_', ' ', '-', '{', '}']) {
        if part.is_empty() || !part.bytes().all(|c| c.is_ascii_alphabetic()) {
            continue;
        }
        // CamelCase -> its words, and the whole part too (PowerShell -> power, shell, powershell).
        let mut cur = String::new();
        let mut words = Vec::new();
        for c in part.chars() {
            if c.is_ascii_uppercase() && cur.chars().last().is_some_and(|p| p.is_ascii_lowercase()) {
                words.push(std::mem::take(&mut cur));
            }
            cur.push(c);
        }
        words.push(cur);
        let mut words: Vec<String> = words.into_iter().map(|w| w.to_ascii_lowercase()).filter(|w| !GENERIC.contains(&w.as_str())).collect();
        if words.len() > 1 {
            words.push(words.concat());
        }
        out.extend(words.into_iter().filter(|w| w.len() >= 3));
    }
    out.sort();
    out.dedup();
    out
}

/// A query's plain English words (3 letters or more), or none if it has
/// anything else (filters, other scripts): only those try the ids.
fn query_words(q: &str) -> Vec<String> {
    let words: Vec<String> = q.split_whitespace().map(|w| w.to_ascii_lowercase()).collect();
    if words.is_empty() || words.iter().any(|w| w.len() < 3 || !w.bytes().all(|c| c.is_ascii_alphabetic())) {
        return Vec::new();
    }
    words
}

/// Does every query word start one of the id's words? (No typo
/// tolerance here: ids are full of near misses.)
fn id_matches(id: &[String], words: &[String]) -> bool {
    words.iter().all(|w| id.iter().any(|i| i.starts_with(w.as_str())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_by_name() {
        let apps = vec![App { name: "الحاسبة".into(), id: "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".into() }, App { name: "Paint".into(), id: "Microsoft.Paint_8wekyb3d8bbwe!App".into() }];
        assert_eq!(find(&apps, "الحاسبه", 4)[0].name, "الحاسبة");
        assert_eq!(find(&apps, "paint", 4)[0].target(), r"shell:AppsFolder\Microsoft.Paint_8wekyb3d8bbwe!App");
        assert!(find(&apps, "zzzz", 4).is_empty());
    }

    #[test]
    fn english_words_from_ids() {
        assert_eq!(id_words("Microsoft.WindowsNotepad_8wekyb3d8bbwe!App"), ["notepad"]);
        assert_eq!(id_words("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"), ["calculator"]);
        assert_eq!(id_words(r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe"), ["power", "powershell", "shell"]);
        let apps = vec![
            App { name: "المفكرة".into(), id: "Microsoft.WindowsNotepad_8wekyb3d8bbwe!App".into() },
            App { name: "الحاسبة".into(), id: "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".into() },
            App { name: "الرسام".into(), id: "Microsoft.Paint_8wekyb3d8bbwe!App".into() },
            App { name: "Notepad++".into(), id: r"{6D809377-6AF0-444B-8957-A3773F02200E}\Notepad++\notepad++.exe".into() },
        ];
        let names = |q: &str| find(&apps, q, 4).iter().map(|a| a.name.clone()).collect::<Vec<_>>();
        assert_eq!(names("notepad"), ["Notepad++", "المفكرة"]);
        assert_eq!(names("calculator"), ["الحاسبة"]);
        assert_eq!(names("calc"), ["الحاسبة"]);
        assert_eq!(names("paint"), ["الرسام"]);
        // No noise: generic words, short words and near misses find nothing by id.
        assert!(names("windows").is_empty());
        assert!(!names("no").contains(&"المفكرة".to_string()));
        assert!(!names("notpad").contains(&"المفكرة".to_string()));
    }
}

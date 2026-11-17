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

/// The apps `q` finds, best first, at most `max`.
pub fn find<'a>(apps: &'a [App], q: &str, max: usize) -> Vec<&'a App> {
    let names: Vec<&str> = apps.iter().map(|a| a.name.as_str()).collect();
    rank(&names, q).into_iter().take(max).map(|i| &apps[i]).collect()
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
}

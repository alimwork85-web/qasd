//! The text of PDF and Office files, for content search. Release builds
//! abort on a panic, so a parser crash would take the daemon down with it:
//! the text is extracted by a child `qasd doc-text <path>` instead, killed
//! past DOC_TIMEOUT, the way Windows Search runs its filters.

use std::io::Read;
use std::time::{Duration, Instant};

/// Largest document read.
pub const DOC_MAX: u64 = 64 << 20;
/// Most text kept from one document (content search's file limit).
const TEXT_MAX: usize = crate::content::MAX_REPO_FILE as usize;
const DOC_TIMEOUT: Duration = Duration::from_secs(20);
/// Lines past this many bytes are broken at the next space: results show
/// a line, and content indexing takes very long lines for minified code.
const LINE: usize = 200;
const EXTS: &[&str] = &["pdf", "docx", "xlsx", "pptx", "doc", "xls", "ppt"];

/// Whether `name` (or a path ending in it) is a document read through
/// `text`.
pub fn is_doc(name: &[u8]) -> bool {
    let name = name.rsplit(|&b| b == b'/' || b == b'\\').next().unwrap_or(name);
    name.iter().rposition(|&b| b == b'.').filter(|&p| p > 0).is_some_and(|p| EXTS.iter().any(|x| x.as_bytes().eq_ignore_ascii_case(&name[p + 1..])))
}

/// The text of the document at `path` (the index's form), from a child
/// process; None if it could not be read in time.
pub fn text(path: &[u8]) -> Option<Vec<u8>> {
    use std::os::windows::process::CommandExt;
    let exe = std::env::current_exe().ok()?;
    // Only qasd.exe has the doc-text command (not a test or another program
    // using this crate).
    if !exe.file_stem().is_some_and(|s| s.eq_ignore_ascii_case("qasd")) {
        return None;
    }
    let mut child = std::process::Command::new(exe)
        .arg("doc-text")
        .arg(crate::sys::os(path))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .spawn()
        .ok()?;
    let mut out = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        out.by_ref().take(TEXT_MAX as u64).read_to_end(&mut buf).ok().map(|_| buf)
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(s)) if s.success() => return reader.join().ok().flatten(),
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if start.elapsed() > DOC_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

/// `qasd doc-text <path>`: print the document's text.
pub fn main(path: &str) -> ! {
    use std::io::Write;
    let text = extract(&crate::sys::bytes(path));
    let ok = text.is_some_and(|t| std::io::stdout().lock().write_all(t.as_bytes()).is_ok());
    std::process::exit(if ok { 0 } else { 1 })
}

/// The document's text, in this process. Never recalls a cloud-only file.
pub fn extract(path: &[u8]) -> Option<String> {
    let mut f = crate::content::open_regular(path)?;
    let mut bytes = Vec::new();
    f.by_ref().take(DOC_MAX + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > DOC_MAX {
        return None;
    }
    let name = String::from_utf8_lossy(path).to_ascii_lowercase();
    let ext = name.rsplit('.').next()?;
    let raw = if ext == "pdf" {
        use unicode_normalization::UnicodeNormalization;
        // Arabic often comes out as presentation forms (ﻣﺤﻤﺪ): NFKC makes
        // them the letters people type.
        pdf_oxide::PdfDocument::from_bytes(bytes).ok()?.extract_all_text().ok()?.nfkc().collect()
    } else {
        use office_oxide::DocumentFormat as F;
        let format = match ext {
            "docx" => F::Docx,
            "xlsx" => F::Xlsx,
            "pptx" => F::Pptx,
            "doc" => F::Doc,
            "xls" => F::Xls,
            "ppt" => F::Ppt,
            _ => return None,
        };
        office_oxide::Document::from_reader(std::io::Cursor::new(bytes), format).ok()?.plain_text()
    };
    Some(lines(&raw))
}

/// `s` with page breaks as new lines, NULs dropped, and lines past LINE
/// bytes broken at the next space; at most TEXT_MAX bytes.
fn lines(s: &str) -> String {
    let mut out = String::with_capacity(s.len().min(TEXT_MAX));
    let mut run = 0;
    for c in s.chars() {
        let c = match c {
            '\x0c' => '\n',
            '\0' => continue,
            c => c,
        };
        if out.len() + c.len_utf8() > TEXT_MAX {
            break;
        }
        if c == ' ' && run >= LINE {
            out.push('\n');
            run = 0;
            continue;
        }
        out.push(c);
        run = if c == '\n' { 0 } else { run + c.len_utf8() };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doc_names() {
        assert!(is_doc(b"/C:/x/Report.PDF"));
        assert!(is_doc(b"a.docx"));
        assert!(!is_doc(b"a.txt"));
        assert!(!is_doc(b".pdf"));
    }

    #[test]
    fn breaks_long_lines() {
        let s = "كلمة ".repeat(100);
        let out = lines(&s);
        assert!(out.lines().all(|l| l.len() <= LINE + 20));
        assert_eq!(out.split_whitespace().count(), 100);
        assert_eq!(lines("a\x0cb\0c"), "a\nbc");
    }
}

//! qasdw: Qasd's search window. Lives in the tray; Alt+Q shows a search box
//! over whatever you're doing, results update as you type, Enter opens.
//!
//! Plain Win32: one popup window drawn by hand (double-buffered GDI) with a
//! single EDIT control for input. Right-click (or the menu key) on a
//! result shows the shell's own menu for the file, as Explorer does. Searches go to the daemon over its named
//! pipe on a worker thread, so typing never waits on one.

#![windows_subsystem = "windows"]

use qasd::pipe::Conn;
use qasd::sys;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL};
use windows_sys::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::Controls::{HIMAGELIST, ILD_NORMAL, ImageList_GetIcon};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, GetDpiForWindow, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const CLASS: &str = "QasdSearchWindow";
const HOTKEY_ID: i32 = 1;
/// Results arrived from the worker (lparam: Box<Answer>).
const WM_RESULTS: u32 = WM_APP + 1;
/// Tray icon events.
const WM_TRAY: u32 = WM_APP + 2;
/// Another qasdw was started: show this one.
const WM_SHOW_ME: u32 = WM_APP + 3;
const TIMER_RETRY: usize = 1;
const LIMIT: usize = 60;
const MAX_ROWS: usize = 8;
const CMD_SHOW: usize = 100;
const CMD_EXIT: usize = 101;
const CMD_STARTUP: usize = 102;
const EM_SETCUEBANNER: u32 = 0x1501;
const EM_GETSEL: u32 = 0x00B0;
const EM_SETSEL: u32 = 0x00B1;

fn w(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// One result row.
struct Item {
    /// As Windows shows it: C:\x\y.txt
    path: String,
    name: String,
    dir: String,
    is_dir: bool,
    /// A grep hit: line number and text.
    line: Option<(u64, String)>,
}

struct Answer {
    seq: u64,
    items: Vec<Item>,
    error: Option<String>,
    took: Duration,
}

enum Job {
    /// Start the daemon (and load its index) before the first keystroke.
    Warm,
    Search(u64, String),
    Opened(String),
}

struct Theme {
    bg: u32,
    input: u32,
    text: u32,
    sub: u32,
    sel: u32,
    line: u32,
    accent: u32,
}

/// COLORREF from 0xRRGGBB.
const fn rgb(c: u32) -> u32 {
    ((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF)
}

fn theme(light: bool) -> Theme {
    if light {
        Theme {
            bg: rgb(0xF9F9F9),
            input: rgb(0xFFFFFF),
            text: rgb(0x1B1B1B),
            sub: rgb(0x5F5F5F),
            sel: rgb(0xE3EEFB),
            line: rgb(0xE5E5E5),
            accent: rgb(0x2563EB),
        }
    } else {
        Theme {
            bg: rgb(0x202020),
            input: rgb(0x2B2B2B),
            text: rgb(0xF3F3F3),
            sub: rgb(0x9E9E9E),
            sel: rgb(0x2E3A4B),
            line: rgb(0x333333),
            accent: rgb(0x4CC2FF),
        }
    }
}

fn system_light() -> bool {
    let key = w(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let val = w("AppsUseLightTheme");
    let mut v = 1u32;
    let mut n = 4u32;
    unsafe { RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), val.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), (&raw mut v).cast(), &mut n) };
    v != 0
}

struct Fonts {
    input: HFONT,
    name: HFONT,
    sub: HFONT,
    glyph: HFONT,
}

fn font(px: i32, weight: i32, face: &str) -> HFONT {
    let f = w(face);
    unsafe { CreateFontW(-px, 0, 0, 0, weight, 0, 0, 0, DEFAULT_CHARSET as u32, 0, 0, CLEARTYPE_QUALITY as u32, 0, f.as_ptr()) }
}

impl Fonts {
    fn new(dpi: u32) -> Fonts {
        let s = |px: i32| px * dpi as i32 / 96;
        let glyph_face = if Path::new(r"C:\Windows\Fonts\SegoeIcons.ttf").exists() { "Segoe Fluent Icons" } else { "Segoe MDL2 Assets" };
        Fonts { input: font(s(20), 400, "Segoe UI"), name: font(s(15), 600, "Segoe UI"), sub: font(s(12), 400, "Segoe UI"), glyph: font(s(18), 400, glyph_face) }
    }
    fn free(&self) {
        unsafe {
            DeleteObject(self.input);
            DeleteObject(self.name);
            DeleteObject(self.sub);
            DeleteObject(self.glyph);
        }
    }
}

struct App {
    hwnd: HWND,
    edit: HWND,
    dpi: u32,
    fonts: Fonts,
    light: bool,
    edit_brush: HBRUSH,
    items: Vec<Item>,
    sel: usize,
    top: usize,
    hover: Option<usize>,
    status: String,
    seq: u64,
    shown_seq: u64,
    jobs: Sender<Job>,
    icons: HashMap<String, i32>,
    images: HIMAGELIST,
    query: String,
    tray_icon: HICON,
    taskbar_created: u32,
    /// A right-click menu (or what it runs) is up: losing focus to it
    /// doesn't hide the window.
    menu_up: bool,
}

static APP: AtomicPtr<App> = AtomicPtr::new(std::ptr::null_mut());
/// The newest search asked for: the worker skips older ones.
static LATEST: AtomicU64 = AtomicU64::new(0);

fn app() -> &'static mut App {
    // Safety: set once before the message loop; only the UI thread uses it.
    unsafe { &mut *APP.load(Ordering::Relaxed) }
}

fn scale(v: i32) -> i32 {
    v * app().dpi as i32 / 96
}

// Layout, at 96 DPI.
const WIDTH: i32 = 720;
const HEAD: i32 = 60;
const ROW: i32 = 50;
const FOOT: i32 = 30;
const PAD: i32 = 10;

fn data_dir() -> PathBuf {
    qasd::default_dir(&sys::home())
}

/// qasd.exe next to this exe: it runs the daemon.
fn daemon_exe() -> PathBuf {
    std::env::current_exe().map(|p| p.with_file_name("qasd.exe")).unwrap_or_else(|_| PathBuf::from("qasd.exe"))
}

fn worker(jobs: Receiver<Job>, hwnd: usize) {
    let mut conn: Option<(Conn, BufReader<Conn>)> = None;
    let mut ask = |req: &Value| -> Result<Value, String> {
        for attempt in 0..2 {
            if conn.is_none() {
                let c = qasd::pipe::connect_or_start(&data_dir(), &daemon_exe()).map_err(|e| format!("تعذّر تشغيل الخدمة: {e}"))?;
                let r = c.try_clone().map_err(|e| e.to_string())?;
                conn = Some((c, BufReader::new(r)));
            }
            let (c, r) = conn.as_mut().unwrap();
            let mut line = String::new();
            let ok = c.write_all(format!("{req}\n").as_bytes()).is_ok() && r.read_line(&mut line).is_ok_and(|n| n > 0);
            if ok {
                return serde_json::from_str(&line).map_err(|e| e.to_string());
            }
            conn = None;
            if attempt == 1 {
                return Err("انقطع الاتصال بالخدمة".into());
            }
        }
        unreachable!()
    };
    while let Ok(job) = jobs.recv() {
        match job {
            Job::Warm => {
                let _ = ask(&json!({"op": "status"}));
            }
            Job::Opened(p) => {
                let _ = ask(&json!({"op": "open", "path": p}));
            }
            Job::Search(seq, q) => {
                if seq < LATEST.load(Ordering::Relaxed) {
                    continue;
                }
                let t = Instant::now();
                let mut a = Answer { seq, items: Vec::new(), error: None, took: Duration::ZERO };
                if !q.trim().is_empty() {
                    match ask(&json!({"q": q, "limit": LIMIT, "boost": true, "cwd": sys::home()})) {
                        Ok(v) if v["ok"] == true => a.items = items(&v),
                        Ok(v) => a.error = Some(v["error"].as_str().unwrap_or("error").to_string()),
                        Err(e) => a.error = Some(e),
                    }
                }
                a.took = t.elapsed();
                let p = Box::into_raw(Box::new(a));
                if unsafe { PostMessageW(hwnd as HWND, WM_RESULTS, 0, p as isize) } == 0 {
                    drop(unsafe { Box::from_raw(p) });
                }
            }
        }
    }
}

fn item(path: &str, is_dir: bool, line: Option<(u64, String)>) -> Item {
    let path = sys::display(path.as_bytes());
    let cut = path.rfind('\\').map_or(0, |i| i + 1);
    let (dir, name) = (path[..cut.saturating_sub(1).max(if cut == 3 { 3 } else { 0 })].to_string(), path[cut..].to_string());
    let dir = if dir.is_empty() { path[..cut].to_string() } else { dir };
    Item { name: if name.is_empty() { path.clone() } else { name }, dir, path, is_dir, line }
}

fn items(v: &Value) -> Vec<Item> {
    let mut out = Vec::new();
    for h in v["hits"].as_array().into_iter().flatten() {
        out.push(item(h["path"].as_str().unwrap_or(""), h["kind"] == "dir", None));
    }
    for f in v["files"].as_array().into_iter().flatten() {
        let p = f["path"].as_str().unwrap_or("");
        for m in f["matches"].as_array().into_iter().flatten().take(3) {
            let text = m["text"].as_str().unwrap_or("").trim().to_string();
            out.push(item(p, false, Some((m["line"].as_u64().unwrap_or(0), text))));
        }
    }
    out.truncate(LIMIT * 3);
    out
}

/// "3 نتائج": Arabic counted nouns, Western digits.
fn results_ar(n: usize) -> String {
    match n {
        0 => "لا نتائج".into(),
        1 => "نتيجة واحدة".into(),
        2 => "نتيجتان".into(),
        3..=10 => format!("{n} نتائج"),
        _ => format!("{n} نتيجة"),
    }
}

fn edit_text(edit: HWND) -> String {
    let n = unsafe { GetWindowTextLengthW(edit) } as usize;
    let mut b = vec![0u16; n + 1];
    let got = unsafe { GetWindowTextW(edit, b.as_mut_ptr(), b.len() as i32) } as usize;
    String::from_utf16_lossy(&b[..got])
}

fn rows_shown() -> usize {
    app().items.len().min(MAX_ROWS)
}

fn window_height() -> i32 {
    let a = app();
    let rows = rows_shown() as i32;
    let body = if rows > 0 { rows * ROW + PAD } else { 0 };
    let foot = if a.query.trim().is_empty() && a.items.is_empty() { 0 } else { FOOT };
    scale(HEAD + body + foot)
}

fn layout() {
    let a = app();
    let mut r: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetWindowRect(a.hwnd, &mut r) };
    let h = window_height();
    unsafe { SetWindowPos(a.hwnd, std::ptr::null_mut(), r.left, r.top, scale(WIDTH), h, SWP_NOZORDER | SWP_NOACTIVATE) };
    let eh = scale(30);
    unsafe { MoveWindow(a.edit, scale(52), (scale(HEAD) - eh) / 2, scale(WIDTH) - scale(52) - scale(18), eh, 1) };
    unsafe { InvalidateRect(a.hwnd, std::ptr::null(), 0) };
}

fn set_fonts(dpi: u32) {
    let a = app();
    if a.dpi == dpi && !a.fonts.input.is_null() {
        return;
    }
    a.fonts.free();
    a.dpi = dpi;
    a.fonts = Fonts::new(dpi);
    unsafe { SendMessageW(a.edit, WM_SETFONT, a.fonts.input as usize, 1) };
}

fn show() {
    let a = app();
    let mut pt = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut pt) };
    let mon = unsafe { MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST) };
    let mut mi: MONITORINFO = unsafe { std::mem::zeroed() };
    mi.cbSize = size_of::<MONITORINFO>() as u32;
    unsafe { GetMonitorInfoW(mon, &mut mi) };
    let (mut dx, mut dy) = (96u32, 96u32);
    unsafe { GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) };
    set_fonts(dx);
    let light = system_light();
    if light != a.light {
        a.light = light;
        set_theme();
    }
    let wr = mi.rcWork;
    let width = scale(WIDTH);
    let x = wr.left + (wr.right - wr.left - width) / 2;
    let y = wr.top + (wr.bottom - wr.top) / 4;
    unsafe {
        SetWindowPos(a.hwnd, HWND_TOPMOST, x, y, width, window_height(), SWP_SHOWWINDOW);
    }
    layout();
    unsafe {
        ShowWindow(a.hwnd, SW_SHOW);
        SetForegroundWindow(a.hwnd);
        SetFocus(a.edit);
        SendMessageW(a.edit, EM_SETSEL, 0, -1);
    }
    // Fresh results for what's typed (files may have changed meanwhile).
    search();
}

fn hide() {
    unsafe { ShowWindow(app().hwnd, SW_HIDE) };
}

fn set_theme() {
    let a = app();
    let t = theme(a.light);
    unsafe {
        if !a.edit_brush.is_null() {
            DeleteObject(a.edit_brush);
        }
        a.edit_brush = CreateSolidBrush(t.input);
        // Dark scrollbars/caret etc. where the system draws them.
        let dark: i32 = (!a.light) as i32;
        DwmSetWindowAttribute(a.hwnd, 20, (&raw const dark).cast(), 4); // DWMWA_USE_IMMERSIVE_DARK_MODE
        InvalidateRect(a.edit, std::ptr::null(), 1);
    }
}

fn search() {
    let a = app();
    a.query = edit_text(a.edit);
    a.seq += 1;
    LATEST.store(a.seq, Ordering::Relaxed);
    let _ = a.jobs.send(Job::Search(a.seq, a.query.clone()));
}

fn on_results(ans: Box<Answer>) {
    let a = app();
    if ans.seq < a.shown_seq || ans.seq != a.seq {
        return;
    }
    a.shown_seq = ans.seq;
    unsafe { KillTimer(a.hwnd, TIMER_RETRY) };
    match ans.error {
        Some(e) => {
            // The very first index is still being built: try again shortly.
            if e.contains("index") || e.contains("listed") {
                a.status = format!("جارٍ فهرسة الأقراص لأول مرة… ({e})");
                unsafe { SetTimer(a.hwnd, TIMER_RETRY, 1000, None) };
            } else {
                a.status = e;
            }
            a.items.clear();
        }
        None => {
            a.items = ans.items;
            a.status = if a.query.trim().is_empty() {
                String::new()
            } else {
                format!("{}  ·  {:.1} ms", results_ar(a.items.len()), ans.took.as_secs_f64() * 1000.0)
            };
        }
    }
    a.sel = 0;
    a.top = 0;
    a.hover = None;
    layout();
}

fn ensure_visible() {
    let a = app();
    if a.sel < a.top {
        a.top = a.sel;
    } else if a.sel >= a.top + MAX_ROWS {
        a.top = a.sel + 1 - MAX_ROWS;
    }
}

fn move_sel(d: isize) {
    let a = app();
    if a.items.is_empty() {
        return;
    }
    let n = a.items.len() as isize;
    a.sel = (a.sel as isize + d).clamp(0, n - 1) as usize;
    ensure_visible();
    unsafe { InvalidateRect(a.hwnd, std::ptr::null(), 0) };
}

fn open(i: usize, reveal: bool) {
    let a = app();
    let Some(it) = a.items.get(i) else { return };
    let p = w(&it.path);
    unsafe {
        if reveal {
            let pidl = ILCreateFromPathW(p.as_ptr());
            if !pidl.is_null() {
                SHOpenFolderAndSelectItems(pidl, 0, std::ptr::null(), 0);
                ILFree(pidl);
            }
        } else {
            let verb = w("open");
            ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), p.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL);
        }
    }
    // Ranked higher next time ("boost").
    let _ = a.jobs.send(Job::Opened(it.path.clone()));
    hide();
}

fn copy_path(i: usize) {
    let a = app();
    let Some(it) = a.items.get(i) else { return };
    let text: Vec<u16> = it.path.encode_utf16().chain([0]).collect();
    unsafe {
        if OpenClipboard(a.hwnd) == 0 {
            return;
        }
        EmptyClipboard();
        let h = GlobalAlloc(GMEM_MOVEABLE, text.len() * 2);
        if !h.is_null() {
            let p = GlobalLock(h) as *mut u16;
            std::ptr::copy_nonoverlapping(text.as_ptr(), p, text.len());
            GlobalUnlock(h);
            SetClipboardData(13, h); // CF_UNICODETEXT
        }
        CloseClipboard();
    }
    a.status = "نُسخ المسار".into();
    unsafe { InvalidateRect(a.hwnd, std::ptr::null(), 0) };
}

/// Ctrl+Backspace: delete the word before the caret (a single-line EDIT
/// would insert a box character).
fn delete_word() {
    let a = app();
    let t: Vec<char> = edit_text(a.edit).chars().collect();
    let mut end = t.len();
    while end > 0 && t[end - 1] == ' ' {
        end -= 1;
    }
    while end > 0 && !matches!(t[end - 1], ' ' | '\\' | '/' | '.' | '_' | '-') {
        end -= 1;
    }
    let s: String = t[..end].iter().collect();
    let ws = w(&s);
    unsafe {
        SetWindowTextW(a.edit, ws.as_ptr());
        SendMessageW(a.edit, EM_SETSEL, end, end as isize);
    }
}

/// Handles keys typed in the search box before the EDIT sees them.
fn key(vk: u32) -> bool {
    let a = app();
    let ctrl = unsafe { GetKeyState(VK_CONTROL as i32) } < 0;
    match vk as u16 {
        VK_ESCAPE => hide(),
        VK_RETURN => open(a.sel, ctrl),
        VK_DOWN => move_sel(1),
        VK_UP => move_sel(-1),
        VK_NEXT => move_sel(MAX_ROWS as isize),
        VK_PRIOR => move_sel(-(MAX_ROWS as isize)),
        VK_BACK if ctrl => delete_word(),
        VK_APPS => context_menu(a.sel, None),
        VK_F10 if unsafe { GetKeyState(VK_SHIFT as i32) } < 0 => context_menu(a.sel, None),
        k if ctrl && k == b'C' as u16 && !selection_in_edit() => copy_path(a.sel),
        _ => return false,
    }
    true
}

fn selection_in_edit() -> bool {
    let (mut s, mut e) = (0u32, 0u32);
    unsafe { SendMessageW(app().edit, EM_GETSEL, (&raw mut s) as usize, (&raw mut e) as isize) };
    s != e
}

/// The system image list index of a file's icon (by extension; the file
/// itself for programs and shortcuts, whose icons are their own).
fn icon_index(it: &Item) -> i32 {
    let a = app();
    let ext = it.name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
    let own = matches!(ext.as_str(), "exe" | "lnk" | "ico" | "url" | "appref-ms");
    let key = if it.is_dir {
        "<dir>".to_string()
    } else if own {
        it.path.clone()
    } else {
        format!(".{ext}")
    };
    if let Some(&i) = a.icons.get(&key) {
        return i;
    }
    let mut fi: SHFILEINFOW = unsafe { std::mem::zeroed() };
    let (p, attrs, flags) = if own {
        (w(&it.path), 0, SHGFI_SYSICONINDEX)
    } else if it.is_dir {
        (w("folder"), FILE_ATTRIBUTE_DIRECTORY, SHGFI_SYSICONINDEX | SHGFI_USEFILEATTRIBUTES)
    } else {
        (w(&format!("x.{ext}")), FILE_ATTRIBUTE_NORMAL, SHGFI_SYSICONINDEX | SHGFI_USEFILEATTRIBUTES)
    };
    let list = unsafe { SHGetFileInfoW(p.as_ptr(), attrs, &mut fi, size_of::<SHFILEINFOW>() as u32, flags) };
    if a.images == 0 && list != 0 {
        // The 32 px system list.
        a.images = list as HIMAGELIST;
    }
    let i = if list == 0 { -1 } else { fi.iIcon };
    a.icons.insert(key, i);
    i
}

fn draw_text(dc: HDC, s: &str, r: &mut RECT, flags: u32) {
    // (DrawTextW reads an empty string's pointer anyway: never hand it a dangling one.)
    if s.is_empty() {
        return;
    }
    let ws: Vec<u16> = s.encode_utf16().chain([0]).collect();
    unsafe { DrawTextW(dc, ws.as_ptr(), ws.len() as i32 - 1, r, flags | DT_NOPREFIX | DT_SINGLELINE) };
}

fn fill(dc: HDC, r: &RECT, color: u32) {
    unsafe {
        let b = CreateSolidBrush(color);
        FillRect(dc, r, b);
        DeleteObject(b);
    }
}

fn round_fill(dc: HDC, r: &RECT, color: u32, radius: i32) {
    unsafe {
        let b = CreateSolidBrush(color);
        let ob = SelectObject(dc, b);
        let op = SelectObject(dc, GetStockObject(NULL_PEN));
        RoundRect(dc, r.left, r.top, r.right + 1, r.bottom + 1, radius, radius);
        SelectObject(dc, ob);
        SelectObject(dc, op);
        DeleteObject(b);
    }
}

fn paint() {
    let a = app();
    let t = theme(a.light);
    let mut ps: PAINTSTRUCT = unsafe { std::mem::zeroed() };
    let hdc = unsafe { BeginPaint(a.hwnd, &mut ps) };
    let mut rc: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(a.hwnd, &mut rc) };
    let (cw, ch) = (rc.right, rc.bottom);
    unsafe {
        let dc = CreateCompatibleDC(hdc);
        let bmp = CreateCompatibleBitmap(hdc, cw, ch);
        let old = SelectObject(dc, bmp);
        SetBkMode(dc, TRANSPARENT as i32);
        fill(dc, &rc, t.bg);

        // The search box.
        let head = scale(HEAD);
        let mut boxr = RECT { left: scale(8), top: scale(8), right: cw - scale(8), bottom: head - scale(8) };
        round_fill(dc, &boxr, t.input, scale(12));
        SelectObject(dc, a.fonts.glyph);
        SetTextColor(dc, t.accent);
        boxr.left = scale(20);
        boxr.right = scale(48);
        draw_text(dc, "\u{E721}", &mut boxr, DT_CENTER | DT_VCENTER);

        // Results.
        let mut y = head;
        if !a.items.is_empty() {
            fill(dc, &RECT { left: 0, top: y, right: cw, bottom: y + 1 }, t.line);
            y += scale(PAD) / 2;
        }
        let end = (a.top + MAX_ROWS).min(a.items.len());
        for i in a.top..end {
            let it = &a.items[i];
            let row = RECT { left: scale(8), top: y, right: cw - scale(8), bottom: y + scale(ROW) };
            if i == a.sel {
                round_fill(dc, &row, t.sel, scale(10));
                let bar = RECT { left: row.left, top: row.top + scale(14), right: row.left + scale(3), bottom: row.bottom - scale(14) };
                round_fill(dc, &bar, t.accent, scale(3));
            } else if a.hover == Some(i) {
                round_fill(dc, &row, t.input, scale(10));
            }
            let isz = scale(32).min(32 * a.dpi as i32 / 96);
            let idx = icon_index(it);
            if idx >= 0 && a.images != 0 {
                let hi = ImageList_GetIcon(a.images, idx, ILD_NORMAL);
                if !hi.is_null() {
                    let s = scale(28);
                    DrawIconEx(dc, scale(22), y + (scale(ROW) - s) / 2, hi, s, s, 0, std::ptr::null_mut(), DI_NORMAL);
                    DestroyIcon(hi);
                }
            }
            let _ = isz;
            let left = scale(62);
            let right = cw - scale(20);
            // Name, then the folder (or a grep hit's line).
            SelectObject(dc, a.fonts.name);
            SetTextColor(dc, t.text);
            let mut r1 = RECT { left, top: y + scale(6), right, bottom: y + scale(27) };
            draw_text(dc, &it.name, &mut r1, DT_LEFT | DT_END_ELLIPSIS);
            SelectObject(dc, a.fonts.sub);
            SetTextColor(dc, t.sub);
            let mut r2 = RECT { left, top: y + scale(27), right, bottom: y + scale(45) };
            match &it.line {
                Some((n, text)) => draw_text(dc, &format!("{n}:  {text}"), &mut r2, DT_LEFT | DT_END_ELLIPSIS),
                None => draw_text(dc, &it.dir, &mut r2, DT_LEFT | DT_PATH_ELLIPSIS),
            }
            y += scale(ROW);
        }

        // Footer: count and keys, read right to left.
        if ch - y >= scale(FOOT) - 2 {
            let fy = ch - scale(FOOT);
            fill(dc, &RECT { left: 0, top: fy, right: cw, bottom: fy + 1 }, t.line);
            SelectObject(dc, a.fonts.sub);
            SetTextColor(dc, t.sub);
            let mut fr = RECT { left: scale(16), top: fy, right: cw - scale(16), bottom: ch };
            draw_text(dc, &a.status, &mut fr, DT_RIGHT | DT_VCENTER | DT_RTLREADING | DT_END_ELLIPSIS);
            if !a.items.is_empty() {
                let mut kr = RECT { left: scale(16), top: fy, right: cw / 2 + scale(80), bottom: ch };
                draw_text(dc, "Enter فتح   ·   Ctrl+Enter المجلد   ·   Ctrl+C نسخ المسار", &mut kr, DT_LEFT | DT_VCENTER | DT_RTLREADING);
            }
        }

        BitBlt(hdc, 0, 0, cw, ch, dc, 0, 0, SRCCOPY);
        SelectObject(dc, old);
        DeleteObject(bmp);
        DeleteDC(dc);
        EndPaint(a.hwnd, &ps);
    }
}

/// The result row under a client point.
fn row_at(y: i32) -> Option<usize> {
    let a = app();
    let top = scale(HEAD) + scale(PAD) / 2;
    if y < top {
        return None;
    }
    let i = a.top + ((y - top) / scale(ROW)) as usize;
    (i < a.items.len() && i < a.top + MAX_ROWS).then_some(i)
}

fn tray(add: bool) {
    let a = app();
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = a.hwnd;
    nid.uID = 1;
    nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = a.tray_icon;
    let tip = w("قصد — بحث سريع (Alt+Q)");
    nid.szTip[..tip.len().min(127)].copy_from_slice(&tip[..tip.len().min(127)]);
    unsafe { Shell_NotifyIconW(if add { NIM_ADD } else { NIM_DELETE }, &nid) };
}

fn startup_key() -> (Vec<u16>, Vec<u16>) {
    (w(r"Software\Microsoft\Windows\CurrentVersion\Run"), w("Qasd"))
}

fn starts_with_windows() -> bool {
    let (k, v) = startup_key();
    let mut n = 0u32;
    unsafe {
        RegGetValueW(HKEY_CURRENT_USER, k.as_ptr(), v.as_ptr(), windows_sys::Win32::System::Registry::RRF_RT_REG_SZ, std::ptr::null_mut(), std::ptr::null_mut(), &mut n)
            == 0
    }
}

/// Start (or stop starting) with Windows: HKCU\...\Run\Qasd.
fn set_startup(on: bool) {
    use windows_sys::Win32::System::Registry::{KEY_SET_VALUE, REG_SZ, RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW};
    let (k, v) = startup_key();
    let mut h = std::ptr::null_mut();
    unsafe {
        if RegOpenKeyExW(HKEY_CURRENT_USER, k.as_ptr(), 0, KEY_SET_VALUE, &mut h) != 0 {
            return;
        }
        if on {
            let exe = std::env::current_exe().unwrap_or_default();
            let val = w(&format!("\"{}\"", exe.display()));
            RegSetValueExW(h, v.as_ptr(), 0, REG_SZ, val.as_ptr().cast(), (val.len() * 2) as u32);
        } else {
            RegDeleteValueW(h, v.as_ptr());
        }
        RegCloseKey(h);
    }
}

fn tray_menu() {
    let a = app();
    unsafe {
        let m = CreatePopupMenu();
        let open_label = w("فتح البحث\tAlt+Q");
        let start = w("التشغيل مع ويندوز");
        let exit = w("خروج");
        AppendMenuW(m, MF_STRING, CMD_SHOW, open_label.as_ptr());
        AppendMenuW(m, MF_STRING | if starts_with_windows() { MF_CHECKED } else { 0 }, CMD_STARTUP, start.as_ptr());
        AppendMenuW(m, MF_SEPARATOR, 0, std::ptr::null());
        AppendMenuW(m, MF_STRING, CMD_EXIT, exit.as_ptr());
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        SetForegroundWindow(a.hwnd);
        let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_LAYOUTRTL, pt.x, pt.y, 0, a.hwnd, std::ptr::null());
        DestroyMenu(m);
        match cmd as usize {
            CMD_SHOW => show(),
            CMD_STARTUP => set_startup(!starts_with_windows()),
            CMD_EXIT => {
                tray(false);
                PostQuitMessage(0);
            }
            _ => {}
        }
    }
}

/// The shell's menu for the file being shown, while it is up: submenus like
/// "Open with" and "Send to" fill themselves through these messages.
mod shell_menu {
    use std::cell::RefCell;
    use windows::Win32::Foundation::{HWND as WHWND, LPARAM as WLPARAM, LRESULT as WLRESULT, POINT as WPOINT, WPARAM as WWPARAM};
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        CMF_EXTENDEDVERBS, CMF_NORMAL, CMIC_MASK_PTINVOKE, CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX, IContextMenu, IContextMenu2,
        IContextMenu3, IShellFolder, ILFree, SHBindToParent, SHParseDisplayName,
    };
    use windows::Win32::UI::WindowsAndMessaging::HMENU as WHMENU;
    use windows::core::{Interface, PCSTR, PCWSTR};

    thread_local! {
        static ACTIVE: RefCell<Option<IContextMenu>> = const { RefCell::new(None) };
    }

    /// (Not in the bindings: the same bit as SEE_MASK_UNICODE.)
    const CMIC_MASK_UNICODE: u32 = 0x4000;

    /// The first id the shell's items take in our menu.
    pub const FIRST: u32 = 100;

    pub struct Menu {
        cm: IContextMenu,
        pidl: *mut ITEMIDLIST,
    }

    impl Drop for Menu {
        fn drop(&mut self) {
            ACTIVE.with_borrow_mut(|a| *a = None);
            unsafe { ILFree(Some(self.pidl)) };
        }
    }

    /// The shell's items for `path`, added to `menu` from position `at`.
    pub fn fill(path: &str, hwnd: *mut core::ffi::c_void, menu: *mut core::ffi::c_void, at: u32, extended: bool) -> Option<Menu> {
        let w: Vec<u16> = path.encode_utf16().chain([0]).collect();
        unsafe {
            let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
            SHParseDisplayName(PCWSTR(w.as_ptr()), None, &mut pidl, 0, None).ok()?;
            let mut child: *mut ITEMIDLIST = std::ptr::null_mut();
            let folder: IShellFolder = match SHBindToParent(pidl, Some(&mut child)) {
                Ok(f) => f,
                Err(_) => {
                    ILFree(Some(pidl));
                    return None;
                }
            };
            let cm: IContextMenu = match folder.GetUIObjectOf(WHWND(hwnd), &[child as *const ITEMIDLIST], None) {
                Ok(c) => c,
                Err(_) => {
                    ILFree(Some(pidl));
                    return None;
                }
            };
            let flags = CMF_NORMAL | if extended { CMF_EXTENDEDVERBS } else { 0 };
            if cm.QueryContextMenu(WHMENU(menu), at, FIRST, 0x7FFF, flags).is_err() {
                ILFree(Some(pidl));
                return None;
            }
            ACTIVE.with_borrow_mut(|a| *a = Some(cm.clone()));
            Some(Menu { cm, pidl })
        }
    }

    impl Menu {
        /// Run the shell's item `id` (as TrackPopupMenu returned it).
        pub fn invoke(&self, id: u32, hwnd: *mut core::ffi::c_void, x: i32, y: i32) {
            let off = (id - FIRST) as usize;
            let mut info: CMINVOKECOMMANDINFOEX = unsafe { std::mem::zeroed() };
            info.cbSize = size_of::<CMINVOKECOMMANDINFOEX>() as u32;
            info.fMask = CMIC_MASK_UNICODE | CMIC_MASK_PTINVOKE;
            info.hwnd = WHWND(hwnd);
            info.lpVerb = PCSTR(off as *const u8);
            info.lpVerbW = PCWSTR(off as *const u16);
            info.nShow = 1; // SW_SHOWNORMAL
            info.ptInvoke = WPOINT { x, y };
            unsafe {
                let _ = self.cm.InvokeCommand(&info as *const CMINVOKECOMMANDINFOEX as *const CMINVOKECOMMANDINFO);
                // "Copy"/"Cut" put the shell's data on the clipboard: make it
                // stay there after this menu is gone.
                let _ = windows::Win32::System::Ole::OleFlushClipboard();
            }
        }
    }

    /// Hand a menu message to the shell's menu while it is up (owner-drawn
    /// items, submenus filled on demand). Some(result) when it took it.
    pub fn message(msg: u32, wp: usize, lp: isize) -> Option<isize> {
        ACTIVE.with_borrow(|a| {
            let cm = a.as_ref()?;
            unsafe {
                if let Ok(cm3) = cm.cast::<IContextMenu3>() {
                    let mut r = WLRESULT(0);
                    cm3.HandleMenuMsg2(msg, WWPARAM(wp), WLPARAM(lp), Some(&mut r)).ok()?;
                    return Some(r.0);
                }
                let cm2 = cm.cast::<IContextMenu2>().ok()?;
                cm2.HandleMenuMsg(msg, WWPARAM(wp), WLPARAM(lp)).ok()?;
                Some(0)
            }
        })
    }
}

const CMD_REVEAL: u32 = 1;
const CMD_COPY_PATH: u32 = 2;

/// The right-click menu for result `i`: ours on top (open its location,
/// copy its path), then the shell's own, as in Explorer. At the
/// mouse, or under the row when opened from the keyboard.
fn context_menu(i: usize, at: Option<POINT>) {
    let a = app();
    let Some(it) = a.items.get(i) else { return };
    let path = it.path.clone();
    a.sel = i;
    unsafe { InvalidateRect(a.hwnd, std::ptr::null(), 0) };
    let pt = at.unwrap_or_else(|| {
        let row = (i.saturating_sub(a.top)) as i32;
        let mut p = POINT { x: scale(62), y: scale(HEAD) + scale(PAD) / 2 + (row + 1) * scale(ROW) - scale(8) };
        unsafe { ClientToScreen(a.hwnd, &mut p) };
        p
    });
    unsafe {
        let m = CreatePopupMenu();
        let reveal = w("فتح موقع الملف");
        let copy = w("نسخ المسار");
        AppendMenuW(m, MF_STRING, CMD_REVEAL as usize, reveal.as_ptr());
        AppendMenuW(m, MF_STRING, CMD_COPY_PATH as usize, copy.as_ptr());
        AppendMenuW(m, MF_SEPARATOR, 0, std::ptr::null());
        // Shift+right-click: the extended items too, as in Explorer.
        let extended = GetKeyState(VK_SHIFT as i32) < 0;
        let shell = shell_menu::fill(&path, a.hwnd, m, 3, extended);
        a.menu_up = true;
        let cmd = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_LAYOUTRTL, pt.x, pt.y, 0, a.hwnd, std::ptr::null()) as u32;
        match cmd {
            0 => {}
            CMD_REVEAL => open(i, true),
            CMD_COPY_PATH => copy_path(i),
            id if id >= shell_menu::FIRST => {
                if let Some(s) = &shell {
                    s.invoke(id, a.hwnd, pt.x, pt.y);
                }
                // The file may be gone or renamed: done here.
                hide();
            }
            _ => {}
        }
        a.menu_up = false;
        drop(shell);
        DestroyMenu(m);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if APP.load(Ordering::Relaxed).is_null() {
        return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
    }
    let a = app();
    match msg {
        WM_PAINT => {
            paint();
            0
        }
        WM_ERASEBKGND => 1,
        WM_HOTKEY if wp as i32 == HOTKEY_ID => {
            if unsafe { IsWindowVisible(hwnd) } != 0 && unsafe { GetForegroundWindow() } == hwnd {
                hide();
            } else {
                show();
            }
            0
        }
        WM_SHOW_ME => {
            show();
            0
        }
        WM_ACTIVATE => {
            if (wp & 0xFFFF) as u32 == WA_INACTIVE && !a.menu_up {
                hide();
            }
            0
        }
        WM_COMMAND => {
            // EN_CHANGE from the search box.
            if (wp >> 16) as u32 == EN_CHANGE && lp as HWND == a.edit {
                search();
            }
            0
        }
        WM_CTLCOLOREDIT => {
            let t = theme(a.light);
            unsafe {
                SetTextColor(wp as HDC, t.text);
                SetBkColor(wp as HDC, t.input);
            }
            a.edit_brush as LRESULT
        }
        WM_RESULTS => {
            let ans = unsafe { Box::from_raw(lp as *mut Answer) };
            on_results(ans);
            0
        }
        WM_TIMER if wp == TIMER_RETRY => {
            unsafe { KillTimer(hwnd, TIMER_RETRY) };
            search();
            0
        }
        WM_MOUSEMOVE => {
            let y = ((lp >> 16) & 0xFFFF) as i16 as i32;
            let h = row_at(y);
            if h != a.hover {
                a.hover = h;
                unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
            }
            0
        }
        WM_LBUTTONDOWN => {
            let y = ((lp >> 16) & 0xFFFF) as i16 as i32;
            if let Some(i) = row_at(y) {
                a.sel = i;
                unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
            }
            unsafe { SetFocus(a.edit) };
            0
        }
        WM_RBUTTONUP => {
            let (x, y) = ((lp & 0xFFFF) as i16 as i32, ((lp >> 16) & 0xFFFF) as i16 as i32);
            if let Some(i) = row_at(y) {
                let mut p = POINT { x, y };
                unsafe { ClientToScreen(hwnd, &mut p) };
                context_menu(i, Some(p));
            }
            0
        }
        WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR if a.menu_up => match shell_menu::message(msg, wp, lp) {
            Some(r) => r,
            None => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
        },
        WM_LBUTTONDBLCLK => {
            let y = ((lp >> 16) & 0xFFFF) as i16 as i32;
            if let Some(i) = row_at(y) {
                let ctrl = unsafe { GetKeyState(VK_CONTROL as i32) } < 0;
                open(i, ctrl);
            }
            0
        }
        WM_MOUSEWHEEL => {
            let delta = ((wp >> 16) & 0xFFFF) as i16 as i32;
            let n = a.items.len();
            if n > MAX_ROWS {
                let top = a.top as i32 - delta.signum() * 3;
                a.top = top.clamp(0, (n - MAX_ROWS) as i32) as usize;
                unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
            }
            0
        }
        WM_DPICHANGED => {
            set_fonts((wp & 0xFFFF) as u32);
            layout();
            0
        }
        WM_SETTINGCHANGE => {
            let light = system_light();
            if light != a.light {
                a.light = light;
                set_theme();
                unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
            }
            0
        }
        WM_TRAY => {
            match (lp & 0xFFFF) as u32 {
                WM_LBUTTONUP => show(),
                WM_RBUTTONUP | WM_CONTEXTMENU => tray_menu(),
                _ => {}
            }
            0
        }
        WM_DESTROY => {
            tray(false);
            unsafe { PostQuitMessage(0) };
            0
        }
        m if m == a.taskbar_created && m != 0 => {
            // Explorer restarted: the tray icon is gone; add it back.
            tray(true);
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

fn main() {
    // No console here: a panic goes to %LOCALAPPDATA%\Qasd\window.log.
    std::panic::set_hook(Box::new(|info| {
        let _ = std::fs::create_dir_all(data_dir());
        let bt = std::backtrace::Backtrace::force_capture();
        let _ = std::fs::write(data_dir().join("window.log"), format!("{info}\n{bt}"));
    }));
    // OLE, not just COM: the shell's "Copy" puts a data object on the clipboard.
    let _ = unsafe { windows::Win32::System::Ole::OleInitialize(None) };
    let class = w(CLASS);
    // One instance: a second start shows the first one's window.
    let mutex = w(r"Local\QasdSearchWindow");
    unsafe {
        CreateMutexW(std::ptr::null(), 0, mutex.as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let other = FindWindowW(class.as_ptr(), std::ptr::null());
            if !other.is_null() {
                PostMessageW(other, WM_SHOW_ME, 0, 0);
            }
            return;
        }
    }
    let hinst = unsafe { GetModuleHandleW(std::ptr::null()) };
    let icon = unsafe { LoadImageW(hinst, 1 as _, IMAGE_ICON, 0, 0, LR_DEFAULTSIZE | LR_SHARED) } as HICON;
    let small = unsafe { LoadImageW(hinst, 1 as _, IMAGE_ICON, GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON), LR_SHARED) } as HICON;
    let wc = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        style: CS_DROPSHADOW | CS_DBLCLKS,
        lpfnWndProc: Some(wndproc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: hinst,
        hIcon: icon,
        hCursor: unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW) },
        hbrBackground: std::ptr::null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: class.as_ptr(),
        hIconSm: small,
    };
    unsafe { RegisterClassExW(&wc) };
    let title = w("قصد");
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            0,
            0,
            WIDTH,
            HEAD,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        )
    };
    // Rounded corners (Windows 11).
    let round: i32 = 2; // DWMWCP_ROUND
    unsafe { DwmSetWindowAttribute(hwnd, 33, (&raw const round).cast(), 4) };
    let edit_class = w("EDIT");
    let edit = unsafe {
        CreateWindowExW(
            0,
            edit_class.as_ptr(),
            std::ptr::null(),
            WS_CHILD | WS_VISIBLE | ES_AUTOHSCROLL as u32,
            0,
            0,
            100,
            30,
            hwnd,
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        )
    };
    let cue = w("ابحث عن ملف أو مجلد…  (grep: للبحث داخل الملفات)");
    unsafe { SendMessageW(edit, EM_SETCUEBANNER, 1, cue.as_ptr() as isize) };

    let (tx, rx) = std::sync::mpsc::channel();
    let hw = hwnd as usize;
    std::thread::spawn(move || worker(rx, hw));
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    let app = Box::new(App {
        hwnd,
        edit,
        dpi,
        fonts: Fonts::new(dpi),
        light: system_light(),
        edit_brush: std::ptr::null_mut(),
        items: Vec::new(),
        sel: 0,
        top: 0,
        hover: None,
        status: String::new(),
        seq: 0,
        shown_seq: 0,
        jobs: tx,
        icons: HashMap::new(),
        images: 0,
        query: String::new(),
        tray_icon: small,
        taskbar_created: unsafe { RegisterWindowMessageW(w("TaskbarCreated").as_ptr()) },
        menu_up: false,
    });
    APP.store(Box::into_raw(app), Ordering::Relaxed);
    unsafe { SendMessageW(edit, WM_SETFONT, crate::app().fonts.input as usize, 1) };
    set_theme();
    tray(true);
    // Warm the daemon (and its index) now, not on the first keystroke.
    let _ = crate::app().jobs.send(Job::Warm);

    if unsafe { RegisterHotKey(hwnd, HOTKEY_ID, MOD_ALT | MOD_NOREPEAT, b'Q' as u32) } == 0 {
        let text = w("الاختصار Alt+Q مستخدم من برنامج آخر.\nيمكنك فتح البحث من أيقونة قصد في شريط المهام.");
        let cap = w("قصد");
        unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), cap.as_ptr(), MB_OK | MB_ICONWARNING | MB_RTLREADING | MB_RIGHT) };
    }
    // `qasdw --show [--query text]`: open right away (from a shortcut or script).
    let args: Vec<String> = std::env::args().collect();
    if let Some(q) = args.iter().position(|a| a == "--query").and_then(|i| args.get(i + 1)) {
        let ws = w(q);
        unsafe { SetWindowTextW(edit, ws.as_ptr()) };
    }
    if args.iter().any(|a| a == "--show" || a == "--query") {
        show();
    }

    let mut msg: MSG = unsafe { std::mem::zeroed() };
    while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
        if msg.message == WM_KEYDOWN && msg.hwnd == edit && key(msg.wParam as u32) {
            continue;
        }
        // (Enter and Esc would beep in a single-line EDIT.)
        if msg.message == WM_CHAR && msg.hwnd == edit && matches!(msg.wParam, 0x0D | 0x1B | 0x7F) {
            continue;
        }
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

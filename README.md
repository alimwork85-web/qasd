# Qasd (قَصْد)

[![Release](https://img.shields.io/github/v/release/alimwork85-web/qasd?color=2a78d6)](https://github.com/alimwork85-web/qasd/releases/latest)
[![Downloads](https://img.shields.io/github/downloads/alimwork85-web/qasd/total?color=2a78d6)](https://github.com/alimwork85-web/qasd/releases)
[![Windows 10/11](https://img.shields.io/badge/Windows-10%20%2F%2011-0078D4?logo=windows)](https://github.com/alimwork85-web/qasd/releases/latest)
[![Rust](https://img.shields.io/badge/Rust-fast-dea584?logo=rust)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/license-MIT-green)](https://github.com/alimwork85-web/qasd/blob/main/LICENSE)

**Developer:** Ali Alhumedi · [Download](https://github.com/alimwork85-web/qasd/releases/latest) · [Changelog / سجل التغييرات](https://github.com/alimwork85-web/qasd/blob/main/CHANGELOG.md)

**Qasd** is a whole-disk file search for Windows. It finds files by name even when you mistype them, searches inside text files through an index, and shows what is using your disk space. It has a search window on **Alt+Q**, a command-line interface with a small background daemon, a Rust crate, and an MCP server for coding agents.

Qasd is a Windows port of [fsearch](https://github.com/noahdunnagan/fsearch) (macOS, MIT). The search engine (name index, fuzzy matching, trigram content index, query language, git filters, MCP) comes from fsearch. The macOS-specific layers were rewritten for Windows.

---

## Contents

- [Quick start](#quick-start)
- [Search window (Alt+Q)](#search-window-altq)
- [Query syntax](#query-syntax)
- [Disk usage](#disk-usage)
- [Content search (grep)](#content-search-grep)
- [Speed](#speed)
- [Administrator rights](#administrator-rights)
- [API and MCP](#api-and-mcp)
- [Environment variables](#environment-variables)
- [How it works on Windows](#how-it-works-on-windows)
- [Limitations](#limitations)
- [العربية](#بالعربية)
- [License and credits](#license-and-credits)

---

## Quick start

**Option 1: build from source**

```powershell
cargo build --release
.\target\release\qasd.exe install   # copies to %LOCALAPPDATA%\Qasd\bin, adds it to PATH, starts with Windows
```

**Option 2: download**

1. Download the zip from [Releases](https://github.com/alimwork85-web/qasd/releases).
2. Extract it and double-click `install.cmd`.

**Then try it:**

```powershell
qasd qasd main                 # find files by name
qasd "ext:rs grep:apply_dir"   # search inside files
```

---

## Search window (Alt+Q)

Press **Alt+Q** anywhere. A search box opens over whatever you are doing, and results update as you type. Each result shows its file icon and folder.

| Key | Action |
| --- | --- |
| Typing | Search by name, or use `grep:` to search inside files |
| ↑ ↓, PgUp, PgDn | Move through results |
| Enter | Open the file |
| Ctrl+Enter | Show the file in its folder |
| Ctrl+C | Copy its path |
| Right-click, Menu key, or Shift+F10 | Open the file's Explorer context menu (open with, send to, copy, delete, properties, shell extensions). Shift+right-click shows the extended menu. |
| Drag a result | Drop the file into any program: mail, editor, chat, or folder |
| Ctrl+P | Show or hide the preview pane |
| Ctrl+U, the pie button, or right-click a folder | Open disk usage (see [Disk usage](#disk-usage)) |
| Empty search box | Show recent searches. Enter runs one again, Delete removes it. |
| Esc, or click elsewhere | Close the window |

**Preview pane.** It shows a thumbnail for images, videos, PDFs and folders. For text files it shows the first lines. For other files it shows the icon, type, size and date. Files that exist only in OneDrive's cloud are never downloaded for preview.

**Other behavior:**

- Each result shows its size and date. A folder's size is the total of everything inside it, summed from the index, so no disk access is needed.
- Files you open rank higher in future searches.
- The app lives in the system tray. Right-click the tray icon to start with Windows or quit.
- It is plain Win32, so it opens instantly and uses a few MB of memory.
- It follows the system's light or dark theme and DPI settings.

---

## Query syntax

```text
qasd "readme in:~\Documents"              # inside a folder (C:\..., ~\..., or relative)
qasd "type:image size:>5mb mtime:<7d"     # by type, size and modification time
qasd "ext:rs regex:fn\s+\w+_dir"          # regex inside files
qasd "sym:apply_dir"                      # where a symbol is defined
qasd "src/**/*.{c,h} !test/ init"         # globs; ! excludes
qasd "git:modified in:~\Projects\app"     # git status filters
```

**Matching rules:**

- Words are fuzzy and forgive typos: one letter in a short word, two in words of 8 or more letters. For example, `mian.rs` finds `main.rs`.
- `'exact` matches exactly.
- `^prefix` matches the start of a name, and `suffix$` matches the end.
- `!exclude` leaves out matches.
- A phrase with spaces goes in quotes, for example `grep:"fn main"`.

**Filters:**

| Filter | Purpose |
| --- | --- |
| `ext:` | File extension |
| `type:` / `kind:` | File type or category |
| `in:` | Folder to search in |
| `size:` | File size |
| `mtime:` | Modification time |
| `path:` | Match against the full path |
| `re:` | Regex on names |
| `grep:` | Text inside files |
| `regex:` | Regex inside files |
| `sym:` | Symbol definitions |
| `limit:` | Maximum number of results |
| `git:` | Git status: `modified`, `staged`, `untracked`, `clean`, `tracked`, `ignored`, `conflict` |

---

## Disk usage

Press **Ctrl+U** in the search window, click the pie button, or right-click a folder. It shows what takes up space:

- The contents of a folder, largest first.
- Each item's size, its share of the total, and a bar.
- Every drive, when no folder is selected.

Enter goes into a folder, Backspace goes up one level, and Esc returns to search. Sizes come from the index, so the view appears in a fraction of a second without reading the disk.

---

## Content search (grep)

Qasd indexes file contents as well as names.

- Text files in your user folder up to **1 MB** are indexed. Inside git repositories the limit is **10 MB**.
- Folders that git ignores are skipped.
- `AppData`, lockfiles, large JSON files, minified assets and build output are skipped.
- To skip more folders, add them to `%LOCALAPPDATA%\Qasd\content-skip.txt`, one per line.
- `in:` with a folder outside your user folder searches that folder by reading its files directly.

---

## Speed

Measured on Windows 11 with a C: drive holding about 2 million entries.

| Operation | Result |
| --- | --- |
| First full index, as administrator (read from the MFT) | 5.3 s |
| First full index, as a normal user (folder crawl) | 9.5 s warm, 34 s cold |
| Restart, catching up on changes (change journal) | 0.08 s |
| Find a file by name, in the daemon | ~0.8 ms |
| Grep, first page, in the daemon | ~0.3 ms |
| CLI search, end to end (mostly Windows process start) | ~28 ms |
| New, renamed, edited or deleted file appears in results | Under one CLI round trip (~30 ms) |
| Name index on disk | 59 MB |
| Daemon memory | ~50 MB |

Index times and memory use depend on the number of files and the hardware.

---

## Administrator rights

As a normal user, Qasd indexes every folder you can open. It skips the few folders that only administrators can read.

If you run the daemon elevated, it also indexes those folders. It builds the index directly from each NTFS drive's master file table, which is several times faster.

To avoid a UAC prompt at every start, run this from an administrator terminal:

```powershell
qasd install --login   # creates a sign-in task with highest privileges
```

The change journal (restart catch-up and overflow recovery) works with or without administrator rights.

---

## API and MCP

**JSON API.** The daemon listens on the named pipe `\\.\pipe\qasd-<hash>`. Only the current user can open it. You can also use `qasd stdio`. Paths on the wire use the index form, `/C:/Users/me/x.txt`, and requests accept either form.

```json
{"q": "qasd main", "limit": 20, "offset": 20}
{"op": "grep", "pattern": "apply_dir", "in": "C:\\Users\\me\\Projects", "context": 2}
{"q": "main.rs", "boost": true, "near": "C:\\Users\\me\\app\\src\\lib.rs", "git": true}
{"op": "open", "path": "C:\\Users\\me\\app\\src\\main.rs"}
{"op": "recent", "in": "~\\Projects\\app"}
```

**MCP server for coding agents.** `qasd mcp` runs an MCP server over stdio. It provides `find_files`, `grep` and `multi_grep`.

```powershell
claude mcp add -s user qasd -- "%LOCALAPPDATA%\Qasd\bin\qasd.exe" mcp
```

**Other commands:**

| Command | Purpose |
| --- | --- |
| `qasd status` | Show daemon status |
| `qasd stop` | Stop the daemon |
| `qasd serve` | Run the daemon in the foreground |
| `qasd uninstall` | Remove Qasd |
| `qasd bench <query>` | Benchmark a query |

---

## Environment variables

| Variable | Effect |
| --- | --- |
| `QASD_DRIVES=C,D` | Index only these drives (default: all fixed and removable drives) |
| `QASD_NO_REMOVABLE=1` | Leave out USB drives and memory cards |
| `QASD_SKIP=D:\VMs;E:\backup` | Leave out these folders |

---

## How it works on Windows

| fsearch (macOS) | Qasd (Windows) |
| --- | --- |
| `getattrlistbulk` crawl | As administrator: each NTFS drive's `$MFT` is read and parsed directly (names, parents, sizes, times, links). Otherwise: `FindFirstFileExW` with `FindExInfoBasic` and `LARGE_FETCH`, with folders crawled in parallel using rayon. |
| FSEvents | `ReadDirectoryChangesW` on each drive root, covering the whole subtree |
| FSEvents replay after restart or dropped events | NTFS change journal (USN), readable without administrator rights. Only folders changed since the last run are relisted. Folder modification times are used where the journal cannot say. |
| Unix socket | Named pipe with overlapped I/O, ACL limited to the current user and SYSTEM |
| malloc space-efficient mode | mimalloc, with `mi_collect` when idle |
| launchd LaunchAgent | Task Scheduler sign-in task |
| `~/Library/Application Support/FSearch` | `%LOCALAPPDATA%\Qasd` |
| `/Users/me/x` | `/C:/Users/me/x` internally (one tree, drives under `/`); `C:\Users\me\x` on screen |

Symbolic links, junctions and mount points are listed as links and never followed. Paths are stored as WTF-8, so any NTFS file name round-trips correctly.

---

## Qasd or Everything?

[Everything](https://www.voidtools.com/) by voidtools is the best-known file search on Windows, and it is free. Qasd is a different tool with a different focus. Use this table to choose:

| | Everything | Qasd |
| --- | --- | --- |
| Price and license | Freeware | MIT, open source |
| Search by file name | Yes, instant | Yes, with typo tolerance |
| Search inside files | Yes, through `content:`, but files are not indexed and the search is slow | Yes, through a trigram content index for text files up to 1 MB (10 MB in git repos) |
| Regex | Yes | Yes (`regex:` and `re:`) |
| Boolean operators | Yes (`\|`, `!`, default AND) | Yes (`!` for exclusion, plus `'exact`, `^prefix`, `suffix$`) |
| Git status filters | No | Yes (`git:modified`, `git:staged`, and others) |
| Disk usage view | Not in its documentation | Yes (Ctrl+U) |
| Preview pane | Not in its documentation | Yes (Ctrl+P) |
| Remote access | HTTP and ETP servers, and a command-line client (es.exe), in the full version | JSON API over a named pipe, and an MCP server for coding agents |
| Maturity | Established, with versions running from Windows XP to Windows 11 | New project, first public release |

**Choose Everything** if you mainly search by file name, want the most mature tool, or need network and SDK integrations.

**Choose Qasd** if you want one tool that combines name search, typo tolerance, content search, and disk usage, and you want to read or change the code.

**Notes on the comparison:**

- Qasd's speed numbers are measured on one machine. They have not been run side by side with Everything on the same hardware.
- "Not in its documentation" means the features are not described in Everything's official FAQ. It does not mean they are impossible.
- Everything's official FAQ says it indexes only file and folder names, and that content search is slow because content is not indexed.

---

## Limitations

- Content search covers only text files under the size limits above. Binary files and large files are not searched inside.
- Files that exist only in OneDrive's cloud are never downloaded by a search or a preview.
- Index times on the first run depend on the number of files. Without administrator rights, the first index of a large drive is slower (see [Speed](#speed)).

---

## بالعربية

**قصد** أداة بحث فائقة السرعة في كل أقراص ويندوز، وهي نسخة لويندوز من أداة fsearch التي تعمل على ماك فقط. تبحث في أسماء الملفات بتسامح مع الأخطاء الإملائية، وفي محتوى الملفات النصية عبر فهرس يبقى محدّثاً لحظياً.

**التثبيت:** نزّل ملف zip من صفحة [الإصدارات](https://github.com/alimwork85-web/qasd/releases)، ثم فك الضغط واضغط مرتين على `install.cmd`.

**نافذة البحث:** اضغط **Alt+Q** في أي مكان واكتب اسم الملف، فتظهر النتائج أثناء الكتابة.

- **Enter** يفتح الملف، و**Ctrl+Enter** يفتح مجلده، و**Ctrl+C** ينسخ مساره، و**Esc** يغلق النافذة.
- النقر بالزر الأيمن على أي نتيجة يعرض قائمة الملف نفسها التي تظهر في المستكشف.
- اسحب أي نتيجة وأفلتها في برنامج آخر.
- **Ctrl+P** يُظهر لوحة المعاينة أو يخفيها.
- عندما يكون مربع البحث فارغاً تظهر عمليات البحث السابقة.

**حجم المجلدات:** اضغط **Ctrl+U** لمعرفة ما يشغل مساحة القرص: المحتويات من الأكبر إلى الأصغر، مع الحجم والنسبة وشريط يوضحها. وهذا كله يُحسب من الفهرس دون قراءة القرص.

**البحث من الطرفية:**

```powershell
qasd تقرير                          # بالاسم
qasd "ext:md grep:الميزانية"         # داخل الملفات
qasd "in:~\Desktop pdf"             # داخل مجلد محدد
```

**التشغيل التلقائي:** `qasd install --login` يجعل الخدمة تعمل عند تسجيل الدخول. وإذا شغّلته من طرفية بصلاحيات المدير فستُفهرَس أيضاً المجلدات التي لا يقرؤها إلا المدير.

**الأقراص الخارجية:** تُفهرس أسماء الملفات على الأقراص الثابتة وعلى أقراص USB وبطاقات الذاكرة تلقائياً. القرص المتصل يُفهرس خلال ثانية، والمفصول يختفي من النتائج.

---

## License and credits

MIT License. Copyright © 2026 **Ali Alhumedi**. Built on fsearch, copyright © 2026 Noah Dunnagan.

See [LICENSE](https://github.com/alimwork85-web/qasd/blob/main/LICENSE).

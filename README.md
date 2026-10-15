# Qasd (قَصْد)

**Developer: Ali Alhumedi** · [Download](https://github.com/alimwork85-web/qasd/releases/latest) · [Changelog / سجل التغييرات](CHANGELOG.md)

Whole-disk file search for Windows: fuzzy names that forgive typos, and indexed
search inside files. A search window on Alt+Q, a CLI with a small background
daemon, a Rust crate, or an MCP server for coding agents.

Qasd is a Windows port of [fsearch](https://github.com/noahdunnagan/fsearch)
(macOS, MIT). The search engine (name index, fuzzy matching, trigram content
index, query language, git filters, MCP) is fsearch's. The macOS layers were
rewritten for Windows.

```
cargo build --release
.\target\release\qasd.exe install          # -> %LOCALAPPDATA%\Qasd\bin, on PATH, window starts with Windows
qasd qasd main                             # find files by name
qasd "ext:rs grep:apply_dir"               # search inside files
```

Or download the zip from [Releases](https://github.com/alimwork85-web/qasd/releases),
extract it, and double-click `install.cmd`.

## The search window (Alt+Q)

Press **Alt+Q** anywhere: a search box opens over whatever you're doing, with
results as you type, each with its file icon and folder.

| Key | |
|---|---|
| typing | search (names, or `grep:` inside files) |
| ↑ ↓, PgUp PgDn | move |
| Enter | open the file |
| Ctrl+Enter | show it in its folder |
| Ctrl+C | copy its path |
| right-click, menu key, Shift+F10 | the file's Explorer menu: open with, send to, copy, delete, properties, your shell extensions (Shift+right-click for the extended one) |
| drag a result | drop the file into any program: a mail, an editor, a chat, a folder |
| Ctrl+P | show or hide the preview pane |
| empty box | your recent searches: Enter runs one again, Delete removes it |
| Ctrl+U, the pie button, or right-click a folder | what takes the space: a folder's contents largest first, each with its size, share and a bar (every drive when no folder is selected); Enter goes into a folder, Backspace up, Esc back to searching |

Each result shows its size and date; a folder's size is everything in it,
summed from the index (no disk access). The preview pane shows the selected
file's thumbnail (images, videos, PDFs, folders), a text file's first lines,
or its icon, type, size and date. Files only in OneDrive's cloud are never
downloaded to preview them.
| Esc, or clicking elsewhere | close |

It lives in the tray (right-click: start with Windows, quit). It's plain
Win32, so it opens instantly and uses a few MB. It follows the system's
light or dark theme and DPI. Files you open rank higher next time.

## Speed

Measured on this machine: Windows 11, C: with about 2 million entries.

| | |
|---|---:|
| first full index, as administrator (read from the MFT) | 5.3 s |
| first full index, as a normal user (folder crawl) | 9.5 s warm, 34 s cold |
| restart, catching up on changes since the last run (change journal) | 0.08 s |
| find a file by name, in the daemon | ~0.8 ms |
| grep, first page, in the daemon | ~0.3 ms |
| a CLI search, end to end (mostly Windows process start) | ~28 ms |
| a new, renamed, edited or deleted file shows up | under one CLI round trip (~30 ms) |
| name index on disk | 59 MB |
| daemon memory | ~50 MB |

## Queries

```
qasd "readme in:~\Documents"              # inside a folder (C:\..., ~\..., relative)
qasd "type:image size:>5mb mtime:<7d"
qasd "ext:rs regex:fn\s+\w+_dir"          # regex inside files
qasd "sym:apply_dir"                      # where it's defined
qasd "src/**/*.{c,h} !test/ init"         # globs, in or out
qasd "git:modified in:~\Projects\app"
```

Words are fuzzy and forgive a typo at any letter, two in words of 8+
(`mian.rs` finds `main.rs`). Also `'exact`, `^prefix`, `suffix$` and
`!exclude`. Filters: `ext:` `type:` `kind:` `in:` `size:` `mtime:` `re:`
`path:` `grep:` `regex:` `sym:` `limit:` and `git:` (`modified` `staged`
`untracked` `clean` `tracked` `ignored` `conflict`). A pattern with spaces goes
in quotes: `grep:"fn main"`.

## What grep covers

Text files in your user folder up to 1 MB, and up to 10 MB inside git repos,
minus what git ignores. It leaves out `AppData`, lockfiles, large JSON,
minified assets and build output. Add more folders to skip to
`%LOCALAPPDATA%\Qasd\content-skip.txt`, one per line. File names are indexed
on every fixed drive and on USB sticks and memory cards: a drive plugged in
is indexed within a second, one unplugged leaves the results, and Qasd lets
go of a drive as soon as Windows is asked to eject it, so "Safely Remove"
never finds it in use. `in:` a folder outside your user folder is searched by
reading its files directly. OneDrive files that are only in the cloud are
never downloaded by a search.

## Administrator rights

As a normal user, Qasd indexes every folder you can open and leaves out the
few that only administrators can read. Run the daemon elevated and it indexes
those too, and builds the index straight from each NTFS drive's master file
table, several times faster. `qasd install --login` from an administrator
terminal creates a sign-in task with the highest rights, so no UAC prompt
appears each time. The change journal (restart catch-up, overflow recovery)
works either way.

Environment variables:

- `QASD_DRIVES=C,D` limits indexing to these drives (default: every fixed and removable drive).
- `QASD_NO_REMOVABLE=1` leaves USB sticks and memory cards out.
- `QASD_SKIP=D:\VMs;E:\backup` leaves out these folders.

## API

JSON lines over the named pipe `\\.\pipe\qasd-<hash>`, which only the current
user can open, or through `qasd stdio`. Paths on the wire use the index's
form, `/C:/Users/me/x.txt`. Requests accept either form.

```json
{"q": "qasd main", "limit": 20, "offset": 20}
{"op": "grep", "pattern": "apply_dir", "in": "C:\\Users\\me\\Projects", "context": 2}
{"q": "main.rs", "boost": true, "near": "C:\\Users\\me\\app\\src\\lib.rs", "git": true}
{"op": "open", "path": "C:\\Users\\me\\app\\src\\main.rs"}
{"op": "recent", "in": "~\\Projects\\app"}
```

For coding agents, `qasd mcp` is an MCP server on stdio with `find_files`,
`grep` and `multi_grep`:

```
claude mcp add -s user qasd -- "%LOCALAPPDATA%\Qasd\bin\qasd.exe" mcp
```

Other commands: `qasd status`, `qasd stop`, `qasd serve` (daemon in the
foreground), `qasd uninstall`, `qasd bench <query>`.

## How it works on Windows

| fsearch on macOS | Qasd on Windows |
|---|---|
| `getattrlistbulk` crawl | as administrator: each NTFS drive's $MFT read raw and parsed (names, parents, sizes, times, links); otherwise `FindFirstFileExW` (`FindExInfoBasic`, `LARGE_FETCH`), folders in parallel on rayon |
| FSEvents | `ReadDirectoryChangesW` on each drive root, whole subtree |
| FSEvents replay after a restart or a dropped burst | the NTFS change journal (USN), read without administrator rights: just the folders changed since; mtime relisting where a journal can't say |
| unix socket | named pipe, overlapped I/O, ACL limited to the user and SYSTEM |
| malloc space-efficient mode | mimalloc, with `mi_collect` when idle |
| launchd LaunchAgent | Task Scheduler sign-in task |
| `~/Library/Application Support/FSearch` | `%LOCALAPPDATA%\Qasd` |
| `/Users/me/x` | `/C:/Users/me/x` inside (one tree, drives under `/`), `C:\Users\me\x` on screen |

Symbolic links, junctions and mount points are listed as links and never
followed. Paths are kept as WTF-8, so any NTFS name round-trips.

---

## بالعربية

**قصد** أداة بحث فائقة السرعة في كل أقراص ويندوز، وهي نسخة لويندوز من أداة
fsearch التي تعمل على الماك فقط. تبحث في أسماء الملفات بتسامح مع الأخطاء
الإملائية، وفي محتوى الملفات النصية عبر فهرس، ويبقى الفهرس محدّثاً لحظياً.

**التثبيت:** نزّل ملف zip من صفحة [الإصدارات](https://github.com/alimwork85-web/qasd/releases)،
وفك الضغط، ثم اضغط مرتين على `install.cmd`.

**نافذة البحث:** اضغط **Alt+Q** في أي مكان واكتب اسم الملف، فتظهر النتائج أثناء الكتابة.
Enter يفتح الملف، وCtrl+Enter يفتح مجلده، وCtrl+C ينسخ مساره، وEsc يغلق النافذة.
والنقر بالزر الأيمن على أي نتيجة يعرض قائمة الملف نفسها التي تظهر في المستكشف.
اسحب أي نتيجة وأفلتها في برنامج آخر. وتعرض كل نتيجة حجمها وتاريخها، وحجم المجلد يشمل كل ما فيه.
لوحة المعاينة تعرض الصورة أو أول أسطر الملف النصي، ويُظهرها Ctrl+P أو يخفيها.
وعندما يكون مربع البحث فارغاً تظهر عمليات البحث السابقة.
ولمعرفة ما الذي يشغل مساحة القرص: Ctrl+U أو زر الدائرة في مربع البحث. يعرض محتويات المجلد من الأكبر إلى الأصغر، مع حجم كل عنصر ونسبته وشريط يوضحها. Enter يدخل المجلد، وBackspace يصعد مستوى، وEsc يعود إلى البحث.
وكل هذا يُحسب من الفهرس في أجزاء من الثانية، دون قراءة القرص.
تُفهرس الأقراص الخارجية (USB) تلقائياً عند توصيلها، وتُزال من النتائج عند فصلها.
تعمل النافذة مع تشغيل ويندوز، وأيقونتها في شريط المهام بجانب الساعة.

ويمكنك البحث من الطرفية أيضاً: اكتب `qasd` متبوعاً بما تبحث عنه:

```
qasd تقرير                         # بالاسم
qasd "ext:md grep:الميزانية"        # داخل الملفات
qasd "in:~\Desktop pdf"            # داخل مجلد
```

لتعمل الخدمة تلقائياً عند تسجيل الدخول: `qasd install --login`. وإذا شغّلته من
طرفية بصلاحيات المدير فستُفهرَس أيضاً المجلدات التي لا يقرؤها إلا المدير.

## License

MIT. Qasd © 2026 **Ali Alhumedi**. Built on fsearch © 2026 Noah Dunnagan.
See [LICENSE](LICENSE).

المطوّر: **Ali Alhumedi**.

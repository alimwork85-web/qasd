<div align="center">

# Qasd (قَصْد)

[![GitHub release](https://img.shields.io/github/v/release/alimwork85-web/qasd)](https://github.com/alimwork85-web/qasd/releases)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-99.7%25-orange)](https://www.rust-lang.org)
[![Windows](https://img.shields.io/badge/Windows-10%2F11-0078D6)](https://www.microsoft.com/windows)

**Whole-disk file search for Windows.** Alt+Q anywhere, typo-tolerant names, indexed grep inside files, and an MCP server for AI coding agents.

[Download](https://github.com/alimwork85-web/qasd/releases/latest) · [Changelog](CHANGELOG.md) · [Issues](https://github.com/alimwork85-web/qasd/issues)

</div>

---

<div align="center">
  <img src="assets/screenshot-main.png" alt="Qasd search window" width="700" /><br>
  <em>The search window: press Alt+Q anywhere — results appear as you type</em>
</div>

<div align="center">
  <img src="assets/screenshot-grep.png" alt="Qasd grep search" width="700" /><br>
  <em>Indexed grep — search inside your files in milliseconds</em>
</div>

<div align="center">
  <img src="assets/screenshot-disk.png" alt="Qasd disk usage" width="700" /><br>
  <em>Disk usage view: Ctrl+U to see what takes the space</em>
</div>

---

## Why Qasd?

| Feature | Everything | Qasd |
|---|---|---|
| Instant name search | ✅ | ✅ + typo-tolerant fuzzy matching |
| Search inside files (grep) | ❌ | ✅ Indexed, ~0.3 ms |
| AI agent support (MCP) | ❌ | ✅ Built-in MCP server |
| Disk usage analyzer | ❌ | ✅ Ctrl+U, computed from index |
| Open source | ✅ | ✅ Rust, MIT |
| Lightweight UI | ✅ | ✅ Plain Win32, a few MB |

A Windows port of [fsearch](https://github.com/) (macOS). The search engine is fsearch's; every Windows layer was rewritten from scratch.

## Quick start

```bash
cargo build --release
.\target\release\qasd.exe install    # -> %LOCALAPPDATA%\Qasd\bin, on PATH
qasd main                            # find files by name
qasd "ext:rs grep:apply_dir"         # search inside files

Or download the zip from Releases, extract it, and double-click install.cmd.

Features

Instant search window
Alt+Q anywhere
Open the file / its folder
Enter / Ctrl+Enter
Copy path
Ctrl+C
Preview pane
Ctrl+P
Disk usage
Ctrl+U or the pie button
Drag a result
Drop into mail, editor, chat, folder
Recent searches
Empty box
CLI search
qasd <query>
MCP server for agents
qasd mcp

Performance
Measured on Windows 11, C: with ~2 million entries:

Measured on Windows 11, C: with ~2 million entries:

First full index, as admin (raw MFT read)
5.3 s
First full index, normal user (folder crawl)
9.5 s warm
Restart catch-up (USN change journal)
0.08 s
Name lookup in the daemon
~0.8 ms
Grep, first page, in the daemon
~0.3 ms
Name index on disk
59 MB
Daemon memory
~50 MB

Queries
qasd "readme in:~\Documents"              # inside a folder
qasd "type:image size:>5mb mtime:<7d"     # filters
qasd "ext:rs regex:fn\s+\w+_dir"          # regex inside files
qasd "sym:apply_dir"                      # where it's defined
qasd "src/**/*.{c,h} !test/ init"         # globs, in or out
qasd "git:modified in:~\Projects\app"     # git status filters

Words are fuzzy and forgive a typo at any letter (mian.rs finds main.rs). Also 'exact, ^prefix, suffix$, !exclude.

MCP server for coding agents
claude mcp add -s user qasd -- "%LOCALAPPDATA%\Qasd\bin\qasd.exe" mcp

Tools exposed: find_files, grep, multi_grep. Give your AI agent instant whole-disk search without crawling the filesystem.

How it works on Windows
getattrlistbulk crawl
Raw $MFT parse as admin; FindFirstFileExW otherwise
FSEvents
ReadDirectoryChangesW per drive root
FSEvents replay
NTFS USN change journal — no admin needed
Unix socket
Named pipe, overlapped I/O, ACL limited to the user
launchd
Task Scheduler sign-in task
Symlinks, junctions and mount points are listed as links and never followed. Paths are kept as WTF-8, so any NTFS name round-trips.

License
MIT. Qasd © 2026 Ali Alhumedi. Built on fsearch © 2026 Noah Dunnagan.

قصد (Qasd)
بحث فائق السرعة في كل أقراص ويندوز. اضغط Alt+Q في أي مكان، ابحث في الأسماء بتسامح مع الأخطاء الإملائية، وفي محتوى الملفات عبر فهرس نصي، مع خادم MCP لوكلاء البرمجة بالذكاء الاصطناعي.

تحميل · سجل التغييرات · المشاكل

لماذا قصد؟
بحث فوري في الأسماء
✅
✅ + تسامح مع الأخطاء
البحث داخل الملفات
❌
✅ فهرس نصي، ~0.3 مللي ثانية
دعم وكلاء الذكاء الاصطناعي (MCP)
❌
✅ خادم مدمج
تحليل استخدام القرص
❌
✅ Ctrl+U من الفهرس مباشرةً
مفتوح المصدر
✅
✅ Rust، رخصة MIT
قصد نسخة لويندوز من أداة fsearch التي تعمل على الماك فقط. محرك البحث hers لفsearch، وكل طبقات ويندوز كُتبت من جديد.

التثبيت
نزّل ملف ZIP من صفحة الإصدارات، فك الضغط، واضغط مرتين على install.cmd.

أو من المصدر:

bash

نسخ
cargo build --release
.\target\release\qasd.exe install
الاستخدام
اضغط Alt+Q في أي مكان واكتب اسم الملف. Enter يفتح الملف، وCtrl+Enter يفتح مجلده، وCtrl+C ينسخ مساره، وCtrl+P يعرض المعاينة، وCtrl+U يعرض ما يشغل مساحة القرص.

ومن الطرفية:

bash

نسخ
qasd تقرير                         # بالاسم
qasd "ext:md grep:الميزانية"        # داخل الملفات
qasd "in:~\Desktop pdf"            # داخل مجلد
الكلمات ضبابية وتتسامح مع الأخطاء: كتابة mian.rs تجد main.rs.

لتعمل الخدمة تلقائياً عند تسجيل الدخول: qasd install --login. وإذا شغّلته بصلاحيات المدير فستُفهرَس أيضاً المجلدات المحمية وتُبنى الفهرسة من MFT مباشرةً (أسرع بمرات).

الأداء
على ويندوز 11 بقرص فيه نحو مليوني ملف: الفهرسة الكاملة كمسؤول تستغرق 5.3 ثانية، والتحديث بعد إعادة التشغيل 0.08 ثانية، والبحث عن اسم أقل من مللي ثانية واحدة.

الرخصة
MIT. قصد © 2026 علي الحميدي. مبني على fsearch © 2026 Noah Dunnagan.

</div> ```

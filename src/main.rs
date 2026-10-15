mod frecency;
mod git;
mod mcp;
mod server;

use qasd::{index, live, query, sys};

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::time::Instant;

/// See `qasd::alloc`.
#[global_allocator]
static GLOBAL: qasd::alloc::Alloc = qasd::alloc::Alloc;

const USAGE: &str = "qasd - whole-disk file search for Windows

usage:
  qasd <query...> [--json]   search (starts the daemon if needed)
  qasd stdio                 JSON lines on stdin/stdout
  qasd mcp                   MCP server on stdin/stdout, for coding agents
  qasd serve                 run the daemon in the foreground
  qasd status
  qasd stop                  stop the daemon (keeps the index)
  qasd install [--login]     copy qasd and the search window to %LOCALAPPDATA%\\Qasd\\bin,
                             add it to PATH, start the window (Alt+Q) with Windows;
                             --login also starts the daemon at sign-in
                             (run from an administrator terminal to index every folder)
  qasd uninstall             remove the sign-in task and the window's start with Windows (keeps the index)
  qasd bench <query...>      time a query in-process against the saved index

examples:
  qasd qasd main                       find files by name (typos forgiven)
  qasd \"ext:rs grep:apply_dir\"         search inside files
  qasd \"report in:~\\Documents\"        inside a folder
  qasd \"type:image size:>5mb mtime:<7d\"";

/// The Task Scheduler task that starts the daemon at sign-in.
const TASK: &str = "Qasd Daemon";

fn home() -> String {
    sys::home()
}

fn data_dir() -> PathBuf {
    let d = qasd::default_dir(&home());
    std::fs::create_dir_all(&d).ok();
    d
}

fn main() {
    // Arabic and other non-ASCII paths print right in the console.
    unsafe { windows_sys::Win32::System::Console::SetConsoleOutputCP(65001) };
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("-h" | "--help" | "help") => eprintln!("{USAGE}"),
        Some("serve") => server::serve(data_dir(), home()),
        Some("stdio") => stdio(),
        Some("mcp") => mcp::run(data_dir()),
        Some("status") => print_one(&serde_json::json!({"op": "status"}), true),
        Some("stop") => match server::stop(&data_dir()) {
            Ok(()) => println!("stopped"),
            Err(e) => die(&e),
        },
        Some("bench") => bench(&args[1..].join(" ")),
        Some("install") => install(args.iter().any(|a| a == "--login")),
        Some("uninstall") => uninstall(),
        Some(_) => {
            let json = args.iter().any(|a| a == "--json");
            let q: Vec<&str> = args.iter().map(String::as_str).filter(|a| *a != "--json").collect();
            // The folder `git:` filters and globs are about, unless `in:` says.
            let cwd = std::env::current_dir().map(|d| String::from_utf8_lossy(&sys::bytes(d)).into_owned()).unwrap_or_default();
            print_one(&serde_json::json!({"q": q.join(" "), "cwd": cwd}), json);
        }
    }
}

fn print_one(req: &serde_json::Value, raw: bool) {
    let mut s = server::connect(&data_dir()).unwrap_or_else(|e| die(&format!("cannot reach daemon: {e}")));
    s.write_all(format!("{req}\n").as_bytes()).unwrap();
    // One request only: the daemon needn't keep the cores warm for a next one.
    let _ = s.shutdown(std::net::Shutdown::Write);
    let mut line = String::new();
    BufReader::new(&mut s).read_line(&mut line).unwrap();
    if raw {
        print!("{line}");
        return;
    }
    let v: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
    if v["ok"] != true {
        die(v["error"].as_str().unwrap_or("error"));
    }
    let show = |p: &serde_json::Value| sys::display(p.as_str().unwrap_or("").as_bytes());
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    for h in v["hits"].as_array().into_iter().flatten() {
        let _ = writeln!(out, "{}", show(&h["path"]));
    }
    for f in v["files"].as_array().into_iter().flatten() {
        for m in f["matches"].as_array().into_iter().flatten() {
            let _ = writeln!(out, "{}:{}: {}", show(&f["path"]), m["line"], m["text"].as_str().unwrap_or("").trim());
        }
    }
    let _ = out.flush();
}

fn stdio() {
    let s = server::connect(&data_dir()).unwrap_or_else(|e| die(&format!("cannot reach daemon: {e}")));
    let mut up = s.try_clone().unwrap();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if writeln!(up, "{line}").is_err() {
                break;
            }
        }
        let _ = up.shutdown(std::net::Shutdown::Write);
    });
    let mut out = std::io::stdout().lock();
    for line in BufReader::new(s).lines() {
        let Ok(line) = line else { break };
        if writeln!(out, "{line}").and_then(|_| out.flush()).is_err() {
            break;
        }
    }
}

fn bench(qs: &str) {
    let dir = data_dir();
    let idx = index::Index::load(&dir.join("index.bin")).unwrap_or_else(|| die("no index yet; run qasd serve"));
    let live = live::Live::new(idx);
    let home = home();
    let mut q = query::Query::parse(qs, &home).unwrap_or_else(|e| die(&e));
    // Like the daemon: a content search when the query has a pattern, on
    // high-priority threads, paths included.
    let grep = q.grep.take().map(|p| qasd::Grep::new(&p, q.grep_mode).unwrap_or_else(|e| die(&e)));
    let content = grep.as_ref().map(|_| qasd::content::Content::open_shared(dir.join("content")));
    let pool = rayon::ThreadPoolBuilder::new()
        .start_handler(|_| {
            sys::priority_interactive();
            qasd::alloc::keep_on_this_thread();
        })
        .build()
        .unwrap();
    let run = || -> Vec<String> {
        let (Some(g), Some(c)) = (&grep, &content) else {
            let mut p = Vec::new();
            let hits = pool.install(|| query::Searcher { live: &live }.search(&q));
            return hits
                .iter()
                .map(|h| {
                    live.base.path(h.idx as usize, &mut p);
                    format!("{:5} {}", h.score, sys::display(&p))
                })
                .collect();
        };
        let r = if q.scope.as_ref().is_none_or(|s| qasd::content::in_scope(s, home.as_bytes())) {
            pool.install(|| c.search(g, &q))
        } else {
            let paths = pool.install(|| qasd::content::scan_paths(&live, q.clone_for_scan()));
            qasd::content::verify(g, &paths, q.limit)
        };
        let first = |f: &qasd::FileMatches| f.lines.first().map_or(String::new(), |(n, t)| format!("{n}: {}", t.trim()));
        r.files.iter().map(|f| format!("{}:{}", sys::display(&f.path), first(f))).collect()
    };
    // Between runs, another name search: a repeat would be answered from
    // the names cache, which only a repeat of the last query hits.
    let other = query::Query::parse("zzzzzz", &home).unwrap();
    let mut times = Vec::new();
    let mut out = Vec::new();
    for _ in 0..20 {
        pool.install(|| query::Searcher { live: &live }.search(&other));
        let t = Instant::now();
        out = run();
        times.push(t.elapsed());
    }
    for l in out.iter().take(10) {
        println!("{l}");
    }
    let first = times[0];
    times.sort();
    eprintln!("first {first:.2?}  median {:.2?}  min {:.2?}", times[times.len() / 2], times[0]);
}

fn bin_dir() -> PathBuf {
    data_dir().join("bin")
}

/// Run a program quietly; did it succeed?
fn run_quiet(prog: &str, args: &[&str]) -> bool {
    use std::os::windows::process::CommandExt;
    std::process::Command::new(prog)
        .args(args)
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn task_exists() -> bool {
    run_quiet("schtasks", &["/Query", "/TN", TASK])
}

/// Add `dir` to the user's PATH (HKCU\Environment), through PowerShell so
/// the change is broadcast to new terminals.
fn add_to_path(dir: &str) -> bool {
    let script = format!(
        "$d='{dir}'; $p=[Environment]::GetEnvironmentVariable('Path','User'); \
         if (-not $p) {{ $p='' }}; \
         if (($p -split ';') -notcontains $d) {{ [Environment]::SetEnvironmentVariable('Path', ($p.TrimEnd(';') + ';' + $d).TrimStart(';'), 'User') }}"
    );
    run_quiet("powershell", &["-NoProfile", "-NonInteractive", "-Command", &script])
}

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

/// Copy `from` over `to` (through a temp name, so a reinstall from the
/// installed copy works too).
fn put(from: &std::path::Path, to: &std::path::Path) {
    if from.canonicalize().ok() == to.canonicalize().ok() {
        return;
    }
    let tmp = to.with_extension("new");
    std::fs::copy(from, &tmp).and_then(|_| std::fs::rename(&tmp, to)).unwrap_or_else(|e| die(&format!("copy {}: {e}", to.display())));
}

/// Start the search window as the signed-in user. From an elevated
/// terminal it goes through Explorer, so files it opens don't run as
/// administrator.
fn start_window(win: &std::path::Path) {
    let w = win.display().to_string();
    if sys::elevated() {
        run_quiet("explorer.exe", &[&w]);
    } else {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new(win).creation_flags(0x0000_0008).spawn(); // DETACHED_PROCESS
    }
}

fn install(login: bool) {
    // A plain reinstall keeps an existing sign-in task.
    let login = login || task_exists();
    let bin = bin_dir().join("qasd.exe");
    let win = bin_dir().join("qasdw.exe");
    // Stop the old daemon and window so the new binaries can replace them.
    run_quiet("schtasks", &["/End", "/TN", TASK]);
    run_quiet("taskkill", &["/F", "/IM", "qasdw.exe"]);
    server::stop(&data_dir()).unwrap_or_else(|e| die(&e));
    std::fs::create_dir_all(bin_dir()).unwrap();
    let exe = std::env::current_exe().unwrap();
    put(&exe, &bin);
    let win_src = exe.with_file_name("qasdw.exe");
    let has_window = win_src.exists() || win.exists();
    if win_src.exists() {
        put(&win_src, &win);
    }
    let on_path = add_to_path(&bin_dir().display().to_string());
    let agents = format!("for coding agents: claude mcp add -s user qasd -- \"{}\" mcp", bin.display());
    let path_note = if on_path { "added to PATH (open a new terminal)" } else { "could not add it to PATH; add it by hand" };
    println!("installed {} ({path_note})", bin.display());
    // A sign-in task needs an administrator; without one, the search window
    // (or the first search) starts the daemon.
    if login && sys::elevated() {
        // At sign-in, with the highest rights: every folder is indexed (from
        // the MFT), and no UAC prompt shows each time.
        let tr = format!("\"{}\" serve", bin.display());
        let args = ["/Create", "/F", "/TN", TASK, "/SC", "ONLOGON", "/RL", "HIGHEST", "/TR", &tr];
        if !run_quiet("schtasks", &args) {
            die("schtasks /Create failed");
        }
        run_quiet("schtasks", &["/Run", "/TN", TASK]);
        println!("the daemon starts at sign-in as administrator");
    } else {
        let note = if login { " (a sign-in task needs an administrator terminal; the search window starts the daemon instead)" } else { "" };
        println!("the daemon starts on first use{note}");
    }
    if has_window {
        // The search window starts with Windows; Alt+Q opens it. Started
        // last, so it finds the elevated daemon instead of starting its own.
        let val = format!("\"{}\"", win.display());
        run_quiet("reg", &["add", RUN_KEY, "/v", "Qasd", "/t", "REG_SZ", "/d", &val, "/f"]);
        std::thread::sleep(std::time::Duration::from_millis(500));
        start_window(&win);
        println!("search window: press Alt+Q (starts with Windows; tray icon to quit)");
    }
    println!("{agents}");
}

fn uninstall() {
    run_quiet("schtasks", &["/End", "/TN", TASK]);
    let removed = run_quiet("schtasks", &["/Delete", "/F", "/TN", TASK]);
    run_quiet("reg", &["delete", RUN_KEY, "/v", "Qasd", "/f"]);
    run_quiet("taskkill", &["/F", "/IM", "qasdw.exe"]);
    let _ = server::stop(&data_dir());
    let what = if removed { format!("the sign-in task \"{TASK}\" and ") } else { String::new() };
    println!("removed {what}the search window's start with Windows; index kept in {}", data_dir().display());
}

fn die(msg: &str) -> ! {
    eprintln!("qasd: {msg}");
    std::process::exit(1)
}

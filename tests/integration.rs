//! End-to-end tests against a throwaway tmux server.
//!
//! Every test starts its own server on a private socket and kills it when
//! done, so your real tmux is never touched. Tests are skipped when tmux is
//! not installed.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::Duration;

const TMM: &str = env!("CARGO_BIN_EXE_tmm");

struct Server {
    socket: String,
    tmux_env: String,
    state: PathBuf,
}

impl Server {
    fn start(tag: &str) -> Option<Server> {
        if Command::new("tmux").arg("-V").output().is_err() {
            eprintln!("tmux is not installed; skipping");
            return None;
        }
        let socket = format!("tmm-test-{}-{tag}", std::process::id());
        let state = std::env::temp_dir().join(format!("{socket}-state"));
        fs::create_dir_all(&state).unwrap();
        let mut server = Server {
            socket,
            tmux_env: String::new(),
            state,
        };
        server.tmux(&[
            "-f",
            "/dev/null",
            "new-session",
            "-d",
            "-s",
            "alpha",
            "-x",
            "120",
            "-y",
            "40",
        ]);
        let path = server.tmux(&["display-message", "-p", "#{socket_path}"]);
        assert!(!path.is_empty(), "throwaway tmux server did not start");
        server.tmux_env = format!("{path},0,0");
        Some(server)
    }

    fn tmux(&self, args: &[&str]) -> String {
        let output = Command::new("tmux")
            .arg("-L")
            .arg(&self.socket)
            .args(args)
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string()
    }

    /// Run tmm the way fzf would inside a popup of this server.
    fn tmm_with(&self, args: &[&str], stdin: &str, extra: &[(&str, &str)]) -> Output {
        let mut command = Command::new(TMM);
        command
            .args(args)
            .env("TMUX", &self.tmux_env)
            .env("TMM_STATE_DIR", &self.state)
            .env("TMM_SNAPSHOT", self.state.join("snap.txt"))
            .env("FZF_PREVIEW_COLUMNS", "80")
            .env("FZF_PREVIEW_LINES", "24")
            .env("FZF_QUERY", "")
            .env_remove("TMUX_PANE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (key, value) in extra {
            command.env(key, value);
        }
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn tmm(&self, args: &[&str]) -> String {
        let output = self.tmm_with(args, "", &[]);
        assert!(
            output.status.success(),
            "tmm {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).to_string()
    }

    /// Open an inline prompt for `id` and answer it, as fzf would: the
    /// `prompt` transform first, then `enter` with the typed query.
    fn answer(&self, kind: &str, id: &str, text: &str) -> String {
        let opened = self.tmm(&["prompt", kind, id]);
        assert!(opened.starts_with("change-prompt("), "{opened}");
        let output = self.tmm_with(&["enter"], "", &[("FZF_QUERY", text)]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).to_string()
    }

    fn session_id(&self, name: &str) -> String {
        self.tmux(&[
            "display-message",
            "-p",
            "-t",
            &format!("{name}:"),
            "#{session_id}",
        ])
    }

    fn sessions(&self) -> Vec<String> {
        self.tmux(&["list-sessions", "-F", "#{session_name}"])
            .lines()
            .map(str::to_string)
            .collect()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = Command::new("tmux")
            .args(["-L", &self.socket, "kill-server"])
            .output();
        let _ = fs::remove_dir_all(&self.state);
    }
}

/// Text without its ANSI colour sequences.
fn plain(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('\x1b') {
        out.push_str(&rest[..start]);
        let end = rest[start..]
            .find('m')
            .map_or(rest.len(), |i| start + i + 1);
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

fn names(rows: &str) -> Vec<String> {
    rows.lines()
        .map(|line| line.split('\t').nth(1).unwrap().to_string())
        .collect()
}

#[test]
fn rows_favorites_help_and_refresh() {
    let Some(server) = Server::start("rows") else {
        return;
    };
    server.tmux(&["new-session", "-d", "-s", "beta", "-x", "120", "-y", "40"]);
    server.tmux(&["new-session", "-d", "-s", "gamma", "-x", "120", "-y", "40"]);
    server.tmux(&["new-window", "-d", "-t", "beta:"]);

    let rows = server.tmm(&["list"]);
    assert_eq!(names(&rows), ["alpha", "beta", "gamma"]);
    let beta: Vec<&str> = rows.lines().nth(1).unwrap().split('\t').collect();
    assert_eq!(beta[2].trim(), "beta");
    assert_eq!(
        beta[3].trim(),
        "beta",
        "sessions mode: both name columns alike"
    );
    assert_eq!(beta[4].matches('●').count(), 0, "no agent, no dot");
    assert_eq!(
        beta[4], "  ",
        "the dots column keeps one cell without agents"
    );
    assert!(beta[5].contains("2 windows"));
    let name_width = |row: &str| row.split('\t').nth(2).unwrap().len();
    assert_eq!(
        name_width(rows.lines().next().unwrap()),
        name_width(rows.lines().nth(2).unwrap()),
        "names are padded"
    );

    server.tmm(&["favorite", "toggle", "gamma"]);
    let rows = server.tmm(&["list"]);
    assert_eq!(names(&rows), ["gamma", "alpha", "beta"]);
    assert!(
        rows.lines()
            .next()
            .unwrap()
            .split('\t')
            .nth(2)
            .unwrap()
            .starts_with("★ gamma")
    );
    assert!(
        fs::read_to_string(server.state.join("switcher.json"))
            .unwrap()
            .contains("gamma")
    );
    server.tmm(&["favorite", "unstar", "gamma"]);
    assert_eq!(names(&server.tmm(&["list"])), ["alpha", "beta", "gamma"]);

    assert_eq!(server.tmm(&["help"]), "", "first toggle hides the legend");
    assert!(server.state.join("snap.help").exists());
    assert!(
        server.tmm(&["help"]).contains("ctrl-j/k move"),
        "second toggle restores it"
    );
    assert!(!server.state.join("snap.help").exists());

    let snapshot = server.state.join("snap.txt");
    fs::write(&snapshot, "stale\n").unwrap();
    let idle = [("FZF_IDLE_TIME_MS", "1500")];
    let refresh = || {
        let output = server.tmm_with(&["refresh", snapshot.to_str().unwrap()], "", &idle);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).to_string()
    };
    assert!(refresh().starts_with("reload-sync(cat "));
    assert_eq!(
        fs::read_to_string(&snapshot).unwrap(),
        server.tmm(&["list"])
    );
    assert_eq!(refresh(), "", "unchanged rows do not reload");
    let busy = server.tmm_with(
        &["refresh", snapshot.to_str().unwrap()],
        "",
        &[("FZF_IDLE_TIME_MS", "200")],
    );
    assert_eq!(
        String::from_utf8_lossy(&busy.stdout),
        "",
        "no reload while the user is moving"
    );
}

#[test]
fn inline_prompts_and_closing() {
    let Some(server) = Server::start("prompts") else {
        return;
    };
    let alpha = server.session_id("alpha");

    let opened = server.tmm(&["prompt", "create", &alpha]);
    assert!(
        opened.contains("change-prompt(create> )") && opened.contains("change-query()"),
        "{opened}"
    );
    assert!(
        opened.contains("change-header(New session") && opened.contains("disable-search"),
        "{opened}"
    );
    assert!(server.state.join("snap.prompt").exists());

    let done = server.answer("create", &alpha, "delta");
    assert!(
        done.contains("change-prompt(sessions> )")
            && done.contains("enable-search")
            && done.contains("reload-sync("),
        "{done}"
    );
    assert!(
        !server.state.join("snap.prompt").exists(),
        "answering clears the pending prompt"
    );
    assert!(server.sessions().contains(&"delta".to_string()));
    let nothing = server.answer("create", &alpha, "");
    assert!(
        !nothing.contains("reload-sync"),
        "an empty name creates nothing: {nothing}"
    );

    let opened = server.tmm(&["prompt", "rename", &alpha]);
    assert!(
        opened.contains("change-prompt(rename> )") && opened.contains("change-query(alpha)"),
        "prefilled: {opened}"
    );
    server.answer("rename", &alpha, "omega");
    assert!(
        server.sessions().contains(&"omega".to_string())
            && !server.sessions().contains(&"alpha".to_string())
    );

    let delta = server.session_id("delta");
    let closed = server.tmm(&["close", &delta]);
    assert!(
        closed.starts_with("reload-sync("),
        "closing needs no confirmation: {closed}"
    );
    assert!(!server.sessions().contains(&"delta".to_string()));
    let missing = server.tmm(&["close", "$999"]);
    assert!(missing.starts_with("change-header(tmm:"), "{missing}");

    assert!(
        server
            .tmm(&["mode", "toggle"])
            .starts_with("change-prompt(windows> )+reload-sync(")
    );
    assert!(
        server
            .tmm(&["mode", "toggle"])
            .starts_with("change-prompt(projects> )+reload-sync(")
    );
    assert!(
        server
            .tmm(&["mode", "toggle"])
            .starts_with("change-prompt(sessions> )+reload-sync(")
    );

    // fzf hands over an empty {1} when nothing matches the filter; tmux would
    // read that as the current session, so the row keys must do nothing.
    let before = server.sessions();
    assert_eq!(server.tmm(&["close", ""]), "");
    assert_eq!(server.tmm(&["prompt", "rename", ""]), "");
    assert_eq!(server.tmm(&["preview", ""]), "");
    server.tmm(&["preview-next", ""]);
    assert_eq!(server.sessions(), before);

    // The filter typed before a prompt comes back afterwards.
    let omega = server.session_id("omega");
    let opened = server.tmm_with(&["prompt", "rename", &omega], "", &[("FZF_QUERY", "ome")]);
    assert!(String::from_utf8_lossy(&opened.stdout).contains("change-query(omega)"));
    let cancelled = server.tmm(&["esc"]);
    assert!(cancelled.contains("change-query(ome)"), "{cancelled}");
    server.tmm_with(&["prompt", "rename", &omega], "", &[("FZF_QUERY", "ome")]);
    let answered = server.tmm_with(&["enter"], "", &[("FZF_QUERY", "omega")]);
    assert!(String::from_utf8_lossy(&answered.stdout).contains("change-query(ome)"));

    // Tab cancels an open prompt along with switching the mode.
    server.tmm(&["prompt", "rename", &omega]);
    let switched = server.tmm(&["mode", "toggle"]);
    assert!(
        switched.contains("change-prompt(windows> )") && switched.contains("change-header()"),
        "{switched}"
    );
    assert!(!server.state.join("snap.prompt").exists());
    server.tmm(&["mode", "sessions"]);

    assert_eq!(
        server.tmm(&["enter"]).trim(),
        "accept",
        "Enter outside a prompt accepts the row"
    );
    assert_eq!(
        server.tmm(&["esc"]).trim(),
        "abort",
        "Esc outside a prompt closes the popup"
    );
    server.tmm(&["prompt", "rename", &server.session_id("omega")]);
    let cancelled = server.tmm(&["esc"]);
    assert!(
        cancelled.contains("change-header()") && cancelled.contains("enable-search"),
        "{cancelled}"
    );
    assert!(
        !server.state.join("snap.prompt").exists(),
        "Esc drops the pending prompt"
    );

    // The tree hides a window's session; filtering shows the breadcrumbs.
    assert_eq!(server.tmm(&["with-nth"]).trim(), "3,5,6");
    let filtering = server.tmm_with(&["with-nth"], "", &[("FZF_QUERY", "om")]);
    assert_eq!(String::from_utf8_lossy(&filtering.stdout).trim(), "4,5,6");
    // While a prompt borrows the query line, the filter it saved decides.
    server.tmm(&["prompt", "rename", &server.session_id("omega")]);
    let typing = server.tmm_with(&["with-nth"], "", &[("FZF_QUERY", "omeg")]);
    assert_eq!(String::from_utf8_lossy(&typing.stdout).trim(), "3,5,6");
    server.tmm(&["esc"]);
}

#[test]
fn windows_mode_lists_manages_and_previews_windows() {
    let Some(server) = Server::start("windows") else {
        return;
    };
    server.tmux(&["new-window", "-d", "-t", "alpha:", "-n", "editor"]);
    server.tmux(&["new-session", "-d", "-s", "beta", "-x", "120", "-y", "40"]);
    fs::write(server.state.join("snap.mode"), "windows").unwrap();

    let rows = server.tmm(&["list"]);
    let ids: Vec<String> = rows
        .lines()
        .map(|line| line.split('\t').next().unwrap().to_string())
        .collect();
    assert_eq!(
        ids.len(),
        5,
        "alpha, its two windows, beta, its window: {rows}"
    );
    assert!(
        ids[0].starts_with('$')
            && ids[1].starts_with('@')
            && ids[2].starts_with('@')
            && ids[3].starts_with('$')
    );
    let header: Vec<&str> = rows.lines().next().unwrap().split('\t').collect();
    assert!(
        header[2].contains("\x1b[1malpha"),
        "session rows are bold headers: {:?}",
        header[2]
    );
    let first: Vec<&str> = rows.lines().nth(1).unwrap().split('\t').collect();
    assert!(
        plain(first[2]).starts_with("  ├ 0 → "),
        "the arrow marks the session's current window: {:?}",
        first[2]
    );
    assert_eq!(first[5], "", "no badge beyond the arrow: {:?}", first[5]);
    let editor: Vec<&str> = rows.lines().nth(2).unwrap().split('\t').collect();
    assert_eq!(
        editor[1], "alpha",
        "window rows carry their session name for ctrl-s"
    );
    assert_eq!(
        plain(editor[2]).trim(),
        "└ 1   editor",
        "the last window closes the tree"
    );
    assert_eq!(
        plain(editor[3]).trim(),
        "alpha:1   editor",
        "the breadcrumb column names the session"
    );
    let lone: Vec<&str> = rows.lines().nth(4).unwrap().split('\t').collect();
    assert!(
        plain(lone[2]).starts_with("  └ 0   "),
        "a session with one window has nothing to point at: {:?}",
        lone[2]
    );
    assert_eq!(
        plain(editor[2]).chars().count(),
        plain(editor[3]).chars().count(),
        "both name columns share a width"
    );
    assert_eq!(editor[4].matches('●').count(), 0, "no agent, no dot");
    assert_eq!(editor[5], "", "a lone pane is not worth a badge");

    let editor_id = ids[2].clone();
    let preview = server.tmm(&["preview", &editor_id]);
    assert!(preview.contains("alpha  ·  1: editor"), "{preview:?}");
    server.tmm(&["preview-next", &editor_id]);
    assert!(
        server.tmm(&["preview", &editor_id]).contains("1: editor"),
        "window rows do not cycle"
    );

    let opened = server.tmm(&["prompt", "rename", &editor_id]);
    assert!(
        opened.contains("change-query(editor)") && opened.contains("Rename window 1: editor"),
        "{opened}"
    );
    server.answer("rename", &editor_id, "code");
    assert_eq!(
        server.tmux(&["display-message", "-p", "-t", &editor_id, "#{window_name}"]),
        "code"
    );

    let beta = server.session_id("beta");
    let opened = server.tmm(&["prompt", "create", &beta]);
    assert!(opened.contains("New window in beta"), "{opened}");
    server.answer("create", &beta, "logs");
    assert_eq!(
        server
            .tmux(&["list-windows", "-t", "beta", "-F", "#{window_name}"])
            .lines()
            .count(),
        2
    );
    server.answer("create", &beta, "");
    assert_eq!(
        server
            .tmux(&["list-windows", "-t", "beta", "-F", "#{window_name}"])
            .lines()
            .count(),
        3,
        "an unnamed window is fine"
    );

    server.tmux(&["split-window", "-d", "-t", &editor_id]);
    let listed = server.tmm(&["list"]);
    let split: Vec<&str> = listed.lines().nth(2).unwrap().split('\t').collect();
    assert!(split[5].contains("2 panes"), "{:?}", split[5]);

    assert!(
        server
            .tmm(&["close", &editor_id])
            .starts_with("reload-sync(")
    );
    assert_eq!(
        server
            .tmux(&["list-windows", "-t", "alpha", "-F", "#{window_id}"])
            .lines()
            .count(),
        1
    );
    let last = server.tmux(&["display-message", "-p", "-t", "alpha:0", "#{window_id}"]);
    assert!(
        server.tmm(&["close", &last]).starts_with("reload-sync("),
        "closing the last window"
    );
    assert!(
        !server.sessions().contains(&"alpha".to_string()),
        "which ends its session"
    );

    let renamed = server.answer("rename", &beta, "gamma");
    assert!(renamed.contains("reload-sync("), "{renamed}");
    assert!(server.sessions().contains(&"gamma".to_string()));

    server.tmm(&["mode", "sessions"]);
    assert!(!server.state.join("snap.mode").exists());
    assert_eq!(
        server.tmm(&["list"]).lines().count(),
        1,
        "sessions mode again, alpha gone"
    );
    server.tmm(&["mode", "windows"]);
    assert_eq!(
        server.tmm(&["list"]).lines().count(),
        4,
        "gamma and its three windows"
    );
}

#[test]
fn preview_cycles_without_changing_the_active_window() {
    let Some(server) = Server::start("preview") else {
        return;
    };
    server.tmux(&["new-session", "-d", "-s", "beta", "-x", "120", "-y", "40"]);
    server.tmux(&["new-window", "-d", "-t", "beta:"]);
    let alpha = server.session_id("alpha");
    let beta = server.session_id("beta");

    let preview = server.tmm(&["preview", &alpha]);
    assert!(
        preview.contains("alpha") && preview.contains("0:") && !preview.contains("\x1b[2J"),
        "{preview:?}"
    );
    assert!(
        !preview.ends_with('\n'),
        "no trailing newline, or fzf shows a scroll indicator"
    );

    let beta_preview = || server.tmm(&["preview", &beta]);
    server.tmm(&["preview-next", &beta]);
    assert!(beta_preview().contains("1:"), "moves on");
    server.tmm(&["preview-next", &beta]);
    assert!(beta_preview().contains("0:"), "wraps");
    assert_eq!(
        server.tmux(&["display-message", "-p", "-t", "beta:", "#{window_index}"]),
        "0",
        "tmux's active window is untouched"
    );
    server.tmm(&["preview-next", &beta]);
    server.tmux(&["kill-window", "-t", "beta:1"]);
    assert!(
        beta_preview().contains("0:"),
        "a closed preview window falls back to the active one"
    );
    assert!(
        server.tmm(&["preview", "$999"]).contains("Gone"),
        "a vanished row previews a notice"
    );

    fs::write(server.state.join("snap.view"), "hide-preview").unwrap();
    assert_eq!(server.tmm(&["view-return"]).trim(), "hide-preview");
    assert!(
        !server.state.join("snap.view").exists(),
        "the action is replayed once"
    );
}

#[test]
fn agent_activity_from_hooks_and_from_the_screen() {
    let Some(server) = Server::start("agent") else {
        return;
    };
    // A shell whose argv[0] is ".../claude": `ps` shows it as an agent, and
    // hooks typed into it run as its children, the way real hooks do. tmux
    // still reports the real command name, so the cheap pre-filter would skip
    // it; the full scan is forced for this test.
    let fake = server.state.join("claude");
    std::os::unix::fs::symlink("/bin/sh", &fake).unwrap();
    let scan = [("TMM_AGENT_SCAN", "always")];
    server.tmux(&[
        "new-window",
        "-d",
        "-t",
        "alpha:",
        "-n",
        "agent",
        fake.to_str().unwrap(),
    ]);
    thread::sleep(Duration::from_millis(300));
    let pane = server.tmux(&["display-message", "-p", "-t", "alpha:1", "#{pane_id}"]);
    let pid = server.tmux(&["display-message", "-p", "-t", "alpha:1", "#{pane_pid}"]);

    let activity = || {
        let output = server.tmm_with(&["activity"], "", &scan);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    assert_eq!(
        activity(),
        r#"{"@0":"plain","@1":"waiting"}"#,
        "an agent with no busy hint is waiting"
    );

    let record = || server.tmux(&["show-options", "-pqv", "-t", &pane, "@tmm-agent"]);
    let hook = |event: &str| {
        server.tmux(&[
            "send-keys",
            "-t",
            &pane,
            &format!("{TMM} hook claude {event}"),
            "Enter",
        ]);
        for _ in 0..30 {
            if record().contains(&format!("\"event\":\"{event}\"")) {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
        panic!("hook {event} left no record: {}", record());
    };
    hook("UserPromptSubmit");
    assert!(
        record().contains("\"working\"") && record().contains(&format!("\"pid\":{pid}")),
        "{}",
        record()
    );
    assert_eq!(activity(), r#"{"@0":"plain","@1":"working"}"#);
    let session_dots = || {
        let output = server.tmm_with(&["list"], "", &scan);
        let rows = String::from_utf8(output.stdout).unwrap();
        rows.lines()
            .next()
            .unwrap()
            .split('\t')
            .nth(4)
            .unwrap()
            .to_string()
    };
    assert_eq!(
        session_dots(),
        "\x1b[32m●\x1b[0m ",
        "a working agent is one plain green dot"
    );
    hook("Stop");
    assert_eq!(activity(), r#"{"@0":"plain","@1":"waiting"}"#);
    assert_eq!(
        session_dots(),
        "\x1b[33m●\x1b[0m ",
        "a waiting agent is one plain yellow dot"
    );

    server.tmux(&["send-keys", "-t", &pane, "esc to interrupt", "Enter"]);
    thread::sleep(Duration::from_millis(300));
    hook("PermissionRequest");
    assert_eq!(
        activity(),
        r#"{"@0":"plain","@1":"working"}"#,
        "a busy hint after a permission hook means it resumed"
    );

    server.tmux(&[
        "set-option",
        "-p",
        "-t",
        &pane,
        "@tmm-agent",
        r#"{"state":"waiting","pid":1,"started":"never","provider":"claude","event":"Stop"}"#,
    ]);
    assert_eq!(
        activity(),
        r#"{"@0":"plain","@1":"working"}"#,
        "a record for another process is ignored"
    );

    hook("SessionEnd");
    assert_eq!(activity(), r#"{"@0":"plain","@1":"plain"}"#);
    assert_eq!(
        session_dots().matches('●').count(),
        0,
        "an ended agent leaves no dot"
    );
}

#[test]
fn projects_mode_lists_and_opens_directories() {
    let Some(server) = Server::start("projects") else {
        return;
    };
    let root = server.state.join("projects");
    for dir in ["alpha-app", "beta.app", "nested/alpha-app", ".hidden"] {
        fs::create_dir_all(root.join(dir)).unwrap();
    }
    fs::write(root.join("a-file"), "").unwrap();
    server.tmux(&[
        "set",
        "-g",
        "@tmm-projects",
        &format!("{}:2", root.display()),
    ]);
    let root = fs::canonicalize(&root).unwrap();

    server.tmm(&["mode", "projects"]);
    assert_eq!(
        fs::read_to_string(server.state.join("snap.mode")).unwrap(),
        "projects"
    );
    let rows = server.tmm(&["list"]);
    assert_eq!(
        names(&rows),
        ["alpha-app", "alpha-app", "beta_app", "nested"],
        "{rows}"
    );
    let ids: Vec<&str> = rows
        .lines()
        .map(|line| line.split('\t').next().unwrap())
        .collect();
    assert_eq!(
        ids[0],
        root.join("alpha-app").to_str().unwrap(),
        "a project with no session has its path for an id"
    );
    assert_eq!(ids[1], root.join("nested/alpha-app").to_str().unwrap());
    let alpha: Vec<&str> = rows.lines().next().unwrap().split('\t').collect();
    assert_eq!(plain(alpha[4]).trim(), "", "no session, no dots");
    assert_eq!(
        plain(alpha[5]),
        root.to_str().unwrap(),
        "the badge says where it lives"
    );

    // Enter on a project row: find or create its session.
    let top = root.join("alpha-app");
    let id = server
        .tmm(&["open", top.to_str().unwrap()])
        .trim()
        .to_string();
    assert!(id.starts_with('$'), "{id}");
    assert!(server.sessions().contains(&"alpha-app".to_string()));
    assert_eq!(
        server.tmm(&["open", top.to_str().unwrap()]).trim(),
        id,
        "opening again finds the same session"
    );
    // A namesake started elsewhere gets its parent's name appended.
    let nested = root.join("nested/alpha-app");
    server.tmm(&["open", nested.to_str().unwrap()]);
    assert!(
        server.sessions().contains(&"alpha-app-nested".to_string()),
        "{:?}",
        server.sessions()
    );

    let rows = server.tmm(&["list"]);
    assert_eq!(
        names(&rows),
        ["alpha-app", "alpha-app-nested", "beta_app", "nested"],
        "open projects first: {rows}"
    );
    assert_eq!(
        rows.lines().count(),
        4,
        "sessions outside the roots are not projects"
    );
    let first: Vec<&str> = rows.lines().next().unwrap().split('\t').collect();
    assert_eq!(first[0], id, "an open project is its session's row");
    assert_eq!(first[4].matches('●').count(), 0, "no agent, no dot");
    assert!(plain(first[5]).contains("1 window"), "{:?}", first[5]);

    server.tmm(&["favorite", "toggle", "beta_app"]);
    let rows = server.tmm(&["list"]);
    assert_eq!(
        names(&rows),
        ["beta_app", "alpha-app", "alpha-app-nested", "nested"],
        "starred first: {rows}"
    );
    assert!(
        plain(rows.lines().next().unwrap().split('\t').nth(2).unwrap()).starts_with("★ beta_app")
    );

    // A project with no session previews its directory, and the row keys
    // leave it alone.
    let beta = root.join("beta.app");
    fs::create_dir(beta.join("src")).unwrap();
    fs::write(beta.join("notes.md"), "").unwrap();
    let preview_output = server.tmm_with(
        &["preview", beta.to_str().unwrap()],
        "",
        &[("FZF_PREVIEW_COLUMNS", "512")],
    );
    assert!(
        preview_output.status.success(),
        "{}",
        String::from_utf8_lossy(&preview_output.stderr)
    );
    let preview = String::from_utf8_lossy(&preview_output.stdout).to_string();
    assert!(
        preview.contains("beta.app")
            && plain(&preview).contains("src/")
            && preview.contains("notes.md"),
        "{preview:?}"
    );
    assert_eq!(server.tmm(&["close", beta.to_str().unwrap()]), "");
    assert_eq!(
        server.tmm(&["prompt", "rename", beta.to_str().unwrap()]),
        ""
    );
    server.tmm(&["preview-next", beta.to_str().unwrap()]);
    assert!(server.sessions().contains(&"alpha-app".to_string()));
    // An open project's row is its session's row, so ctrl-x closes it.
    assert!(server.tmm(&["close", &id]).starts_with("reload-sync("));
    assert!(!server.sessions().contains(&"alpha-app".to_string()));

    // tab cycles sessions → windows → projects → sessions.
    server.tmm(&["mode", "sessions"]);
    for prompt in ["windows> ", "projects> ", "sessions> "] {
        let switched = server.tmm(&["mode", "toggle"]);
        assert!(
            switched.contains(&format!("change-prompt({prompt})")),
            "{switched}"
        );
    }
}

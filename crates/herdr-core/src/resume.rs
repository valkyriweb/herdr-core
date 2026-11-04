//! Agents come back after a restart (opt-in: `resume_agents = true` in the config).
//! While pi, pii, Claude Code or Codex runs in a kept terminal, the core records
//! how to reopen its session; when recovery replaces the lost program, it runs
//! that in the terminal's shell instead of a bare shell. Deciding is pure
//! (`plan`); reading session files is behind `Lookup`.
//!
//! This is the one place core names agents: Herdr 1 did the same with
//! `resume_agents_on_restore`, and nothing else in core depends on it.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// How to reopen the agent session last seen in a terminal.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct Resume {
    /// The command that reopens the session.
    pub argv: Vec<String>,
    /// Where it runs.
    pub cwd: String,
    /// The agent process it was read from, and that process's name.
    pub pid: i32,
    pub name: String,
    /// The terminal's turn when it was read.
    pub turn: u64,
}

/// What the core knows about one terminal's agent.
pub struct Facts<'a> {
    /// The program's OSC 7501 `app` (`pi`, `claude-code`, `codex`).
    pub app: &'a str,
    pub argv: &'a [String],
    pub fg_argv: &'a [String],
    pub cwd: &'a str,
    pub pid: i32,
    pub labels: &'a BTreeMap<String, String>,
    /// `PI_CODING_AGENT_DIR` from the agent's environment, when it has one.
    pub agent_dir: Option<String>,
}

pub trait Lookup {
    fn home(&self) -> PathBuf;
    /// The most recently written session file in a pi sessions folder for this cwd.
    fn newest(&self, dir: &Path) -> Option<String>;
    fn read(&self, path: &Path) -> Option<String>;
    /// Files this process has open.
    fn open_files(&self, pid: i32) -> Vec<String>;
    /// Paths matching a glob.
    fn glob(&self, pattern: &str) -> Vec<String>;
}

const SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "ksh", "fish", "-sh", "-bash", "-zsh"];
const KEEP_PI: &[(&str, bool)] = &[("--model", true), ("--provider", true), ("--thinking", true)];
const KEEP_CLAUDE: &[(&str, bool)] = &[
    ("--model", true),
    ("--effort", true),
    ("--permission-mode", true),
    ("--add-dir", true),
    ("--dangerously-skip-permissions", false),
];
const KEEP_CODEX: &[(&str, bool)] = &[
    ("-m", true),
    ("--model", true),
    ("-c", true),
    ("--config", true),
    ("-s", true),
    ("--sandbox", true),
    ("--dangerously-bypass-approvals-and-sandbox", false),
    ("--no-daemon", false),
];
/// Other flags these agents take a value for, so a value is never read as the prompt.
const VALUED: &[&str] = &[
    "--session",
    "--session-id",
    "--resume",
    "-r",
    "--name",
    "-n",
    "--append-system-prompt",
    "--system-prompt",
    "--profile",
    "-p",
    "-a",
    "--ask-for-approval",
    "-C",
    "--cd",
    "--settings",
    "--mcp-config",
    "--session-dir",
];

fn base(s: &str) -> &str {
    s.rsplit(['/', '\\']).next().unwrap_or(s)
}

fn is_uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.iter().map(|p| p.len()).eq([8, 4, 4, 4, 12]) && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// argv, with only a shell wrapper's -c script split, so wrapped launches count too.
fn tokens(argv: &[String]) -> Vec<String> {
    if argv.len() >= 3
        && SHELLS.contains(&base(&argv[0]))
        && argv[1] == "-c"
        && let Some(words) = words(&argv[2])
    {
        return argv[..2].iter().cloned().chain(words).chain(argv[3..].iter().cloned()).collect();
    }
    argv.to_vec()
}

/// A shell script's words (quotes and backslashes), up to its first `;`, `&`, `|` or newline.
fn words(script: &str) -> Option<Vec<String>> {
    let (mut out, mut word, mut quote, mut any) = (vec![], String::new(), None, false);
    let mut chars = script.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') => word.push(chars.next()?),
            (Some(_), c) => word.push(c),
            (None, '\'' | '"') => (quote, any) = (Some(c), true),
            (None, '\\') => (word, any) = (word + &chars.next()?.to_string(), true),
            (None, ';' | '&' | '|' | '\n') => break,
            (None, c) if c.is_whitespace() => {
                if any || !word.is_empty() {
                    out.push(std::mem::take(&mut word));
                }
                any = false;
            }
            (None, c) => word.push(c),
        }
    }
    if quote.is_some() {
        return None;
    }
    if any || !word.is_empty() {
        out.push(word);
    }
    Some(out)
}

/// Executable names only (argv[0] and what a shell -c runs), never prompt words, with the index after it.
fn command(toks: &[String]) -> Option<(&str, usize)> {
    let first = base(toks.first()?);
    if toks.len() >= 3 && SHELLS.contains(&first) && toks[1] == "-c" {
        let at = (2..toks.len())
            .find(|&i| !matches!(toks[i].as_str(), "exec" | "env" | "command") && !toks[i].contains('='))?;
        return Some((base(&toks[at]), at + 1));
    }
    Some((first, 1))
}

fn kind_of(app: &str, argvs: [&[String]; 2]) -> Option<&'static str> {
    let names: Vec<&str> = argvs.iter().filter_map(|a| command(a).map(|(n, _)| n)).collect();
    if names.contains(&"pii") {
        return Some("pii");
    }
    match app {
        "pi" => return Some("pi"),
        "claude" | "claude-code" => return Some("claude"),
        "codex" => return Some("codex"),
        _ => {}
    }
    names.iter().find_map(|n| ["pi", "claude", "codex"].into_iter().find(|k| k == n))
}

/// The launch's options: after the agent's name, up to its first operand (the prompt).
fn options(kind: &str, toks: &[String]) -> Vec<String> {
    let Some((name, start)) = command(toks) else { return vec![] };
    let pi = matches!(kind, "pi" | "pii");
    if !(name == kind || (pi && matches!(name, "pi" | "pii"))) {
        return vec![];
    }
    let valued = |t: &str| {
        VALUED.contains(&t) || [KEEP_PI, KEEP_CLAUDE, KEEP_CODEX].iter().any(|k| k.iter().any(|(f, v)| *v && *f == t))
    };
    let (mut out, mut i) = (vec![], start);
    while i < toks.len() {
        let t = &toks[i];
        if !t.starts_with('-') {
            if kind == "codex" && t == "resume" {
                i += if toks.get(i + 1).is_some_and(|s| is_uuid(s)) { 2 } else { 1 };
                continue;
            }
            break;
        }
        if valued(t) && i + 1 < toks.len() {
            out.extend([t.clone(), toks[i + 1].clone()]);
            i += 2;
        } else {
            out.push(t.clone());
            i += 1;
        }
    }
    out
}

fn kept(kind: &str, opts: &[String]) -> Vec<String> {
    let keep = match kind {
        "claude" => KEEP_CLAUDE,
        "codex" => KEEP_CODEX,
        _ => KEEP_PI,
    };
    let (mut out, mut i) = (vec![], 0);
    while i < opts.len() {
        let name = opts[i].split('=').next().unwrap_or_default();
        let hit = keep.iter().find(|(f, _)| *f == name);
        let takes = hit.is_some_and(|(_, v)| *v) && !opts[i].contains('=') || VALUED.contains(&opts[i].as_str());
        let n = if takes { 2 } else { 1 };
        if hit.is_some() {
            out.extend(opts[i..(i + n).min(opts.len())].iter().cloned());
        }
        i += n;
    }
    out
}

fn flag(opts: &[String], names: &[&str]) -> Option<String> {
    opts.iter().enumerate().find_map(|(i, t)| {
        names.iter().find_map(|n| {
            if t == n { opts.get(i + 1).cloned() } else { t.strip_prefix(&format!("{n}=")).map(String::from) }
        })
    })
}

fn has(opts: &[String], names: &[&str]) -> bool {
    opts.iter().any(|t| names.contains(&t.split('=').next().unwrap_or_default()))
}

/// pi's sessions folder name for a cwd.
pub fn pi_slug(cwd: &str) -> String {
    format!("--{}--", cwd.trim_matches('/').replace('/', "-"))
}

/// The last value of `model` that `pick` finds in a JSONL transcript.
fn last_model(text: &str, pick: impl Fn(&serde_json::Value) -> Option<&str>) -> Option<String> {
    text.lines()
        .filter(|l| l.contains("\"model\""))
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|v| pick(&v).map(String::from))
        .last()
}

/// The command that reopens this agent's session, or None if this isn't one or its session is unknown.
pub fn plan(f: &Facts, look: &impl Lookup) -> Option<Vec<String>> {
    let (hc, fg) = (tokens(f.argv), tokens(f.fg_argv));
    let mut kind = kind_of(f.app, [&hc, &fg])?;
    // The process's own argv is the best record, unless it hid it (pi retitles itself).
    let fg_says = command(&fg).is_some_and(|(n, at)| {
        ["pi", "pii", "claude", "codex"].contains(&n) && fg.get(at..).is_some_and(|r| r.iter().any(|a| !a.is_empty()))
    });
    let toks = if fg_says { &fg } else { &hc };
    let home = look.home();
    match kind {
        "pi" | "pii" => {
            if f.agent_dir.as_deref().is_some_and(|d| base(d.trim_end_matches('/')) == "pii") {
                kind = "pii";
            }
            let opts = options(kind, toks);
            let sid = flag(&opts, &["--session-id", "--session"])
                .or_else(|| f.labels.get("harbor.harness-session-id").cloned())
                .or_else(|| {
                    let dir = f
                        .agent_dir
                        .clone()
                        .map(PathBuf::from)
                        .unwrap_or_else(|| home.join(if kind == "pii" { ".pi/pii" } else { ".pi/agent" }));
                    look.newest(&dir.join("sessions").join(pi_slug(f.cwd)))
                })?;
            Some([kind.to_string(), "--session".into(), sid].into_iter().chain(kept(kind, &opts)).collect())
        }
        "claude" => {
            let opts = options(kind, toks);
            let sid = look
                .read(&home.join(".claude/sessions").join(format!("{}.json", f.pid)))
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                .and_then(|v| v.get("sessionId")?.as_str().map(String::from))
                .or_else(|| flag(&opts, &["--session-id", "--resume", "-r"]))?;
            let mut flags = kept(kind, &opts);
            if !has(&flags, &["--model"]) {
                let transcript = look.glob(&format!("{}/.claude/projects/*/{sid}.jsonl", home.display()));
                let model = transcript
                    .first()
                    .and_then(|p| look.read(Path::new(p)))
                    .and_then(|t| last_model(&t, |v| v.get("message")?.get("model")?.as_str()));
                if let Some(m) = model {
                    flags.extend(["--model".into(), m]);
                }
            }
            Some(["claude".into(), "--resume".into(), sid].into_iter().chain(flags).collect())
        }
        _ => {
            let opts = options(kind, toks);
            let rollout =
                look.open_files(f.pid).into_iter().find(|p| base(p).starts_with("rollout-") && p.ends_with(".jsonl"));
            let sid = rollout
                .as_deref()
                .and_then(|p| {
                    base(p).trim_end_matches(".jsonl").get(base(p).len().saturating_sub(42)..).map(String::from)
                })
                .filter(|s| is_uuid(s))
                .or_else(|| {
                    let at = toks.iter().position(|t| t == "resume")?;
                    toks.get(at + 1).filter(|s| is_uuid(s)).cloned()
                })?;
            let mut flags = kept(kind, &opts);
            if !has(&flags, &["--no-daemon"]) {
                flags.push("--no-daemon".into());
            }
            // Name the recorded provider and model: codex refuses a resume whose provider differs.
            let path = rollout
                .or_else(|| look.glob(&format!("{}/.codex/sessions/*/*/*/rollout-*{sid}.jsonl", home.display())).pop());
            if let Some(text) = path.and_then(|p| look.read(Path::new(&p))) {
                let provider = text
                    .lines()
                    .next()
                    .and_then(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                    .and_then(|v| v.get("payload")?.get("model_provider")?.as_str().map(String::from));
                if let Some(p) = provider.filter(|_| !flags.iter().any(|f| f.contains("model_provider"))) {
                    flags.extend(["-c".into(), format!("model_provider=\"{p}\"")]);
                }
                let model = last_model(&text, |v| {
                    (v.get("type")?.as_str()? == "turn_context").then_some(())?;
                    v.get("payload")?.get("model")?.as_str()
                });
                if let Some(m) = model.filter(|_| !has(&flags, &["-m", "--model"])) {
                    flags.extend(["-m".into(), m]);
                }
            }
            Some(["codex".into(), "resume".into(), sid].into_iter().chain(flags).collect())
        }
    }
}

/// The recovery argv: the shell runs the resume command, then stays as the
/// terminal's shell. It runs interactive (`-i`), so the agent gets the same
/// rc-file environment as one typed at a prompt. None for shells without POSIX-style `-c`.
pub fn launch(shell: &[String], argv: &[String]) -> Option<Vec<String>> {
    if !SHELLS.contains(&base(shell.first()?)) {
        return None;
    }
    let q = |a: &[String]| a.iter().map(|s| quote(s)).collect::<Vec<_>>().join(" ");
    let script = format!("{}; exec {}", q(argv), q(shell));
    let interactive = shell.iter().any(|a| a == "-i").then_some(vec![]).unwrap_or_else(|| vec!["-i".to_string()]);
    Some(shell.iter().cloned().chain(interactive).chain(["-c".into(), script]).collect())
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Session files on this machine.
pub struct Disk;

impl Lookup for Disk {
    fn home(&self) -> PathBuf {
        std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
    }
    fn newest(&self, dir: &Path) -> Option<String> {
        std::fs::read_dir(dir)
            .ok()?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
            .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
            .max()
            .map(|(_, p)| p.display().to_string())
    }
    fn read(&self, path: &Path) -> Option<String> {
        std::fs::read(path).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
    }
    fn open_files(&self, pid: i32) -> Vec<String> {
        crate::platform::open_files(pid)
    }
    fn glob(&self, pattern: &str) -> Vec<String> {
        glob_paths(pattern)
    }
}

/// A small glob: `*` matches one path component (or part of one).
fn glob_paths(pattern: &str) -> Vec<String> {
    let mut found = vec![PathBuf::from("/")];
    for part in pattern.trim_start_matches('/').split('/') {
        let mut next = vec![];
        for dir in found {
            if !part.contains('*') {
                let p = dir.join(part);
                if p.exists() {
                    next.push(p);
                }
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for e in entries.flatten() {
                if wild(part, &e.file_name().to_string_lossy()) {
                    next.push(e.path());
                }
            }
        }
        found = next;
    }
    found.into_iter().map(|p| p.display().to_string()).collect()
}

fn wild(pattern: &str, name: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut rest = name;
    for (i, p) in parts.iter().enumerate() {
        if i == 0 {
            let Some(r) = rest.strip_prefix(p) else { return false };
            rest = r;
        } else if i == parts.len() - 1 {
            return rest.ends_with(p);
        } else {
            let Some(at) = rest.find(p) else { return false };
            rest = &rest[at + p.len()..];
        }
    }
    rest.is_empty()
}

impl crate::runtime::Core {
    /// Read how to reopen each kept agent terminal's session: once per turn and
    /// agent process, never while a shell holds the foreground. A record whose
    /// agent left the foreground for 10 s (it was quit) is dropped.
    pub(crate) async fn capture_resumes(self: std::sync::Arc<Self>) {
        let now = crate::runtime::now_ms();
        let due: Vec<_> = {
            let mut st = self.lock();
            let mut dropped = false;
            let due = st
                .reg
                .terminals
                .values_mut()
                .filter(|t| t.keep && t.is_live())
                .filter_map(|t| {
                    if t.resume_lost_ms.is_some_and(|at| now.saturating_sub(at) >= 10_000) {
                        (t.resume, t.resume_lost_ms, dropped) = (None, None, true);
                    }
                    let fg = t.foreground.as_ref().filter(|f| !SHELLS.contains(&base(&f.name)))?;
                    let app = t.status.as_ref().map(|s| s.app.clone()).filter(|a| !a.is_empty())?;
                    let fresh = t.resume.as_ref().is_some_and(|r| r.turn == t.turn && r.pid == fg.pid);
                    // A failed read is retried every 5 s while the same process holds the turn
                    // (its session file may not exist yet).
                    let tried = t.resume_tried.is_some_and(|(turn, pid, at)| {
                        turn == t.turn && pid == fg.pid && now.saturating_sub(at) < 5_000
                    });
                    if fresh || tried {
                        return None;
                    }
                    t.resume_tried = Some((t.turn, fg.pid, now));
                    Some((t.id(), t.turn, app, t.argv.clone(), fg.clone(), t.labels.clone()))
                })
                .collect::<Vec<_>>();
            if dropped {
                self.save_soon();
            }
            due
        };
        if due.is_empty() || !crate::config::load(&self.paths.config).is_ok_and(|c| c.resume_agents == Some(true)) {
            return;
        }
        for (id, turn, app, argv, fg, labels) in due {
            let read = tokio::task::spawn_blocking(move || {
                let facts = Facts {
                    app: &app,
                    argv: &argv,
                    fg_argv: &fg.argv,
                    cwd: &fg.cwd,
                    pid: fg.pid,
                    labels: &labels,
                    agent_dir: crate::platform::env_var(fg.pid, "PI_CODING_AGENT_DIR"),
                };
                (plan(&facts, &Disk), fg)
            });
            let Ok((Some(argv), fg)) = read.await else { continue };
            let mut st = self.lock();
            // Only if the agent it was read from still holds the same turn.
            let same = |t: &&mut crate::terminals::Terminal| {
                t.is_live() && t.turn == turn && t.foreground.as_ref().is_some_and(|f| f.pid == fg.pid)
            };
            let Some(t) = st.reg.terminals.get_mut(&id).filter(same) else { continue };
            t.resume = Some(Resume { argv, cwd: fg.cwd, pid: fg.pid, name: fg.name, turn });
            t.resume_lost_ms = None;
            t.resume_tried = None;
            self.save_soon();
        }
    }
}

/// A foreground change: the agent it was read from is back (or the same
/// program restarted), or someone else holds the terminal now.
pub fn foreground_changed(t: &mut crate::terminals::Terminal, now: Option<&crate::foreground::Foreground>, at: u64) {
    let (Some(r), Some(f)) = (&t.resume, now) else { return };
    if f.pid == r.pid || f.name == r.name {
        t.resume_lost_ms = None;
    } else if t.resume_lost_ms.is_none() {
        t.resume_lost_ms = Some(at);
    }
}

#[cfg(test)]
mod tests;

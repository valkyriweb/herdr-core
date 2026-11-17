use super::*;

#[derive(Default)]
struct Fake {
    files: BTreeMap<String, String>,
    open: Vec<String>,
}

impl Lookup for Fake {
    fn home(&self) -> PathBuf {
        "/h".into()
    }
    fn newest(&self, dir: &Path) -> Option<String> {
        let dir = format!("{}/", dir.display());
        self.files.keys().filter(|k| k.starts_with(&dir)).max().cloned()
    }
    /// Files sort by name in write order.
    fn modified(&self, path: &Path) -> Option<std::time::SystemTime> {
        let rank = self.files.keys().position(|k| *k == path.display().to_string())?;
        Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(rank as u64))
    }
    fn read(&self, path: &Path) -> Option<String> {
        self.files.get(&path.display().to_string()).cloned()
    }
    fn open_files(&self, _pid: i32) -> Vec<String> {
        self.open.clone()
    }
    fn glob(&self, pattern: &str) -> Vec<String> {
        self.files.keys().filter(|k| wild_path(pattern, k)).cloned().collect()
    }
}

fn wild_path(pattern: &str, path: &str) -> bool {
    let (p, q): (Vec<_>, Vec<_>) = (pattern.split('/').collect(), path.split('/').collect());
    p.len() == q.len() && p.iter().zip(&q).all(|(a, b)| wild(a, b))
}

/// Words, or a shell -c launch when written `sh -c | script`.
fn v(s: &str) -> Vec<String> {
    match s.split_once(" | ") {
        Some((head, script)) => v(head).into_iter().chain([script.to_string()]).collect(),
        None => s.split_whitespace().map(String::from).collect(),
    }
}

fn run(app: &str, argv: &str, fg: &str, look: &Fake) -> Option<String> {
    run_in(app, argv, fg, look, false)
}

fn run_in(app: &str, argv: &str, fg: &str, look: &Fake, shared_cwd: bool) -> Option<String> {
    let labels = BTreeMap::new();
    let (argv, fg) = (v(argv), v(fg));
    let f = Facts { app, argv: &argv, fg_argv: &fg, cwd: "/w/p", pid: 7, labels: &labels, agent_dir: None, shared_cwd };
    plan(&f, look).map(|a| a.join(" "))
}

#[test]
fn pi_follows_a_session_switch_after_resuming() {
    let mut look = Fake::default();
    look.files.insert("/h/.pi/agent/sessions/--w-p--/1_old.jsonl".into(), String::new());
    assert_eq!(run("pi", "pi --session old", "pi", &look).as_deref(), Some("pi --session old"));
    look.files.insert("/h/.pi/agent/sessions/--w-p--/2_new.jsonl".into(), String::new());
    let (old, new) = ("/h/.pi/agent/sessions/--w-p--/1_old.jsonl", "/h/.pi/agent/sessions/--w-p--/2_new.jsonl");
    assert_eq!(run("pi", "pi --session old", "pi", &look), Some(format!("pi --session {new}")));
    assert_eq!(run("pi", &format!("pi --session {old}"), "pi", &look), Some(format!("pi --session {new}")));
}

#[test]
fn pi_never_guesses_when_another_agent_shares_the_cwd() {
    let mut look = Fake::default();
    look.files.insert("/h/.pi/agent/sessions/--w-p--/b.jsonl".into(), String::new());
    assert_eq!(run_in("pi", "pi", "pi", &look, true), None);
    assert_eq!(run_in("pi", "pi --session-id s", "pi", &look, true).as_deref(), Some("pi --session s"));
}

const SID: &str = "019a2b3c-4d5e-6f70-8192-a3b4c5d6e7f8";

#[test]
fn pi_reopens_its_newest_session_in_the_cwd_and_keeps_its_model() {
    let mut look = Fake::default();
    look.files.insert("/h/.pi/agent/sessions/--w-p--/2026_a.jsonl".into(), String::new());
    look.files.insert("/h/.pi/agent/sessions/--w-p--/2026_b.jsonl".into(), String::new());
    let got = run("pi", "pi --model opus --thinking high fix the bug", "pi", &look);
    let b = "/h/.pi/agent/sessions/--w-p--/2026_b.jsonl";
    assert_eq!(got, Some(format!("pi --session {b} --model opus --thinking high")));
    assert_eq!(run("pi", "pi --session-id abc", "pi", &look).as_deref(), Some("pi --session abc"));
}

#[test]
fn pii_is_told_apart_from_pi() {
    let mut look = Fake::default();
    look.files.insert("/h/.pi/pii/sessions/--w-p--/x.jsonl".into(), String::new());
    assert_eq!(
        run("pi", "zsh -c | exec pii", "node", &look).as_deref(),
        Some("pii --session /h/.pi/pii/sessions/--w-p--/x.jsonl")
    );
}

#[test]
fn claude_reads_its_session_and_model() {
    let mut look = Fake::default();
    look.files.insert("/h/.claude/sessions/7.json".into(), format!(r#"{{"sessionId":"{SID}"}}"#));
    look.files.insert(
        format!("/h/.claude/projects/-w-p/{SID}.jsonl"),
        "{\"message\":{\"model\":\"claude-a\"}}\n{\"message\":{\"model\":\"claude-b\"}}\n".into(),
    );
    let got = run("claude-code", "claude --dangerously-skip-permissions hello", "claude", &look);
    assert_eq!(got, Some(format!("claude --resume {SID} --dangerously-skip-permissions --model claude-b")));
}

#[test]
fn codex_names_the_recorded_provider_and_model() {
    let mut look = Fake::default();
    let rollout = format!("/h/.codex/sessions/2026/11/04/rollout-2026-11-04T10-00-00-{SID}.jsonl");
    look.files.insert(
        rollout.clone(),
        "{\"payload\":{\"model_provider\":\"openai\"}}\n{\"type\":\"turn_context\",\"payload\":{\"model\":\"gpt-x\"}}\n".into(),
    );
    look.open = vec![rollout];
    let got = run("codex", "codex --profile clawrouter do it", "codex", &look);
    assert_eq!(got, Some(format!("codex resume {SID} --no-daemon -c model_provider=\"openai\" -m gpt-x")));
}

#[test]
fn nothing_without_an_agent_or_a_session() {
    let look = Fake::default();
    assert_eq!(run("", "zsh", "vim notes", &look), None);
    assert_eq!(run("", "zsh -c | echo pi", "zsh", &look), None);
    assert_eq!(run("claude-code", "claude", "claude", &look), None);
    assert_eq!(run("codex", "codex", "codex", &look), None);
}

#[test]
fn a_prompt_word_is_never_a_flag() {
    let look = Fake::default();
    let got = run("pi", "pi --session-id s1 --model m say --thinking now", "pi", &look);
    assert_eq!(got.as_deref(), Some("pi --session s1 --model m"));
}

#[test]
fn the_shell_runs_the_agent_then_stays() {
    let got = launch(&v("/bin/zsh -l"), &["pi".into(), "--session".into(), "it's".into()]).unwrap();
    assert_eq!(got[..4], v("/bin/zsh -l -i -c"));
    assert_eq!(got[4], r"'pi' '--session' 'it'\''s'; exec '/bin/zsh' '-l'");
    assert_eq!(launch(&v("pwsh"), &v("pi")), None);
}

#[test]
fn shell_words_split_like_a_shell() {
    assert_eq!(
        words(r#"exec pi --model "a b" 'c'\ d; echo x"#),
        Some(v("exec pi --model").into_iter().chain(["a b".into(), "c d".into()]).collect())
    );
    assert_eq!(words("'open"), None);
}

#[test]
fn a_quit_agent_is_forgotten_after_another_program_holds_the_terminal() {
    let mut reg = crate::terminals::Registry::default();
    let id = reg.create(crate::terminals::NewTerminal { argv: v("zsh"), keep: true, ..Default::default() }, 0).unwrap();
    let t = reg.terminals.get_mut(&id).unwrap();
    t.resume = Some(Resume { argv: v("pi"), cwd: "/".into(), pid: 7, name: "pi".into(), turn: 1 });
    let fg = |pid, name: &str| crate::foreground::Foreground { pid, name: name.into(), argv: vec![], cwd: "/".into() };
    foreground_changed(t, Some(&fg(9, "zsh")), 100);
    assert_eq!(t.resume_lost_ms, Some(100));
    foreground_changed(t, Some(&fg(7, "pi")), 200);
    assert_eq!(t.resume_lost_ms, None);
}

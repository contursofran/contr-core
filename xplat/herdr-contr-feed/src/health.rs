//! `herdr-contr-feed health`: read-only checks of the core and agent setup, one JSON line per result.

use herdr_core::places::Places;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use herdr_core::hc::{HcClient, HcSession};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub id: &'static str,
    pub state: &'static str,
    pub message: String,
    pub fix: String,
}

impl Check {
    fn new(id: &'static str, state: &'static str, message: impl Into<String>, fix: impl Into<String>) -> Check {
        Check { id, state, message: message.into(), fix: fix.into() }
    }
    pub fn line(&self) -> Value {
        json!({"type": "check", "id": self.id, "state": self.state, "message": self.message, "fix": self.fix})
    }
}

pub fn machine_checks(i: &Inputs, env: &dyn Fn(&str) -> Option<String>, remote: bool) -> Vec<Check> {
    let places = Places::at(&i.root);
    let mut rows = vec![
        Check::new("helper", "ok", format!("Build {} · protocol {}", i.build, crate::protocol::PROTOCOL_VERSION), ""),
        if i.root.is_dir() {
            Check::new("root", "ok", i.root.display().to_string(), "")
        } else {
            Check::new("root", if remote { "warn" } else { "fail" }, "The core folder is missing.", REINSTALL)
        },
        hc(&HcClient::new(HcSession::resolve_env(), 4, 4)),
    ];
    let mut install = checks(i);
    if remote && !places.orch().is_file() {
        install[0] = Check::new("orch", "warn", "The remote helper can show chats. The orchestration tool is missing.", "Install the orchestration tool if this machine runs a coordinator.");
        if !places.kit().is_dir() {
            install[1].state = "warn";
        }
    }
    let orch_matches = install[0].state == "ok";
    rows.extend(install);
    rows.push(profiles(&places, env, orch_matches));
    rows.push(codex(env));
    rows.push(logs(i, env));
    rows.push(plan_usage(i, env));
    rows.push(if program(env, "git") {
        Check::new("git", "ok", "Found on PATH.", "")
    } else {
        Check::new("git", "warn", "Missing from PATH.", "Install Git and make it available on PATH.")
    });
    rows.push(github(env));
    rows.push(if crate::release::REPO.is_empty() {
        Check::new("release", "warn", "This helper has no release repository.", "Install a release build to check for updates.")
    } else {
        Check::new("release", "ok", format!("GitHub release source · {}", crate::release::REPO), "")
    });
    rows
}

fn hc(client: &HcClient) -> Check {
    match client.call("terminal.list", json!({}), Some(Duration::from_secs(4))) {
        Ok(data) => {
            let count = data["terminals"].as_array().map_or(0, Vec::len);
            Check::new("hc", "ok", format!("Core answers · {count} terminals"), "")
        }
        Err(_) => Check::new("hc", "fail", "The hc core did not answer.", "Check that hc is running and that HC_SOCKET or HC_SESSION selects the right core."),
    }
}

fn profiles(p: &Places, env: &dyn Fn(&str) -> Option<String>, orch_matches: bool) -> Check {
    let path = env("CONTR_AGENTS_CONFIG").map_or_else(|| p.agents(), PathBuf::from);
    if !path.is_file() {
        return Check::new("agents", "warn", "The profiles file is missing. Built-in defaults apply.", "Set up agents.json for this machine if it runs workers.");
    }
    let fix = "Check the flow and profiles in agents.json.";
    if !orch_matches {
        return Check::new("agents", "warn", "The profiles file exists. A matching orchestration tool is needed to check it.", REINSTALL);
    }
    match std::process::Command::new(p.orch()).args(["agents", "config", "--check"]).env("CONTR_AGENTS_CONFIG", path).output() {
        Ok(out) if out.status.success() => Check::new("agents", "ok", String::from_utf8_lossy(&out.stdout).trim(), ""),
        Ok(out) => Check::new("agents", "fail", String::from_utf8_lossy(&out.stderr).trim(), fix),
        Err(e) => Check::new("agents", "fail", format!("The profiles check could not run: {e}"), REINSTALL),
    }
}

fn program(env: &dyn Fn(&str) -> Option<String>, name: &str) -> bool {
    env("PATH").is_some_and(|path| std::env::split_paths(&path).any(|dir| executable(&dir.join(name))))
}

fn executable(path: &Path) -> bool {
    let Ok(meta) = path.metadata() else { return false };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    meta.is_file()
}

fn codex_home(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    env("CODEX_HOME").map(PathBuf::from).or_else(|| env("HOME").map(|h| Path::new(&h).join(".codex")))
}

fn codex(env: &dyn Fn(&str) -> Option<String>) -> Check {
    if !program(env, "codex") {
        return Check::new("codex", "warn", "Codex is missing from PATH.", "Install Codex if this machine runs Codex agents.");
    }
    let hooks = codex_home(env).and_then(|d| read_json(&d.join("hooks.json")));
    let names = [("SessionStart", "codex-session-start"), ("Stop", "codex-stop"), ("UserPromptSubmit", "codex-prompt")];
    if hooks.as_ref().is_some_and(|doc| names.iter().all(|(event, hook)| {
        doc["hooks"][event].as_array().into_iter().flatten().any(|group| {
            group["hooks"].as_array().into_iter().flatten().any(|h| h["command"].as_str().is_some_and(|c| c.ends_with(&format!("# contr-core-managed-{hook}"))))
        })
    })) {
        Check::new("codex", "ok", "Codex found. Coordinator hooks are configured.", "")
    } else {
        Check::new("codex", "warn", "Codex found. Coordinator hooks are missing or incomplete.", "For a Codex coordinator, run herdr-contr-orch codex hooks coordinator on. Then trust the entries in /hooks.")
    }
}

fn logs(i: &Inputs, env: &dyn Fn(&str) -> Option<String>) -> Check {
    let claude = i.claude.as_ref().is_some_and(|p| p.join("projects").is_dir());
    let codex = codex_home(env).is_some_and(|p| p.join("sessions").is_dir());
    let found = [(claude, "Claude Code"), (codex, "Codex")].into_iter().filter_map(|(yes, name)| yes.then_some(name)).collect::<Vec<_>>();
    if found.is_empty() {
        Check::new("logs", "warn", "No Claude Code or Codex session folders found.", "Open a session on this machine, or check CLAUDE_CONFIG_DIR and CODEX_HOME.")
    } else {
        Check::new("logs", "ok", format!("Session folders found for {}.", found.join(" and ")), "")
    }
}

fn plan_usage(i: &Inputs, env: &dyn Fn(&str) -> Option<String>) -> Check {
    let home = env("HOME").map(PathBuf::from);
    let sources = crate::usage::Sources {
        state_dir: crate::usage::state_dir(env, home.as_deref(), cfg!(target_os = "macos")),
        statusline_dir: Some(crate::statusline::in_root(&i.root)),
        codex_home: codex_home(env),
        codex: None,
        timeout: Duration::ZERO,
    };
    let plans = crate::usage::read(&sources);
    if plans.is_empty() {
        Check::new("usage", "warn", "No saved plan usage records found.", "Use Claude Code or Codex on this machine to record plan usage.")
    } else {
        Check::new("usage", "ok", format!("Saved records for {}.", plans.iter().map(|p| p.plan).collect::<Vec<_>>().join(" and ")), "")
    }
}

fn github(env: &dyn Fn(&str) -> Option<String>) -> Check {
    use std::process::{Command, Stdio};
    use std::time::Instant;

    if !program(env, "gh") {
        return Check::new("gh", "warn", "Missing from PATH.", "Install the GitHub CLI and make it available on PATH.");
    }
    let child = Command::new("gh").args(["auth", "status", "--hostname", "github.com"])
        .env("GH_PROMPT_DISABLED", "1").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    if let Ok(mut child) = child {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => return Check::new("gh", "ok", "GitHub sign-in verified.", ""),
                Ok(Some(_)) | Err(_) => break,
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
        let _ = child.kill();
        let _ = child.wait();
    }
    Check::new("gh", "warn", "Could not verify GitHub sign-in.", "Run gh auth status. Use gh auth login if GitHub CLI asks you to sign in.")
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

/// What the checks read. Tests give fakes.
pub struct Inputs<'a> {
    /// The backend root (`config::dir`), for the checks `orch` and `kit`.
    pub root: PathBuf,
    /// This helper's build stamp.
    pub build: String,
    /// The Claude Code folder (`~/.claude`, or CLAUDE_CONFIG_DIR).
    pub claude: Option<PathBuf>,
    /// Runs `<orch> --version`: its stdout, or why it did not run.
    pub orch_version: &'a dyn Fn(&Path) -> Result<String, String>,
}

const REINSTALL: &str = "Install again: unzip the latest release and run ./install.sh.";

/// The backend's checks.
pub fn checks(i: &Inputs) -> Vec<Check> {
    let p = Places::at(i.root.clone());
    vec![orch(i, &p), kit(i, &p)]
}

fn orch(i: &Inputs, p: &Places) -> Check {
    let path = p.orch();
    if !path.is_file() {
        return Check::new("orch", "fail", "herdr-contr-orch is not in herdr-contr/bin.", REINSTALL);
    }
    match (i.orch_version)(&path) {
        Err(why) => Check::new("orch", "fail", format!("herdr-contr-orch does not run: {why}"), REINSTALL),
        Ok(out) => {
            // "herdr-contr-orch <version> (<build>)"
            let build = out.trim().split_once('(').and_then(|(_, b)| b.strip_suffix(')')).unwrap_or("unknown");
            if build != i.build {
                Check::new("orch", "fail", format!("herdr-contr-orch is from build {build}, the helper is from build {}.", i.build), REINSTALL)
            } else {
                Check::new("orch", "ok", format!("The helper and herdr-contr-orch are from build {build}."), "")
            }
        }
    }
}

fn kit(i: &Inputs, p: &Places) -> Check {
    if !p.kit().join(".claude-plugin").join("plugin.json").is_file() || !p.marketplace().is_file() {
        return Check::new("kit", "fail", "The kit is not in herdr-contr/kit.", REINSTALL);
    }
    let add = format!("Run: claude plugin marketplace add {} and claude plugin install contr@contr.", p.folder().display());
    let Some(dir) = &i.claude else { return Check::new("kit", "warn", "The Claude Code folder is not known.", add) };
    let known = read_json(&dir.join("plugins/known_marketplaces.json"));
    let path = known.as_ref().and_then(|k| k.pointer("/contr/source/path")).and_then(|v| v.as_str());
    match path {
        None => Check::new("kit", "warn", "The Claude Code plugin contr is not added.", add),
        Some(x) if !same_folder(Path::new(x), &p.folder()) => Check::new("kit", "fail", format!("The Claude Code plugin contr reads {x}, not the installed kit."), add),
        Some(_) => Check::new("kit", "ok", "The Claude Code plugin contr reads the installed kit.", ""),
    }
}

fn same_folder(a: &Path, b: &Path) -> bool {
    a == b || fs::canonicalize(a).ok().zip(fs::canonicalize(b).ok()).is_some_and(|(a, b)| a == b)
}

/// The Claude Code folder: CLAUDE_CONFIG_DIR, else ~/.claude.
pub fn claude_dir(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    env("CLAUDE_CONFIG_DIR").map(PathBuf::from).or_else(|| env("HOME").map(|h| Path::new(&h).join(".claude")))
}

/// Runs `<orch> --version`.
pub fn run_orch_version(path: &Path) -> Result<String, String> {
    let o = std::process::Command::new(path).arg("--version").output().map_err(|e| e.to_string())?;
    if !o.status.success() {
        return Err(format!("exit {}", o.status.code().unwrap_or(-1)));
    }
    Ok(String::from_utf8_lossy(&o.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: &str = "abc1234 2026-11-16";

    fn put(home: &Path, rel: &str, body: &str) {
        let p = home.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    /// A good install in a temp home.
    fn good() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let h = d.path();
        put(h, "herdr-contr/install.json", &json!({"build": B}).to_string());
        put(h, "herdr-contr/bin/herdr-contr-orch", "");
        put(h, "herdr-contr/kit/.claude-plugin/plugin.json", "{}");
        put(h, "herdr-contr/.claude-plugin/marketplace.json", "{}");
        let src = h.join("herdr-contr").display().to_string();
        put(h, "claude/plugins/known_marketplaces.json", &json!({"contr": {"source": {"path": src}}}).to_string());
        d
    }

    fn ok_orch(_: &Path) -> Result<String, String> {
        Ok(format!("herdr-contr-orch 0.1.0 ({B})\n"))
    }

    fn inputs<'a>(h: &Path, orch: &'a dyn Fn(&Path) -> Result<String, String>) -> Inputs<'a> {
        Inputs {
            root: h.join("herdr-contr"),
            build: B.into(),
            claude: Some(h.join("claude")),
            orch_version: orch,
        }
    }

    fn state(c: &[Check], id: &str) -> (&'static str, String) {
        let x = c.iter().find(|x| x.id == id).unwrap();
        (x.state, x.message.clone())
    }

    #[test]
    fn backend_health_has_only_orch_and_kit() {
        let d = good();
        let c = checks(&inputs(d.path(), &ok_orch));
        assert_eq!(c.iter().map(|x| x.id).collect::<Vec<_>>(), ["orch", "kit"]);
        for x in &c {
            assert_eq!(x.state, "ok", "{x:?}");
        }
    }

    /// The checks `orch` and `kit` read the backend root, not `<home>/herdr-contr`.
    #[test]
    fn orch_and_kit_read_the_backend_root() {
        let d = good();
        let h = d.path();
        let r = h.join("root");
        fs::rename(h.join("herdr-contr"), &r).unwrap();
        put(h, "claude/plugins/known_marketplaces.json", &json!({"contr": {"source": {"path": r.display().to_string()}}}).to_string());
        let mut i = inputs(h, &ok_orch);
        i.root = r;
        let c = checks(&i);
        assert_eq!(state(&c, "orch").0, "ok");
        assert_eq!(state(&c, "kit").0, "ok");
    }

    #[test]
    fn orch_missing_broken_or_another_build() {
        let d = good();
        let bad = |_: &Path| -> Result<String, String> { Err("exit 1".into()) };
        assert!(state(&checks(&inputs(d.path(), &bad)), "orch").1.contains("does not run"));
        let old = |_: &Path| -> Result<String, String> { Ok("herdr-contr-orch 0.1.0 (old 2026-01-01)".into()) };
        assert_eq!(state(&checks(&inputs(d.path(), &old)), "orch").0, "fail");
        fs::remove_file(d.path().join("herdr-contr/bin/herdr-contr-orch")).unwrap();
        assert_eq!(state(&checks(&inputs(d.path(), &ok_orch)), "orch").0, "fail");
    }

    #[test]
    fn kit_plugin_elsewhere_absent_or_kit_missing() {
        let d = good();
        let h = d.path();
        put(h, "claude/plugins/known_marketplaces.json", r#"{"contr":{"source":{"path":"/elsewhere"}}}"#);
        assert!(state(&checks(&inputs(h, &ok_orch)), "kit").1.contains("/elsewhere"));
        put(h, "claude/plugins/known_marketplaces.json", "{}");
        assert_eq!(state(&checks(&inputs(h, &ok_orch)), "kit").0, "warn");
        fs::remove_dir_all(h.join("herdr-contr/kit")).unwrap();
        assert_eq!(state(&checks(&inputs(h, &ok_orch)), "kit").0, "fail");
    }

    #[cfg(unix)]
    #[test]
    fn marketplace_compatibility_link_points_to_the_same_kit() {
        let d = good();
        let home = d.path();
        let root = home.join("contr-core");
        fs::rename(home.join("herdr-contr"), &root).unwrap();
        std::os::unix::fs::symlink(&root, home.join("herdr-contr")).unwrap();
        let mut input = inputs(home, &ok_orch);
        input.root = root;
        assert_eq!(state(&checks(&input), "kit").0, "ok");
    }
}

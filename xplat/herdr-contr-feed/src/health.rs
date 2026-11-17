//! `herdr-contr-feed health`: checks the install in one Herdr home (#217). One `check` line for each check:
//! `{id, state: ok|warn|fail, message, fix}`. It reads files and runs `herdr-contr-orch --version`; it writes nothing.
//! The checks `orch` and `kit` read the backend root; they are the only checks.

use herdr_core::places::Places;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

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
    let known: Option<Value> = fs::read(dir.join("plugins").join("known_marketplaces.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let path = known.as_ref().and_then(|k| k.pointer("/contr/source/path")).and_then(|v| v.as_str());
    match path {
        None => Check::new("kit", "warn", "The Claude Code plugin contr is not added.", add),
        Some(x) if Path::new(x) != p.folder() => Check::new("kit", "fail", format!("The Claude Code plugin contr reads {x}, not the installed kit."), add),
        Some(_) => Check::new("kit", "ok", "The Claude Code plugin contr reads the installed kit.", ""),
    }
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
}

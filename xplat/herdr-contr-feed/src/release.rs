//! `update check` and `update install` (#219, #6): the newest release build of this repository, and its install.
//! A release is tagged `build-<date>-<sha7>` and has the assets `herdr-contr-<tag>.zip` and `release.json`
//! (.github/workflows/release.yml, scripts/release-notes.mjs). The releases come from a `Source`. The default,
//! `Releases`, reads the public `release.json` with curl (`Curl`); when there is none (HTTP 404) or it is not
//! schema 1, it asks `gh` (`Gh`), as before.

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The repository whose releases count: HERDR_CONTR_REPO at run time, else the one baked in at build time.
pub const REPO: &str = env!("HERDR_CONTR_REPO");
/// A check is kept this long in update.json; `update check --force` asks again.
pub const DAY_S: u64 = 24 * 3600;

pub type Fail = (&'static str, String);

#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub tag: String,
    pub url: String,
    /// The zip's "sha256:<hex>" when the source gives it.
    pub digest: Option<String>,
    /// The release version (`0.1.<run>`), from release.json; None from gh.
    pub version: Option<String>,
    /// release.json's `releases` (the newest releases and their changes); empty from gh.
    pub releases: Vec<Value>,
    /// True when it came from release.json: curl downloads its zip, else gh does.
    pub from_file: bool,
}

pub trait Source {
    fn newest(&self) -> Result<Release, Fail>;
    /// Puts the release's zip in `dir` and gives its path.
    fn download(&self, rel: &Release, dir: &Path) -> Result<PathBuf, Fail>;
}

fn var(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

fn no_repo() -> Fail {
    ("no-repo", "This helper does not know its GitHub repository (set HERDR_CONTR_REPO).".into())
}

/// `curl` (HERDR_CONTR_CURL, else `curl` on PATH) for the public release URLs of `repo`. `file_url` names another
/// release.json (HERDR_CONTR_RELEASE_URL, for tests).
pub struct Curl {
    pub program: String,
    pub repo: String,
    pub file_url: Option<String>,
}

impl Curl {
    pub fn from_process() -> Curl {
        Curl { program: var("HERDR_CONTR_CURL").unwrap_or_else(|| "curl".into()), repo: var("HERDR_CONTR_REPO").unwrap_or_else(|| REPO.into()), file_url: var("HERDR_CONTR_RELEASE_URL") }
    }

    /// GETs `url` (to the file `to`, else into the answer) and gives the HTTP status and the body. A curl that
    /// cannot start or cannot reach the server: `no-network`.
    fn get(&self, url: &str, to: Option<&Path>) -> Result<(u16, String), Fail> {
        let mut c = Command::new(&self.program);
        c.args(["-sSL", "--connect-timeout", "20", "-w", "\\n%{http_code}", "-o"]);
        match to {
            Some(p) => c.arg(p),
            None => c.arg("-"),
        };
        let o = c.arg(url).output().map_err(|e| ("no-network", format!("curl could not start ({e}).")))?;
        if !o.status.success() {
            return Err(("no-network", format!("curl could not read {url}: {}", String::from_utf8_lossy(&o.stderr).trim())));
        }
        let out = String::from_utf8_lossy(&o.stdout).into_owned();
        let (body, code) = out.rsplit_once('\n').unwrap_or(("", out.as_str()));
        let code = code.trim().parse().map_err(|_| ("http", format!("curl gave no HTTP status for {url}.")))?;
        Ok((code, body.to_string()))
    }

    /// The newest release from release.json. None when there is no file (HTTP 404) or it is not schema 1: then
    /// gh is asked. Another HTTP status: `http`.
    pub fn read(&self) -> Result<Option<Release>, Fail> {
        let url = match &self.file_url {
            Some(u) => u.clone(),
            None if self.repo.is_empty() => return Err(no_repo()),
            None => format!("https://github.com/{}/releases/latest/download/release.json", self.repo),
        };
        let (code, body) = self.get(&url, None)?;
        if code == 404 {
            return Ok(None);
        }
        if code != 200 {
            return Err(("http", format!("HTTP {code} from {url}.")));
        }
        let Ok(v) = serde_json::from_str::<Value>(&body) else { return Ok(None) };
        if v["schema"] != 1 {
            return Ok(None);
        }
        let tag = v["tag"].as_str().unwrap_or_default().to_string();
        if stamp_of(&tag).is_none() {
            return Err(("no-build", format!("The newest release ({tag}) is not a build- release.")));
        }
        let name = zip_name(&tag);
        let listed = v["assets"].as_array().is_some_and(|a| a.iter().any(|x| x == name.as_str()));
        let Some(sum) = v["sha256"][&name].as_str().filter(|_| listed) else {
            return Err(("no-build", format!("The release file of {tag} names no {name} with its sha256.")));
        };
        Ok(Some(Release {
            url: format!("https://github.com/{}/releases/tag/{tag}", self.repo),
            digest: Some(format!("sha256:{sum}")),
            version: v["version"].as_str().map(String::from),
            releases: v["releases"].as_array().cloned().unwrap_or_default(),
            from_file: true,
            tag,
        }))
    }

    pub fn fetch(&self, rel: &Release, dir: &Path) -> Result<PathBuf, Fail> {
        if self.repo.is_empty() {
            return Err(no_repo());
        }
        let name = zip_name(&rel.tag);
        let to = dir.join(&name);
        let url = format!("https://github.com/{}/releases/download/{}/{name}", self.repo, rel.tag);
        let (code, _) = self.get(&url, Some(&to))?;
        if code != 200 {
            return Err(("http", format!("HTTP {code} from {url}.")));
        }
        Ok(to)
    }
}

/// The default source: release.json with curl, else gh (a 404 or a file that is not schema 1). Another failure
/// of curl stops it, with no second read.
pub struct Releases {
    pub curl: Curl,
    pub gh: Gh,
}

impl Releases {
    pub fn from_process() -> Releases {
        Releases { curl: Curl::from_process(), gh: Gh::from_process() }
    }
}

impl Source for Releases {
    fn newest(&self) -> Result<Release, Fail> {
        match self.curl.read()? {
            Some(r) => Ok(r),
            None => self.gh.newest(),
        }
    }
    fn download(&self, rel: &Release, dir: &Path) -> Result<PathBuf, Fail> {
        if rel.from_file { self.curl.fetch(rel, dir) } else { self.gh.download(rel, dir) }
    }
}

pub fn zip_name(tag: &str) -> String {
    format!("herdr-contr-{tag}.zip")
}

/// `gh` (HERDR_CONTR_GH, else `gh` on PATH) for the repository `repo`.
pub struct Gh {
    pub program: String,
    pub repo: String,
}

impl Gh {
    pub fn from_process() -> Gh {
        Gh { program: var("HERDR_CONTR_GH").unwrap_or_else(|| "gh".into()), repo: var("HERDR_CONTR_REPO").unwrap_or_else(|| REPO.into()) }
    }
    fn run(&self, args: &[&str]) -> Result<String, Fail> {
        if self.repo.is_empty() {
            return Err(no_repo());
        }
        let o = Command::new(&self.program).args(args).args(["--repo", &self.repo]).output().map_err(|e| ("no-gh", format!("gh could not start ({e}). Install the GitHub CLI and log in.")))?;
        if !o.status.success() {
            return Err(("gh", format!("gh failed: {}", String::from_utf8_lossy(&o.stderr).trim())));
        }
        Ok(String::from_utf8_lossy(&o.stdout).into_owned())
    }
}

impl Source for Gh {
    fn newest(&self) -> Result<Release, Fail> {
        let text = self.run(&["release", "view", "--json", "tagName,url,assets"])?;
        let v: Value = serde_json::from_str(&text).map_err(|e| ("gh", format!("gh gave no release JSON: {e}")))?;
        let tag = v["tagName"].as_str().unwrap_or_default().to_string();
        if stamp_of(&tag).is_none() {
            return Err(("no-build", format!("The newest release ({tag}) is not a build- release.")));
        }
        let name = zip_name(&tag);
        let asset = v["assets"].as_array().and_then(|a| a.iter().find(|x| x["name"] == name.as_str()));
        let Some(asset) = asset else { return Err(("no-build", format!("The release {tag} has no {name}."))) };
        Ok(Release { url: v["url"].as_str().unwrap_or_default().into(), digest: asset["digest"].as_str().map(String::from), tag, version: None, releases: Vec::new(), from_file: false })
    }
    fn download(&self, rel: &Release, dir: &Path) -> Result<PathBuf, Fail> {
        let name = zip_name(&rel.tag);
        self.run(&["release", "download", &rel.tag, "--pattern", &name, "--dir", &dir.to_string_lossy()])?;
        Ok(dir.join(name))
    }
}

/// "<sha7> <date>", the build stamp, from a tag `build-YYYY-MM-DD-<sha7>`.
pub fn stamp_of(tag: &str) -> Option<String> {
    let rest = tag.strip_prefix("build-")?;
    let (date, sha) = rest.rsplit_once('-')?;
    let ok = date.len() == 10 && sha.len() == 7 && sha.chars().all(|c| c.is_ascii_hexdigit());
    ok.then(|| format!("{sha} {date}"))
}

/// The installed build: install.json's "build" in the herdr-contr folder, else this helper's own.
pub fn installed(folder: &Path) -> String {
    std::fs::read(folder.join("install.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v["build"].as_str().map(String::from))
        .unwrap_or_else(|| crate::protocol::BUILD.into())
}

/// The installed release version: install.json's "release" when it has a "build" (null for an install made
/// before the version), else this helper's own.
pub fn installed_version(folder: &Path) -> Option<String> {
    let record = std::fs::read(folder.join("install.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok());
    match record.filter(|v| v["build"].is_string()) {
        Some(v) => v["release"].as_str().map(String::from),
        None => crate::update::release(),
    }
}

fn same(a: &str, b: &str) -> bool {
    a.split(' ').next() == b.split(' ').next()
}

/// True when `newest` is a higher version than `installed`. When one of them is not a version: `differs`.
pub fn newer(installed: Option<&str>, newest: Option<&str>, differs: bool) -> bool {
    match (installed.and_then(crate::update::version), newest.and_then(crate::update::version)) {
        (Some(i), Some(n)) => n > i,
        _ => differs,
    }
}

/// The `update` line: the installed build and version, the newest, whether they differ and whether the newest is
/// newer, and the releases of release.json. The newest comes from update.json when it is less than a day old
/// and not `force`.
pub fn check(src: &dyn Source, folder: &Path, now_s: u64, force: bool) -> Result<Value, Fail> {
    let cache = folder.join("update.json");
    let kept: Option<Value> = std::fs::read(&cache).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let fresh = kept.filter(|k| !force && now_s.saturating_sub(k["checked_at"].as_u64().unwrap_or(0)) < DAY_S);
    let k = match fresh {
        Some(k) => k,
        None => {
            let r = src.newest()?;
            let k = json!({"checked_at": now_s, "tag": r.tag, "url": r.url, "version": r.version, "releases": r.releases});
            let _ = std::fs::write(&cache, k.to_string());
            k
        }
    };
    let tag = k["tag"].as_str().unwrap_or_default();
    let installed = installed(folder);
    let newest = stamp_of(tag).unwrap_or_default();
    let differs = !newest.is_empty() && !same(&installed, &newest);
    let installed_version = installed_version(folder);
    let newest_version = k["version"].as_str();
    Ok(json!({
        "type": "update", "installed": installed, "newest": newest, "tag": tag, "url": k["url"].as_str().unwrap_or_default(),
        "differs": differs, "checked_at": k["checked_at"].as_u64().unwrap_or(0),
        "installed_version": installed_version, "newest_version": newest_version,
        "newer": newer(installed_version.as_deref(), newest_version, differs),
        "root": folder.display().to_string(), "releases": k["releases"].as_array().cloned().unwrap_or_default(),
    }))
}

fn sha_file(p: &Path) -> Result<String, Fail> {
    let b = std::fs::read(p).map_err(|e| ("check", format!("cannot read {}: {e}", p.display())))?;
    Ok(Sha256::digest(&b).iter().map(|b| format!("{b:02x}")).collect())
}

/// The release target of this machine, as in the zip's remote/ folder.
pub fn own_target() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", _) => "universal-apple-darwin",
        ("linux", "aarch64") => "aarch64-unknown-linux-musl",
        _ => "x86_64-unknown-linux-musl",
    }
}

/// Checks the unzipped folder: every line of remote/SHA256SUMS, and that the helper for this machine is the one
/// the sums name. Gives that helper, and the orchestration tool when the zip has one (macOS).
pub fn checked(top: &Path) -> Result<(PathBuf, Option<PathBuf>), Fail> {
    let sums = std::fs::read_to_string(top.join("remote/SHA256SUMS")).map_err(|e| ("check", format!("the zip has no remote/SHA256SUMS ({e})")))?;
    let mut mine = None;
    for line in sums.lines().filter(|l| !l.trim().is_empty()) {
        let (want, rel) = line.split_once(char::is_whitespace).map(|(a, b)| (a, b.trim().trim_start_matches('*'))).ok_or(("check", format!("bad line in SHA256SUMS: {line}")))?;
        if rel.contains("..") || sha_file(&top.join("remote").join(rel))? != want {
            return Err(("check", format!("remote/{rel} does not match SHA256SUMS. Nothing was installed.")));
        }
        if rel.starts_with(&format!("{}/", own_target())) {
            mine = Some(want.to_string());
        }
    }
    let Some(want) = mine else { return Err(("check", format!("SHA256SUMS names no helper for {}.", own_target()))) };
    let mac = own_target() == "universal-apple-darwin";
    let helper = if mac { top.join("bin/herdr-contr-feed") } else { top.join("remote").join(own_target()).join("herdr-contr-feed") };
    if sha_file(&helper)? != want {
        return Err(("check", "The helper in the zip does not match SHA256SUMS. Nothing was installed.".into()));
    }
    let orch = top.join("bin/herdr-contr-orch");
    Ok((helper, (mac && orch.is_file()).then_some(orch)))
}

/// Where `update install` installs: a Herdr home (`install --home <home>`), or a root of the core
/// (`CONTR_HOME=<root> install`).
#[derive(Debug, Clone, PartialEq)]
pub enum Dest {
    Home(PathBuf),
    Root(PathBuf),
}

/// Downloads the newest release to a new folder in `tmp`, checks it, runs its install with --dry-run, then
/// without. Gives the `update-plan` and `update-done` lines. Stops at the first error; the folder goes after.
pub fn install(src: &dyn Source, home: &Dest, tmp: &Path) -> Result<Vec<Value>, Fail> {
    let rel = src.newest()?;
    let dir = tmp.join(format!("herdr-contr-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| ("write", format!("cannot make {}: {e}", dir.display())))?;
    let out = install_in(src, &rel, home, &dir);
    let _ = std::fs::remove_dir_all(&dir);
    out
}

fn install_in(src: &dyn Source, rel: &Release, home: &Dest, dir: &Path) -> Result<Vec<Value>, Fail> {
    let zip = src.download(rel, dir)?;
    if let Some(d) = rel.digest.as_deref().and_then(|d| d.strip_prefix("sha256:")) {
        if sha_file(&zip)? != d {
            return Err(("check", "The downloaded zip does not match the release's digest. Nothing was installed.".into()));
        }
    }
    let x = dir.join("x");
    let o = Command::new("unzip").arg("-q").arg(&zip).arg("-d").arg(&x).output().map_err(|e| ("unzip", format!("unzip could not start ({e})")))?;
    if !o.status.success() {
        return Err(("unzip", format!("cannot unzip the release: {}", String::from_utf8_lossy(&o.stderr).trim())));
    }
    let tops: Vec<PathBuf> = std::fs::read_dir(&x).map_err(|e| ("unzip", e.to_string()))?.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect();
    let [top] = tops.as_slice() else { return Err(("check", "The zip does not hold one folder.".into())) };
    let (helper, orch) = checked(top)?;
    let run = |dry: bool| -> Result<String, Fail> {
        let mut c = Command::new(&helper);
        c.arg("install");
        c.arg("--backend");
        match home {
            Dest::Home(h) => c.arg("--home").arg(h),
            Dest::Root(r) => c.env("CONTR_HOME", r),
        };
        c.arg("--helper").arg(&helper).arg("--remote").arg(top.join("remote"));
        if let Some(o) = &orch {
            c.arg("--orch").arg(o);
        }
        if dry {
            c.arg("--dry-run");
        }
        let o = c.output().map_err(|e| ("install", format!("the new helper could not start ({e})")))?;
        let text = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)).trim().to_string();
        if o.status.success() { Ok(text) } else { Err(("install", text)) }
    };
    let plan = run(true)?;
    let done = run(false)?;
    let newest = stamp_of(&rel.tag).unwrap_or_default();
    Ok(vec![json!({"type": "update-plan", "text": plan}), json!({"type": "update-done", "installed": newest, "tag": rel.tag, "version": rel.version, "text": done})])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::os::unix::fs::PermissionsExt;

    const TAG: &str = "build-2026-11-16-abc1234";

    /// A fake source: the newest release, and a zip it copies on download. Counts the asks.
    struct Fake {
        rel: Release,
        zip: Option<PathBuf>,
        asks: Cell<u32>,
    }
    impl Source for Fake {
        fn newest(&self) -> Result<Release, Fail> {
            self.asks.set(self.asks.get() + 1);
            Ok(self.rel.clone())
        }
        fn download(&self, rel: &Release, dir: &Path) -> Result<PathBuf, Fail> {
            let to = dir.join(zip_name(&rel.tag));
            std::fs::copy(self.zip.as_ref().unwrap(), &to).unwrap();
            Ok(to)
        }
    }
    fn fake(zip: Option<PathBuf>, digest: Option<String>) -> Fake {
        Fake { rel: Release { tag: TAG.into(), url: "https://example.com/r".into(), digest, version: None, releases: Vec::new(), from_file: false }, zip, asks: Cell::new(0) }
    }
    fn script(p: &Path, body: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    fn hex(b: &[u8]) -> String {
        Sha256::digest(b).iter().map(|b| format!("{b:02x}")).collect()
    }

    /// A release zip as the workflow makes it; its helper logs its arguments to `log`. `bad`: a wrong sum.
    fn release_zip(t: &Path, log: &Path, bad: bool) -> PathBuf {
        let top = t.join("src").join(format!("herdr-contr-{TAG}"));
        let body = format!("echo \"$@\" >> '{0}'; echo \"${{CONTR_HOME:-}}\" >> '{0}.env'; echo planned", log.display());
        script(&top.join("bin/herdr-contr-feed"), &body);
        script(&top.join("bin/herdr-contr-orch"), "true");
        let helper = std::fs::read(top.join("bin/herdr-contr-feed")).unwrap();
        let mut sums = String::new();
        for target in ["universal-apple-darwin", "x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl"] {
            script(&top.join("remote").join(target).join("herdr-contr-feed"), &body);
            let sum = if bad { "0".repeat(64) } else { hex(&helper) };
            sums += &format!("{sum}  {target}/herdr-contr-feed\n");
        }
        std::fs::write(top.join("remote/SHA256SUMS"), sums).unwrap();
        let zip = t.join("r.zip");
        let ok = Command::new("zip").arg("-qry").arg(&zip).arg(top.file_name().unwrap()).current_dir(t.join("src")).status().unwrap();
        assert!(ok.success());
        zip
    }

    #[test]
    fn stamps_from_tags() {
        assert_eq!(stamp_of(TAG).as_deref(), Some("abc1234 2026-11-16"));
        assert_eq!(stamp_of("build-2026-11-16-abc123"), None);
        assert_eq!(stamp_of("v0.8.0"), None);
        assert_eq!(stamp_of("build-2026-11-16-zzzzzzz"), None);
    }

    #[test]
    fn check_compares_and_keeps_a_day() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("install.json"), r#"{"build":"1111111 2026-11-10","files":{}}"#).unwrap();
        let f = fake(None, None);
        let l = check(&f, t.path(), 1000, false).unwrap();
        assert_eq!((l["installed"].as_str(), l["newest"].as_str(), l["differs"].as_bool()), (Some("1111111 2026-11-10"), Some("abc1234 2026-11-16"), Some(true)));
        check(&f, t.path(), 1000 + DAY_S - 1, false).unwrap();
        assert_eq!(f.asks.get(), 1, "a check less than a day old is kept");
        check(&f, t.path(), 1000 + DAY_S, false).unwrap();
        assert_eq!(f.asks.get(), 2, "a day later it asks again");
        check(&f, t.path(), 1000 + DAY_S, true).unwrap();
        assert_eq!(f.asks.get(), 3, "--force asks again");
        std::fs::write(t.path().join("install.json"), r#"{"build":"abc1234 2026-11-16"}"#).unwrap();
        assert_eq!(check(&f, t.path(), 1000 + DAY_S, false).unwrap()["differs"], false);
    }

    #[test]
    fn gh_reads_the_newest_build() {
        let t = tempfile::tempdir().unwrap();
        let gh = |out: &str, code: i32| {
            let p = t.path().join("gh");
            script(&p, &format!("echo \"$@\" > '{}/args'; echo '{out}'; echo oops >&2; exit {code}", t.path().display()));
            Gh { program: p.display().to_string(), repo: "o/r".into() }
        };
        let json = format!(r#"{{"tagName":"{TAG}","url":"https://x/r","assets":[{{"name":"herdr-contr-{TAG}.zip","digest":"sha256:ab"}}]}}"#);
        let r = gh(&json, 0).newest().unwrap();
        assert_eq!((r.tag.as_str(), r.digest.as_deref()), (TAG, Some("sha256:ab")));
        assert_eq!(std::fs::read_to_string(t.path().join("args")).unwrap().trim(), "release view --json tagName,url,assets --repo o/r");
        assert_eq!(gh(r#"{"tagName":"v1.0.0","assets":[]}"#, 0).newest().unwrap_err().0, "no-build");
        assert_eq!(gh(&format!(r#"{{"tagName":"{TAG}","assets":[]}}"#), 0).newest().unwrap_err().0, "no-build");
        assert_eq!(gh("", 1).newest().unwrap_err(), ("gh", "gh failed: oops".to_string()));
        assert_eq!(Gh { program: "gh".into(), repo: String::new() }.newest().unwrap_err().0, "no-repo");
        assert_eq!(Gh { program: t.path().join("none").display().to_string(), repo: "o/r".into() }.newest().unwrap_err().0, "no-gh");
    }

    #[test]
    fn install_runs_a_dry_run_then_the_install() {
        let t = tempfile::tempdir().unwrap();
        let log = t.path().join("log");
        let f = fake(Some(release_zip(t.path(), &log, false)), None);
        let home = t.path().join("home");
        let lines = install(&f, &Dest::Home(home.clone()), t.path()).unwrap();
        assert_eq!((lines[0]["type"].as_str(), lines[0]["text"].as_str()), (Some("update-plan"), Some("planned")));
        assert_eq!((lines[1]["type"].as_str(), lines[1]["installed"].as_str()), (Some("update-done"), Some("abc1234 2026-11-16")));
        let runs: Vec<String> = std::fs::read_to_string(&log).unwrap().lines().map(String::from).collect();
        assert_eq!(runs.len(), 2);
        let first = "install --backend --home";
        assert!(runs[0].starts_with(&format!("{first} {} --helper ", home.display())) && runs[0].ends_with("--dry-run"));
        assert!(!runs[1].contains("--dry-run") && runs[1].contains("--remote"));
        assert!(!t.path().join(format!("herdr-contr-update-{}", std::process::id())).exists(), "the temp folder goes");
        assert_eq!(std::fs::read_to_string(t.path().join("log.env")).unwrap(), "\n\n", "--home: no CONTR_HOME");
    }

    #[test]
    fn a_bad_sum_or_digest_installs_nothing() {
        let t = tempfile::tempdir().unwrap();
        let log = t.path().join("log");
        let bad = fake(Some(release_zip(t.path(), &log, true)), None);
        let e = install(&bad, &Dest::Home(t.path().join("home")), t.path()).unwrap_err();
        assert_eq!(e.0, "check");
        assert!(!log.exists(), "no install ran");
        let good = release_zip(&t.path().join("g"), &log, false);
        let wrong = fake(Some(good), Some(format!("sha256:{}", "0".repeat(64))));
        assert!(install(&wrong, &Dest::Home(t.path().join("home")), t.path()).unwrap_err().1.contains("digest"));
        assert!(!log.exists());
    }

    /// A fake curl: it serves `<d>/json` for release.json and `<d>/zip` for the zip, with the HTTP status in
    /// `<d>/json.code` and `<d>/zip.code` (404 when there is none). `<d>/down`: it cannot reach the server. It
    /// logs each URL to `<d>/urls`.
    fn fake_curl(d: &Path) -> Curl {
        std::fs::create_dir_all(d).unwrap();
        let body = format!(
            r#"d='{}'
out=-; while [ $# -gt 1 ]; do [ "$1" = -o ] && {{ out="$2"; shift; }}; shift; done
echo "$1" >> "$d/urls"
[ -f "$d/down" ] && {{ echo 'Could not resolve host: github.com' >&2; exit 6; }}
case "$1" in *.zip) k=zip;; *) k=json;; esac
if [ "$out" = - ]; then cat "$d/$k" 2>/dev/null; else cp "$d/$k" "$out" 2>/dev/null; fi
printf '\n%s' "$(cat "$d/$k.code" 2>/dev/null || echo 404)""#,
            d.display()
        );
        script(&d.join("curl"), &body);
        Curl { program: d.join("curl").display().to_string(), repo: "o/r".into(), file_url: None }
    }
    fn serve(d: &Path, k: &str, code: u16, body: &[u8]) {
        std::fs::create_dir_all(d).unwrap();
        std::fs::write(d.join(k), body).unwrap();
        std::fs::write(d.join(format!("{k}.code")), code.to_string()).unwrap();
    }
    fn urls(d: &Path) -> Vec<String> {
        std::fs::read_to_string(d.join("urls")).unwrap_or_default().lines().map(String::from).collect()
    }
    /// release.json as scripts/release-notes.mjs writes it, with `sum` for the zip.
    fn release_file(sum: &str) -> String {
        let zip = zip_name(TAG);
        json!({"schema": 1, "version": "0.1.7", "tag": TAG, "sha": "abc1234ffff", "date": "2026-11-16T10:00:00.000Z", "assets": [zip, "release.json"],
            "sha256": {zip.as_str(): sum}, "releases": [{"version": "0.1.7", "tag": TAG, "date": "2026-11-16", "changes": ["Update: no gh"]}]})
        .to_string()
    }
    /// A fake gh that logs its arguments to `<d>/gh-args` and gives `out`.
    fn fake_gh(d: &Path, out: &str) -> Gh {
        let p = d.join("gh");
        script(&p, &format!("echo \"$@\" >> '{}/gh-args'; echo '{out}'", d.display()));
        Gh { program: p.display().to_string(), repo: "o/r".into() }
    }

    #[test]
    fn curl_reads_the_release_file() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("c");
        let mut curl = fake_curl(&d);
        serve(&d, "json", 200, release_file("ab12").as_bytes());
        let r = curl.read().unwrap().unwrap();
        assert_eq!((r.tag.as_str(), r.digest.as_deref(), r.version.as_deref(), r.from_file), (TAG, Some("sha256:ab12"), Some("0.1.7"), true));
        assert_eq!(r.url, format!("https://github.com/o/r/releases/tag/{TAG}"));
        assert_eq!(r.releases[0]["changes"], json!(["Update: no gh"]));
        assert_eq!(urls(&d), ["https://github.com/o/r/releases/latest/download/release.json"]);
        curl.file_url = Some("http://127.0.0.1:9/release.json".into());
        curl.read().unwrap().unwrap();
        assert_eq!(urls(&d)[1], "http://127.0.0.1:9/release.json", "HERDR_CONTR_RELEASE_URL names the file");
        let no_zip = json!({"schema": 1, "version": "0.1.7", "tag": TAG, "assets": ["release.json"], "sha256": {}}).to_string();
        serve(&d, "json", 200, no_zip.as_bytes());
        assert_eq!(curl.read().unwrap_err().0, "no-build");
        curl.file_url = None;
        curl.repo = String::new();
        assert_eq!(curl.read().unwrap_err().0, "no-repo");
    }

    #[test]
    fn a_404_or_another_schema_uses_gh() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("c");
        let gh_json = format!(r#"{{"tagName":"{TAG}","url":"https://x/r","assets":[{{"name":"herdr-contr-{TAG}.zip","digest":"sha256:ab"}}]}}"#);
        let src = Releases { curl: fake_curl(&d), gh: fake_gh(t.path(), &gh_json) };
        let r = src.newest().unwrap();
        assert_eq!((r.from_file, r.version.as_deref(), r.releases.len(), r.url.as_str()), (false, None, 0, "https://x/r"), "404: gh");
        serve(&d, "json", 200, br#"{"schema":2,"tag":"x"}"#);
        assert!(!src.newest().unwrap().from_file, "schema 2: gh");
        serve(&d, "json", 200, b"<html>not json</html>");
        assert!(!src.newest().unwrap().from_file, "not JSON: gh");
        assert_eq!(std::fs::read_to_string(t.path().join("gh-args")).unwrap().lines().count(), 3);
    }

    #[test]
    fn another_failure_stops_with_no_second_read() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("c");
        let src = Releases { curl: fake_curl(&d), gh: fake_gh(t.path(), "{}") };
        serve(&d, "json", 500, b"oops");
        let e = src.newest().unwrap_err();
        assert_eq!(e, ("http", "HTTP 500 from https://github.com/o/r/releases/latest/download/release.json.".to_string()));
        std::fs::write(d.join("down"), "").unwrap();
        let e = src.newest().unwrap_err();
        assert_eq!(e.0, "no-network");
        assert!(e.1.contains("Could not resolve host"), "{}", e.1);
        assert_eq!(urls(&d).len(), 2, "one read each");
        assert!(!t.path().join("gh-args").exists(), "gh is not asked");
        let none = Curl { program: t.path().join("none").display().to_string(), repo: "o/r".into(), file_url: None };
        assert_eq!(none.read().unwrap_err().0, "no-network");
    }

    #[test]
    fn a_neutral_root_updates_with_curl() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("c");
        let log = t.path().join("log");
        let zip = std::fs::read(release_zip(t.path(), &log, false)).unwrap();
        serve(&d, "json", 200, release_file(&hex(&zip)).as_bytes());
        serve(&d, "zip", 200, &zip);
        let src = Releases { curl: fake_curl(&d), gh: fake_gh(t.path(), "{}") };
        let root = t.path().join("contr-core");
        let lines = install(&src, &Dest::Root(root.clone()), t.path()).unwrap();
        assert_eq!((lines[1]["type"].as_str(), lines[1]["version"].as_str(), lines[1]["tag"].as_str()), (Some("update-done"), Some("0.1.7"), Some(TAG)));
        assert_eq!(urls(&d)[1], format!("https://github.com/o/r/releases/download/{TAG}/herdr-contr-{TAG}.zip"));
        let runs: Vec<String> = std::fs::read_to_string(&log).unwrap().lines().map(String::from).collect();
        assert_eq!(runs.len(), 2);
        assert!(runs.iter().all(|r| r.starts_with("install --backend --helper ") && !r.contains("--home")), "{runs:?}");
        let env = std::fs::read_to_string(t.path().join("log.env")).unwrap();
        assert_eq!(env, format!("{0}\n{0}\n", root.display()), "CONTR_HOME is the root");
        assert!(!t.path().join("gh-args").exists());
    }

    #[test]
    fn a_bad_sha256_in_the_file_or_a_missing_zip_installs_nothing() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("c");
        let log = t.path().join("log");
        let zip = std::fs::read(release_zip(t.path(), &log, false)).unwrap();
        serve(&d, "json", 200, release_file(&"0".repeat(64)).as_bytes());
        serve(&d, "zip", 200, &zip);
        let src = Releases { curl: fake_curl(&d), gh: fake_gh(t.path(), "{}") };
        let root = t.path().join("contr-core");
        let e = install(&src, &Dest::Root(root.clone()), t.path()).unwrap_err();
        assert_eq!(e.0, "check");
        assert!(e.1.contains("digest"), "{}", e.1);
        assert!(!log.exists() && !root.exists(), "no install ran");
        serve(&d, "zip", 404, b"");
        assert_eq!(install(&src, &Dest::Root(root.clone()), t.path()).unwrap_err().0, "http");
        assert!(!log.exists());
    }

    #[test]
    fn newer_for_higher_same_and_null() {
        assert!(newer(Some("0.1.6"), Some("0.1.7"), false), "higher");
        assert!(newer(Some("0.1.9"), Some("0.1.10"), false), "by number, not text");
        assert!(!newer(Some("0.1.7"), Some("0.1.7"), true), "same");
        assert!(!newer(Some("0.1.8"), Some("0.1.7"), true), "lower");
        assert!(newer(None, Some("0.1.7"), true) && !newer(None, Some("0.1.7"), false), "null: differs");
        assert!(newer(Some("0.1.7"), None, true) && !newer(Some("0.1.7"), None, false), "null: differs");
    }

    #[test]
    fn check_gives_the_versions_and_keeps_them() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("c");
        let root = t.path().join("contr-core");
        std::fs::create_dir_all(&root).unwrap();
        serve(&d, "json", 200, release_file("ab12").as_bytes());
        let src = Releases { curl: fake_curl(&d), gh: fake_gh(t.path(), "{}") };
        let at = |build: &str, release: Value| {
            std::fs::write(root.join("install.json"), json!({"build": build, "release": release}).to_string()).unwrap();
            check(&src, &root, 1000, false).unwrap()
        };
        let l = at("1111111 2026-11-10", json!("0.1.6"));
        assert_eq!((l["installed_version"].as_str(), l["newest_version"].as_str(), l["newer"].as_bool()), (Some("0.1.6"), Some("0.1.7"), Some(true)));
        assert_eq!(l["root"], root.display().to_string());
        assert_eq!(l["releases"][0]["tag"], TAG);
        let kept: Value = serde_json::from_slice(&std::fs::read(root.join("update.json")).unwrap()).unwrap();
        assert_eq!((kept["tag"].as_str(), kept["version"].as_str(), kept["releases"][0]["version"].as_str()), (Some(TAG), Some("0.1.7"), Some("0.1.7")));
        let l = at("abc1234 2026-11-16", json!("0.1.7"));
        assert_eq!((l["newer"].as_bool(), l["differs"].as_bool()), (Some(false), Some(false)), "same");
        let l = at("1111111 2026-11-10", Value::Null);
        assert_eq!((l["installed_version"].clone(), l["newer"].as_bool()), (Value::Null, Some(true)), "null: differs");
        assert_eq!(urls(&d).len(), 1, "update.json kept the version and the releases");
        std::fs::write(root.join("install.json"), r#"{"build":"1111111 2026-11-10"}"#).unwrap();
        assert_eq!(installed_version(&root), None, "an install made before the version");
    }

    #[test]
    fn check_with_gh_gives_no_releases() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("install.json"), r#"{"build":"abc1234 2026-11-16","release":"0.1.7"}"#).unwrap();
        let l = check(&fake(None, None), t.path(), 1000, false).unwrap();
        assert_eq!((l["releases"].clone(), l["newest_version"].clone(), l["newer"].as_bool()), (json!([]), Value::Null, Some(false)));
        std::fs::write(t.path().join("update.json"), json!({"checked_at": 1000, "tag": TAG, "url": "u"}).to_string()).unwrap();
        assert_eq!(check(&fake(None, None), t.path(), 1000, false).unwrap()["releases"], json!([]), "an update.json made before the releases");
    }
}

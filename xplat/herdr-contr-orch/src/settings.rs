//! The settings that the mod shows (#185): the profiles file as JSON (`agents config --json`), a change of one
//! field of it (`agents set`), and the open owner questions as JSON (`owner open --json`). The defaults and
//! the valid values come from `config`, so the mod keeps no copy of them.
use std::fs;
use std::io;
use std::path::Path;

use serde_json::{json, Map, Value};

use crate::config::{Config, Flow, FLOW_CHOICES, FLOW_MIN, FLOW_NUMBERS};
use crate::owner::{Lock, Store};
use crate::workers::{effort_of, model_of};
use crate::jsonfile;

/// The roles that the settings show, in their order.
pub const ROLES: [&str; 5] = ["orchestrator", "planner", "builder", "reviewer", "debugger"];

/// The file as text, or None when it is missing.
fn read(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(path) {
        Ok(d) => Ok(Some(d)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("agents: {}: {e}", path.display())),
    }
}

/// The file as a JSON object; None when it is not one.
fn object(data: &[u8]) -> Option<Map<String, Value>> {
    match serde_json::from_slice(data) {
        Ok(Value::Object(m)) => Some(m),
        _ => None,
    }
}

/// Where claude is in argv: the first argument with the base name "claude", at the start or after "env" and
/// its NAME=value arguments. None when the command is not claude.
pub fn claude_at(argv: &[String]) -> Option<usize> {
    agent_at(argv, "claude")
}

/// Where codex is in argv, as `claude_at`. None when the command is not codex.
pub fn codex_at(argv: &[String]) -> Option<usize> {
    agent_at(argv, "codex")
}

fn agent_at(argv: &[String], name: &str) -> Option<usize> {
    let mut i = 0;
    if argv.first().is_some_and(|a| a == "env") {
        i = 1;
        let is_var = |a: &str| a.split_once('=').is_some_and(|(n, _)| {
            !n.is_empty() && !n.starts_with(|c: char| c.is_ascii_digit()) && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        });
        while i < argv.len() && is_var(&argv[i]) {
            i += 1;
        }
    }
    argv.get(i).filter(|a| a.rsplit('/').next() == Some(name)).map(|_| i)
}

/// Sets `flag` to `v` and keeps the other arguments. A missing flag goes after claude or codex; a missing
/// --effort goes after --model.
pub fn set_flag(argv: &[String], flag: &str, v: &str) -> Vec<String> {
    let mut out = argv.to_vec();
    let eq = format!("{flag}=");
    for i in 0..out.len() {
        if out[i] == flag && i + 1 < out.len() {
            out[i + 1] = v.to_string();
            return out;
        }
        if out[i].starts_with(&eq) {
            out[i] = format!("{eq}{v}");
            return out;
        }
    }
    let mut at = claude_at(&out).or_else(|| codex_at(&out)).map_or(1, |i| i + 1);
    if flag == "--effort" {
        for (i, a) in out.iter().enumerate() {
            if a == "--model" && i + 1 < out.len() {
                at = i + 2;
            } else if a.starts_with("--model=") {
                at = i + 1;
            }
        }
    }
    let at = at.min(out.len());
    out.splice(at..at, [flag.to_string(), v.to_string()]);
    out
}

/// Sets the model or the effort of a codex command: `--model` (or `-m` when argv has it), and the key
/// `model_reasoning_effort` after `-c` or `--config`. A missing effort goes after the model, else after codex.
pub fn set_codex(argv: &[String], flag: &str, v: &str) -> Vec<String> {
    let short = argv.iter().any(|a| a == "-m");
    if flag == "--model" {
        return set_flag(argv, if short { "-m" } else { "--model" }, v);
    }
    const KEY: &str = "model_reasoning_effort=";
    let mut out = argv.to_vec();
    for i in 1..out.len() {
        if (out[i - 1] == "-c" || out[i - 1] == "--config") && out[i].starts_with(KEY) {
            out[i] = format!("{KEY}{v}");
            return out;
        }
    }
    let mut at = codex_at(&out).map_or(1, |i| i + 1);
    for (i, a) in out.iter().enumerate() {
        if (a == "--model" || a == "-m") && i + 1 < out.len() {
            at = i + 2;
        } else if a.starts_with("--model=") {
            at = i + 1;
        }
    }
    let at = at.min(out.len());
    out.splice(at..at, ["-c".to_string(), format!("{KEY}{v}")]);
    out
}

/// The argv of a role: the file's, when it is a list of text that is not empty, else the built-in one.
fn argv_of(file: &Map<String, Value>, role: &str) -> Vec<String> {
    let given: Option<Vec<String>> = file.get("profiles").and_then(|p| p.get(role)).and_then(|p| p.get("argv")).and_then(|a| serde_json::from_value(a.clone()).ok());
    given.filter(|a| !a.is_empty()).unwrap_or_else(|| Config::default().profiles.get(role).map(|p| p.argv.clone()).unwrap_or_default())
}

/// A flow value as JSON when it is valid for `key`.
fn valid(key: &str, v: &Value) -> bool {
    if let Some((_, ok)) = FLOW_CHOICES.iter().find(|(k, _)| *k == key) {
        return v.as_str().is_some_and(|s| ok.contains(&s));
    }
    FLOW_NUMBERS.contains(&key) && v.as_i64().is_some_and(|n| n >= FLOW_MIN)
}

/// The profiles file as one JSON object: its path, `bad` (it is there and not a JSON object), the flow with the
/// values in effect (a value that is not valid gives the default), the default and valid values of each flow
/// key, and each role's argv in effect, if it is claude, its agent ("claude", "codex" or ""), and its model and
/// effort ("" for an agent that is not claude or codex).
pub fn config_json(path: &Path) -> Result<Value, String> {
    let data = read(path)?;
    let file = data.as_deref().map(object);
    let bad = matches!(file, Some(None));
    let file = file.flatten().unwrap_or_default();
    let defaults = serde_json::to_value(Flow::default()).map_err(|e| e.to_string())?;
    let given = file.get("flow").cloned().unwrap_or(Value::Null);
    let mut flow = Map::new();
    let mut fields = Map::new();
    for (key, choices) in FLOW_CHOICES {
        fields.insert(key.into(), json!({"default": defaults[key], "values": choices}));
    }
    for key in FLOW_NUMBERS {
        fields.insert(key.into(), json!({"default": defaults[key], "min": FLOW_MIN}));
    }
    for key in fields.keys() {
        let v = &given[key.as_str()];
        flow.insert(key.clone(), if valid(key, v) { v.clone() } else { defaults[key.as_str()].clone() });
    }
    let roles: Vec<Value> = ROLES
        .iter()
        .map(|role| {
            let argv = argv_of(&file, role);
            let claude = claude_at(&argv).is_some();
            let agent = if claude { "claude" } else if codex_at(&argv).is_some() { "codex" } else { "" };
            let of = |f: fn(&[String]) -> String| if agent.is_empty() { String::new() } else { f(&argv) };
            json!({"name": role, "argv": argv, "claude": claude, "agent": agent, "model": of(model_of), "effort": of(effort_of)})
        })
        .collect();
    Ok(json!({"path": path.display().to_string(), "bad": bad, "flow": flow, "fields": fields, "roles": roles}))
}

/// Applies one `key value` pair to the file's object.
fn apply(file: &mut Map<String, Value>, key: &str, value: &str) -> Result<(), String> {
    if let Some(k) = key.strip_prefix("flow.") {
        let v = if FLOW_NUMBERS.contains(&k) { value.parse::<i64>().map(Value::from).unwrap_or(Value::Null) } else { Value::from(value) };
        if FLOW_CHOICES.iter().all(|(c, _)| *c != k) && !FLOW_NUMBERS.contains(&k) {
            return Err(format!("unknown key {key}"));
        }
        // The flow is written whole, as Config::save does: the defaults, then the file's keys, then the change.
        let mut flow = match serde_json::to_value(Flow::default()) {
            Ok(Value::Object(m)) => m,
            _ => Map::new(),
        };
        if let Some(Value::Object(was)) = file.get("flow") {
            flow.extend(was.clone());
        }
        // Only the changed key is checked, so another key that is not valid does not block the change.
        let mut one = match serde_json::to_value(Flow::default()) {
            Ok(Value::Object(m)) => m,
            _ => Map::new(),
        };
        one.insert(k.into(), v.clone());
        let check: Flow = serde_json::from_value(Value::Object(one)).map_err(|_| format!("{key}: {value:?} is not a number"))?;
        check.validate()?;
        flow.insert(k.into(), v);
        file.insert("flow".into(), Value::Object(flow));
        return Ok(());
    }
    let parts: Vec<&str> = key.split('.').collect();
    let (role, flag) = match parts.as_slice() {
        ["profile", role, "model"] if ROLES.contains(role) => (*role, "--model"),
        ["profile", role, "effort"] if ROLES.contains(role) => (*role, "--effort"),
        _ => return Err(format!("unknown key {key}")),
    };
    if value.is_empty() || value.starts_with('-') || value.chars().any(char::is_whitespace) {
        return Err(format!("{key}: {value:?} is not a valid value"));
    }
    let argv = argv_of(file, role);
    // A command that is not claude or codex is never changed.
    let argv = if claude_at(&argv).is_some() {
        set_flag(&argv, flag, value)
    } else if codex_at(&argv).is_some() {
        set_codex(&argv, flag, value)
    } else {
        return Ok(());
    };
    if !matches!(file.get("profiles"), Some(Value::Object(_))) {
        file.insert("profiles".into(), Value::Object(Map::new()));
    }
    let Some(Value::Object(profiles)) = file.get_mut("profiles") else { unreachable!() };
    let mut p = match profiles.get(role) {
        Some(Value::Object(p)) => p.clone(),
        _ => Map::new(),
    };
    p.insert("argv".into(), json!(argv));
    profiles.insert(role.into(), Value::Object(p));
    Ok(())
}

/// Sets each `key value` pair in the file under its lock, and keeps all other keys. Keys: flow.<key>,
/// profile.<role>.model and profile.<role>.effort (a role that is not claude or codex stays as it is). A file that is not a JSON object, an unknown key or a
/// value that is not valid changes nothing. A missing file is made.
pub fn set(path: &Path, pairs: &[(String, String)]) -> Result<(), String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir).map_err(|e| format!("agents: {}: {e}", dir.display()))?;
    }
    let mut lock_path = path.as_os_str().to_owned();
    lock_path.push(".lock");
    let _lock = Lock::take(Path::new(&lock_path)).map_err(|e| format!("agents: lock: {e}"))?;
    let mut file = match read(path)? {
        None => Map::new(),
        Some(d) => object(&d).ok_or_else(|| format!("agents: {}: not a JSON object; it is not changed", path.display()))?,
    };
    for (k, v) in pairs {
        apply(&mut file, k, v).map_err(|e| format!("agents: {e}"))?;
    }
    let mut out = serde_json::to_vec_pretty(&file).map_err(|e| e.to_string())?;
    out.push(b'\n');
    jsonfile::write(path, &out, 0o644).map_err(|e| format!("agents: {}: {e}", path.display()))
}

/// The open items, with the fields that their cards show: the questions that have no answer
/// (`kind` "question"), then the to-dos that are not done (`kind` "todo"; `todo_kind` is do or know; `due` is YYYY-MM-DD or empty).
/// Each group is oldest first.
pub fn open_items(store: &Store) -> Result<Value, String> {
    let (qs, ts) = store.read()?.open("");
    let qs = qs.into_iter().map(|q| {
        json!({"kind": "question", "id": q.id, "coordinator": q.coordinator, "title": q.title, "from": q.from, "question": q.question,
               "options": q.options, "impacts": q.impacts, "recommended": q.recommended, "refs": q.refs})
    });
    let ts = ts.into_iter().map(|t| {
        json!({"kind": "todo", "id": t.id, "coordinator": t.coordinator, "from": t.from, "todo_kind": t.kind, "text": t.text, "due": t.due})
    });
    Ok(Value::Array(qs.chain(ts).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn claude_at_finds_claude_after_env() {
        assert_eq!(claude_at(&v(&["claude"])), Some(0));
        assert_eq!(claude_at(&v(&["/usr/bin/claude", "-p"])), Some(0));
        assert_eq!(claude_at(&v(&["env", "A=1", "B_2=x", "claude"])), Some(3));
        assert_eq!(claude_at(&v(&["env", "1A=1", "claude"])), None);
        assert_eq!(claude_at(&v(&["codex"])), None);
        assert_eq!(claude_at(&[]), None);
    }

    #[test]
    fn set_codex_sets_the_model_and_the_effort_key() {
        assert_eq!(codex_at(&v(&["env", "A=1", "/bin/codex"])), Some(2));
        assert_eq!(set_codex(&v(&["codex", "--model", "a", "x"]), "--model", "b"), v(&["codex", "--model", "b", "x"]));
        assert_eq!(set_codex(&v(&["codex", "-m", "a"]), "--model", "b"), v(&["codex", "-m", "b"]));
        assert_eq!(set_codex(&v(&["codex", "x"]), "--model", "b"), v(&["codex", "--model", "b", "x"]));
        assert_eq!(set_codex(&v(&["codex", "-c", "model_reasoning_effort=low", "x"]), "--effort", "high"), v(&["codex", "-c", "model_reasoning_effort=high", "x"]));
        assert_eq!(set_codex(&v(&["codex", "--config", "model_reasoning_effort=\"low\""]), "--effort", "high"), v(&["codex", "--config", "model_reasoning_effort=high"]));
        assert_eq!(set_codex(&v(&["codex", "-m", "a", "-c", "x=1"]), "--effort", "low"), v(&["codex", "-m", "a", "-c", "model_reasoning_effort=low", "-c", "x=1"]));
        assert_eq!(set_codex(&v(&["codex", "x"]), "--effort", "low"), v(&["codex", "-c", "model_reasoning_effort=low", "x"]));
        // The key as a plain word, not after -c, is not the effort.
        assert_eq!(set_codex(&v(&["codex", "model_reasoning_effort=low"]), "--effort", "high"), v(&["codex", "-c", "model_reasoning_effort=high", "model_reasoning_effort=low"]));
    }

    #[test]
    fn set_flag_replaces_or_adds_the_flag() {
        assert_eq!(set_flag(&v(&["claude", "--model", "opus"]), "--model", "sonnet"), v(&["claude", "--model", "sonnet"]));
        assert_eq!(set_flag(&v(&["claude", "--model=opus"]), "--model", "haiku"), v(&["claude", "--model=haiku"]));
        assert_eq!(set_flag(&v(&["env", "A=1", "claude", "-p"]), "--model", "opus"), v(&["env", "A=1", "claude", "--model", "opus", "-p"]));
        assert_eq!(set_flag(&v(&["claude", "-p", "--model", "opus", "x"]), "--effort", "low"), v(&["claude", "-p", "--model", "opus", "--effort", "low", "x"]));
        assert_eq!(set_flag(&v(&["claude", "--model=opus", "x"]), "--effort", "low"), v(&["claude", "--model=opus", "--effort", "low", "x"]));
        assert_eq!(set_flag(&v(&["claude", "--model"]), "--model", "opus"), v(&["claude", "--model", "opus", "--model"]));
    }
}

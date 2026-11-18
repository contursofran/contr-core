//! The opt-in Codex question hook and its entry in the user's hooks.json.
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use herdr_core::model::QuestionRequest;
use herdr_core::question::{codex, CODEX_TOOLS};
use serde_json::{json, Map, Value};

use crate::batch;
use crate::spawn::ENV_COORDINATOR;
use crate::workers::{Core, Manager};

const MATCHER: &str = "^request_user_input(_async)?$";
const MARKER: &str = " # contr-core-managed";
const USAGE: &str = "Usage: herdr-contr-orch codex hooks on|off|status [--file <hooks.json>]";

/// The hook always lets the call continue, even when a report cannot be sent.
pub(crate) fn question_hook(
    env: &dyn Fn(&str) -> String,
    input: &mut dyn BufRead,
    core: &dyn Core,
) {
    let to = env(ENV_COORDINATOR);
    if to.is_empty() {
        return;
    }
    let Ok(hook) = serde_json::from_reader::<_, Value>(input) else {
        return;
    };
    let tool = hook["tool_name"].as_str().unwrap_or_default();
    if !CODEX_TOOLS.contains(&tool) {
        return;
    }
    let id = hook["tool_use_id"].as_str().unwrap_or_default();
    let Some(request) = parse(tool, id, &hook["tool_input"])
        .or_else(|| latest_question(hook["transcript_path"].as_str()?))
    else {
        return;
    };

    let caller = env("HC_TERMINAL_ID");
    let worker = if caller.is_empty() {
        "Codex worker".into()
    } else {
        Manager::new(core, caller.clone())
            .get(&caller)
            .ok()
            .map(|w| w.name)
            .filter(|name| !name.is_empty())
            .unwrap_or(caller)
    };
    let mut text = format!("{worker}: question");
    for q in request.questions {
        text.push_str(&format!("\n{}", q.text));
        for option in q.options {
            text.push_str(&format!("\n- {}", option.label));
            if option.recommended {
                text.push_str(" (Recommended)");
            }
            if !option.detail.is_empty() {
                text.push_str(&format!(": {}", option.detail));
            }
        }
    }
    text.push_str(&format!("\nReply with the `message` tool to {worker}."));
    let _ = batch::report(core, env, &to, &text, batch::now(), &batch::arm_timer);
}

fn parse(tool: &str, id: &str, input: &Value) -> Option<QuestionRequest> {
    let mut request = match input.as_str() {
        Some(args) => codex(tool, id, args),
        None => codex(tool, id, &input.to_string()),
    }?;
    request.questions.retain(|q| !q.text.trim().is_empty());
    (!request.questions.is_empty()).then_some(request)
}

fn latest_question(path: &str) -> Option<QuestionRequest> {
    let mut latest = None;
    for line in BufReader::new(File::open(path).ok()?)
        .lines()
        .map_while(Result::ok)
    {
        let Ok(row) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let call = &row["payload"];
        if row["type"] == "response_item" && call["type"] == "function_call" {
            if let Some(question) = parse(
                call["name"].as_str().unwrap_or_default(),
                call["call_id"].as_str().unwrap_or_default(),
                &call["arguments"],
            ) {
                latest = Some(question);
            }
        }
    }
    latest
}

pub(crate) fn run(
    args: &[String],
    env: &dyn Fn(&str) -> String,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> i32 {
    let (action, file) = match args {
        [hooks, action] if hooks == "hooks" => (action.as_str(), None),
        [hooks, action, flag, path] if hooks == "hooks" && flag == "--file" => {
            (action.as_str(), Some(PathBuf::from(path)))
        }
        _ => ("", None),
    };
    if !matches!(action, "on" | "off" | "status") {
        let _ = writeln!(err, "{USAGE}");
        return 2;
    }
    match file
        .map_or_else(|| hooks_path(env), Ok)
        .and_then(|path| update(&path, action))
    {
        Ok(on) => {
            let _ = writeln!(
                out,
                "Codex question hook: {}",
                if on { "on" } else { "off" }
            );
            let _ = writeln!(out, "Use /hooks in Codex to trust a new or changed hook.");
            0
        }
        Err(e) => {
            let _ = writeln!(err, "codex hooks: {e}");
            1
        }
    }
}

fn hooks_path(env: &dyn Fn(&str) -> String) -> Result<PathBuf, String> {
    let codex_home = env("CODEX_HOME");
    if !codex_home.is_empty() {
        return Ok(PathBuf::from(codex_home).join("hooks.json"));
    }
    let home = env("HOME");
    if home.is_empty() {
        return Err("set CODEX_HOME or HOME, or use --file".into());
    }
    Ok(PathBuf::from(home).join(".codex/hooks.json"))
}

fn managed(hook: &Value) -> bool {
    hook["type"] == "command"
        && hook["command"]
            .as_str()
            .is_some_and(|cmd| cmd.ends_with(MARKER))
}

fn update(path: &Path, action: &str) -> Result<bool, String> {
    update_file(path, action).map_err(|e| format!("{}: {e}", path.display()))
}

fn update_file(path: &Path, action: &str) -> Result<bool, Box<dyn std::error::Error>> {
    let mut doc: Map<String, Value> = match fs::read(path) {
        Ok(bytes) => {
            let value: Value = serde_json::from_slice(&bytes)?;
            match value {
                Value::Object(object) => object,
                _ => return Err("expected a JSON object".into()),
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Map::new(),
        Err(e) => return Err(e.into()),
    };
    let hooks = doc.entry("hooks").or_insert_with(|| json!({}));
    let event = hooks
        .as_object_mut()
        .ok_or("expected a hooks object")?
        .entry("PreToolUse")
        .or_insert_with(|| json!([]));
    let groups = event.as_array_mut().ok_or("expected a PreToolUse array")?;
    let on = groups.iter().any(|group| {
        group["hooks"]
            .as_array()
            .is_some_and(|hooks| hooks.iter().any(managed))
    });
    match action {
        "on" if !on => {
            let exe = std::env::current_exe()?;
            let exe = exe
                .to_str()
                .ok_or("program path is not UTF-8")?
                .replace('\'', "'\\''");
            let command = format!(
                "[ -z \"$CONTR_COORDINATOR\" ] || '{exe}' hook codex-question || true{MARKER}"
            );
            groups.push(
                json!({"matcher": MATCHER, "hooks": [{"type": "command", "command": command}]}),
            );
        }
        "off" if on => groups.retain_mut(|group| {
            let Some(hooks) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
                return true;
            };
            let before = hooks.len();
            hooks.retain(|hook| !managed(hook));
            before == hooks.len()
                || !hooks.is_empty()
                || group
                    .as_object()
                    .is_some_and(|g| g.keys().any(|k| k != "matcher" && k != "hooks"))
        }),
        _ => return Ok(on),
    }
    let mut bytes = serde_json::to_vec_pretty(&doc)?;
    bytes.push(b'\n');
    crate::jsonfile::write(path, &bytes, 0o600)?;
    Ok(action == "on")
}

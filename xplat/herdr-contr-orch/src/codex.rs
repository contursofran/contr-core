//! Codex coordinator launch and the opt-in hooks in the user's hooks.json.
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use herdr_core::model::QuestionRequest;
use herdr_core::question::{codex, CODEX_TOOLS};
use serde_json::{json, Map, Value};

use crate::batch;
use crate::owner::Lock;
use crate::spawn::{CODEX_ENV, CODEX_SERVER, ENV_COORDINATOR};
use crate::tools::{agent_tools, owner_tools};
use crate::workers::{Core, Manager};

const MARKER: &str = " # contr-core-managed";
pub(crate) const COORDINATOR_ENV: &str = "CONTR_CODEX_COORDINATOR";
const USAGE: &str = "Usage:\n  herdr-contr-orch codex coordinator [Codex arguments…]\n  herdr-contr-orch codex hooks [coordinator] on|off|status [--file <hooks.json>]";

pub(crate) fn is_coordinator(env: &dyn Fn(&str) -> String) -> bool {
    env(COORDINATOR_ENV) == "1"
        && env(ENV_COORDINATOR).is_empty()
        && !env("HC_TERMINAL_ID").is_empty()
}

fn coordinator(
    args: &[String],
    env: &dyn Fn(&str) -> String,
    core: &dyn Core,
) -> Result<i32, String> {
    if env("HC_TERMINAL_ID").is_empty() || !env(ENV_COORDINATOR).is_empty() {
        return Err("start a Codex coordinator in its own hc terminal".into());
    }
    let cfg = crate::tools::load_config(env)?;
    let kit = if cfg.kit.is_empty() {
        crate::home::places(env)?.kit()
    } else {
        PathBuf::from(cfg.kit)
    };
    let kit = std::path::absolute(kit).map_err(|e| e.to_string())?;
    fs::metadata(kit.join("commands/orchestrate.md"))
        .map_err(|e| format!("{}: {e}", kit.display()))?;

    let store = crate::home::owner_store(env)?;
    let names: Vec<_> = agent_tools(core, env)
        .into_iter()
        .chain(owner_tools(&store, "", ""))
        .map(|tool| tool.name)
        .collect();
    let env_vars: Vec<_> = CODEX_ENV
        .into_iter()
        .chain([
            "CONTR_KIT",
            "CONTR_STATE_DIR",
            "CONTR_PROJECT",
            "HERDR_DESK_HC_SOCKET",
            "XDG_RUNTIME_DIR",
            "XDG_STATE_HOME",
        ])
        .collect();
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.to_str().ok_or("program path is not UTF-8")?;
    // Codex merges each key with lower config layers, including tool filters.
    let overrides = [
        ("command", json!(exe)),
        ("args", json!(["mcp"])),
        ("env_vars", json!(env_vars)),
        ("enabled", json!(true)),
        ("enabled_tools", json!(names)),
        ("disabled_tools", json!([])),
    ];
    let mut command = Command::new("codex");
    command.env(COORDINATOR_ENV, "1").env("CONTR_KIT", kit);
    for (key, value) in overrides {
        command.args(["-c", &format!("{CODEX_SERVER}.{key}={value}")]);
    }
    command.args(args);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(command.exec().to_string())
    }
    #[cfg(not(unix))]
    command
        .status()
        .map(|s| s.code().unwrap_or(1))
        .map_err(|e| e.to_string())
}

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
    core: &dyn Core,
) -> i32 {
    if let [sub, rest @ ..] = args {
        if sub == "coordinator" {
            return coordinator(rest, env, core).unwrap_or_else(|e| {
                let _ = writeln!(err, "codex coordinator: {e}");
                1
            });
        }
    }
    let (coordinator, args) = match args {
        [hooks, scope, rest @ ..] if hooks == "hooks" && scope == "coordinator" => (true, rest),
        [hooks, rest @ ..] if hooks == "hooks" => (false, rest),
        _ => (false, &[][..]),
    };
    let (action, file) = match args {
        [action] => (action.as_str(), None),
        [action, flag, path] if flag == "--file" => (action.as_str(), Some(PathBuf::from(path))),
        _ => ("", None),
    };
    if !matches!(action, "on" | "off" | "status") {
        let _ = writeln!(err, "{USAGE}");
        return 2;
    }
    let (label, specs) = if coordinator {
        ("coordinator hooks", COORDINATOR_HOOKS)
    } else {
        ("question hook", QUESTION_HOOKS)
    };
    match file.map_or_else(|| hooks_path(env), Ok).and_then(|path| {
        update_file(&path, action, specs).map_err(|e| format!("{}: {e}", path.display()))
    }) {
        Ok(status) => {
            let _ = writeln!(out, "Codex {label}: {status}");
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

struct HookSpec {
    event: &'static str,
    matcher: Option<&'static str>,
    name: &'static str,
}

const QUESTION_HOOKS: &[HookSpec] = &[HookSpec {
    event: "PreToolUse",
    matcher: Some("^request_user_input(_async)?$"),
    name: "codex-question",
}];
const COORDINATOR_HOOKS: &[HookSpec] = &[
    HookSpec {
        event: "SessionStart",
        matcher: Some("^(startup|resume|clear|compact|fork)$"),
        name: "codex-session-start",
    },
    HookSpec {
        event: "Stop",
        matcher: None,
        name: "codex-stop",
    },
    HookSpec {
        event: "UserPromptSubmit",
        matcher: None,
        name: "codex-prompt",
    },
];

fn update_file(
    path: &Path,
    action: &str,
    specs: &[HookSpec],
) -> Result<&'static str, Box<dyn std::error::Error>> {
    // Both hook sets share the same file. Hold the lock across the read and atomic write.
    let _lock = if action == "on" || (action == "off" && path.try_exists()?) {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        let mut lock_path = path.as_os_str().to_owned();
        lock_path.push(".lock");
        Some(Lock::take(Path::new(&lock_path))?)
    } else {
        None
    };
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
    let hooks = doc
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or("expected a hooks object")?;
    let (mut enabled, mut changed) = (0, false);
    for spec in specs {
        if action != "on" && !hooks.contains_key(spec.event) {
            continue;
        }
        let groups = hooks
            .entry(spec.event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or_else(|| format!("expected a {} array", spec.event))?;
        let (on, modified) = spec.update(groups, action)?;
        enabled += usize::from(on);
        changed |= modified;
    }
    if changed {
        let mut bytes = serde_json::to_vec_pretty(&doc)?;
        bytes.push(b'\n');
        crate::jsonfile::write(path, &bytes, 0o600)?;
    }
    Ok(if enabled == specs.len() {
        "on"
    } else if enabled == 0 {
        "off"
    } else {
        "partial"
    })
}

impl HookSpec {
    fn update(
        &self,
        groups: &mut Vec<Value>,
        action: &str,
    ) -> Result<(bool, bool), Box<dyn std::error::Error>> {
        let marker = if self.name == "codex-question" {
            MARKER.to_string()
        } else {
            format!("{MARKER}-{}", self.name)
        };
        let managed = |hook: &Value| {
            hook["type"] == "command"
                && hook["command"]
                    .as_str()
                    .is_some_and(|cmd| cmd.ends_with(&marker))
        };
        let on = groups.iter().any(|g| {
            g["hooks"]
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
                let command = if self.name == "codex-question" {
                    format!(
                        "[ -z \"$CONTR_COORDINATOR\" ] || '{exe}' hook {} || true{marker}",
                        self.name
                    )
                } else {
                    format!("'{exe}' hook {}{marker}", self.name)
                };
                let mut group = json!({"hooks": [{"type": "command", "command": command}]});
                if let Some(matcher) = self.matcher {
                    group["matcher"] = json!(matcher);
                }
                if self.event == "SessionStart" {
                    group["hooks"][0]["additionalContextLimit"] = json!(5000);
                }
                groups.push(group);
                Ok((true, true))
            }
            "off" if on => {
                groups.retain_mut(|group| {
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
                });
                Ok((false, true))
            }
            _ => Ok((on, false)),
        }
    }
}

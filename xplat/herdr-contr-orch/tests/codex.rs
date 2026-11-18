//! Codex hook payloads and hooks.json edits, with a fake core and temporary files.
mod common;

use std::fs;

use common::{call, call_with, path, Fake, Run};
use serde_json::{json, Value};

const ASYNC: &str = r#"{"questions":[{"title":"Which plan?","options":["Fast","Safe"]},{"title":"Any notes?"}]}"#;
const BLOCKING: &str = r#"{"questions":[{"id":"color","header":"Color","question":"Pick a color?","options":[{"label":"Red (Recommended)","description":"Warm."},{"label":"Blue","description":"Cool."}]}]}"#;
const REPLY: &str = "\nReply with the `message` tool to b-19.";

fn payload(tool: &str, input: Value) -> Value {
    json!({"hook_event_name":"PreToolUse","tool_name":tool,"tool_input":input,"tool_use_id":"call_c5"})
}

fn hook(core: &Fake, env: &[(&str, &str)], input: &str) {
    let r = call_with(&["hook", "codex-question"], env, core, input);
    assert_eq!((r.code, r.out.as_str(), r.err.as_str()), (0, "", ""));
}

fn worker(state: &str) -> [(&str, &str); 4] {
    [("CONTR_COORDINATOR", "t1"), ("HC_TERMINAL_ID", "t7"), ("CONTR_STATE_DIR", state), ("CONTR_AGENTS_CONFIG", state)]
}

fn core() -> Fake {
    Fake::default().reply("terminal.get", json!({"name": "b-19", "address": "t7"}))
}

fn sent(core: &Fake) -> Vec<Value> {
    core.calls().into_iter().filter(|(method, _)| method == "terminal.send").map(|(_, args)| args).collect()
}

#[test]
fn async_questions_send_at_once_or_wait_for_a_busy_coordinator() {
    let state = tempfile::tempdir().unwrap();
    let vars = worker(path(state.path()));
    let text = format!("b-19: question\nWhich plan?\n- Fast\n- Safe\nAny notes?{REPLY}");
    let input = payload("request_user_input_async", serde_json::from_str(ASYNC).unwrap()).to_string();

    let free = core();
    hook(&free, &vars, &input);
    assert_eq!(sent(&free), [json!({"terminal":"t1","caller":"t7","text":text})]);

    // The first send set the batch time. This question still tries to send immediately.
    let busy = core().reply("terminal.send", json!({"sent":false,"reason":"blocked"}));
    hook(&busy, &vars, &input);
    assert_eq!(sent(&busy)[0]["text"], text);
    assert!(!busy.methods().contains(&"terminal.list".to_string()));
    let held = orch::held::take(&|k| vars.iter().find(|(key, _)| *key == k).map_or("", |(_, value)| value).into(), "t1").unwrap();
    assert_eq!(held, [orch::held::Report { from: "t7 b-19".into(), text }]);
}

#[test]
fn blocking_questions_accept_object_and_string_arguments() {
    let state = tempfile::tempdir().unwrap();
    let text = format!("b-19: question\nPick a color?\n- Red (Recommended): Warm.\n- Blue: Cool.{REPLY}");
    for input in [serde_json::from_str(BLOCKING).unwrap(), json!(BLOCKING)] {
        let fake = core();
        hook(&fake, &worker(path(state.path())), &payload("request_user_input", input).to_string());
        assert_eq!(sent(&fake)[0]["text"], text);
    }
}

#[test]
fn missing_question_text_uses_the_newest_transcript_call() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("rollout.jsonl");
    fs::write(&log, format!("{}\n{{", include_str!("../../fixtures/question-codex.jsonl"))).unwrap();
    let fake = core();
    let mut input = payload("request_user_input", json!({"questions":[{"question":" "}]}));
    input["transcript_path"] = json!(log);
    hook(&fake, &worker(path(dir.path())), &input.to_string());
    assert_eq!(sent(&fake)[0]["text"], format!("b-19: question\nShip it?\n- Now: n\n- Later: l{REPLY}"));
}

#[test]
fn ignored_inputs_and_failed_sends_are_silent() {
    let dir = tempfile::tempdir().unwrap();
    let vars = worker(path(dir.path()));
    let valid = payload("request_user_input_async", json!(ASYNC)).to_string();
    let other = payload("shell", json!(ASYNC)).to_string();
    let missing = payload("request_user_input_async", Value::Null).to_string();
    let fake = core();
    for (env, input) in [(&vars[1..], valid.as_str()), (&vars[..], "{"), (&vars[..], &other), (&vars[..], &missing)] {
        hook(&fake, env, input);
    }
    assert!(fake.calls().is_empty());
    let failed = core().err("terminal.send", "hc: not_found");
    hook(&failed, &vars, &valid);
    assert_eq!(sent(&failed).len(), 1);
}

fn hooks(action: &str, file: &std::path::Path) -> Run {
    call(&["codex", "hooks", action, "--file", path(file)], &[], &Fake::default())
}

fn status(r: Run, state: &str) {
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.err, "");
    assert!(r.out.contains(&format!("Codex question hook: {state}")));
    assert!(r.out.contains("/hooks"));
}

#[test]
fn hooks_toggle_only_the_managed_entry() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("hooks.json");
    let original = json!({"description":"other hooks","hooks":{
        "Stop":[{"hooks":[{"type":"command","command":"notify"}]}],
        "PreToolUse":[{"matcher":"^request_user_input(_async)?$","hooks":[
            {"type":"command","command":"other # contr-managed"}
        ]}]
    }});
    fs::write(&file, original.to_string()).unwrap();
    status(hooks("status", &file), "off");
    status(hooks("on", &file), "on");
    let bytes = fs::read(&file).unwrap();
    let mut added: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(added["hooks"]["PreToolUse"][0], original["hooks"]["PreToolUse"][0]);
    let entry = added["hooks"]["PreToolUse"][1].clone();
    assert_eq!(entry["matcher"], "^request_user_input(_async)?$");
    assert_eq!(entry["hooks"][0]["type"], "command");
    let exe = std::env::current_exe().unwrap();
    let quoted = path(&exe).replace('\'', "'\\''");
    assert_eq!(entry["hooks"][0]["command"], format!(
        "[ -z \"$CONTR_COORDINATOR\" ] || '{quoted}' hook codex-question || true # contr-core-managed"
    ));
    status(hooks("on", &file), "on");
    status(hooks("status", &file), "on");
    assert_eq!(fs::read(&file).unwrap(), bytes);

    // A user can put another command in the same group.
    let groups = added["hooks"]["PreToolUse"].as_array_mut().unwrap();
    groups.pop();
    groups[0]["hooks"].as_array_mut().unwrap().push(entry["hooks"][0].clone());
    fs::write(&file, added.to_string()).unwrap();
    status(hooks("off", &file), "off");
    assert_eq!(serde_json::from_slice::<Value>(&fs::read(&file).unwrap()).unwrap(), original);
}

#[test]
fn hooks_use_codex_home_and_make_a_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let codex = dir.path().join("codex");
    let file = codex.join("hooks.json");
    let vars = [("CODEX_HOME", path(&codex)), ("HOME", path(&home))];
    status(hooks("off", &file), "off");
    assert!(!file.exists());
    status(call(&["codex", "hooks", "on"], &vars, &Fake::default()), "on");
    assert!(file.is_file() && !home.exists());
    status(hooks("off", &file), "off");
    assert_eq!(serde_json::from_slice::<Value>(&fs::read(file).unwrap()).unwrap()["hooks"]["PreToolUse"], json!([]));
    status(call(&["codex", "hooks", "on"], &vars[1..], &Fake::default()), "on");
    assert!(home.join(".codex/hooks.json").is_file());
}

#[test]
fn invalid_hook_files_are_never_rewritten() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("hooks.json");
    for invalid in ["{", "[]", r#"{"hooks":{"PreToolUse":false}}"#] {
        fs::write(&file, invalid).unwrap();
        for action in ["on", "off", "status"] {
            let r = hooks(action, &file);
            assert_eq!(r.code, 1);
            assert!(r.err.contains("codex hooks:"));
            assert_eq!(fs::read_to_string(&file).unwrap(), invalid);
        }
    }
}

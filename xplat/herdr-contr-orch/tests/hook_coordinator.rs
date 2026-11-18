//! The hooks of a coordinator, on a fake hc core. The transcripts, the owner file, the kit and the folder of
//! the markers are in temporary folders.
mod common;

use std::fs;
use std::path::Path;

use common::{call_with, path, Fake, Run};
use orch::hook::{short_model, CHAT_SKILL_LINE, RESTORE_MAX};
use orch::owner::{Question, Store, Todo};
use serde_json::{json, Value};

const PROMPT: &str = r#"{"type":"user","message":{"role":"user","content":"do the task"}}"#;
const ORCHESTRATE: &str = r#"{"type":"user","message":{"content":"<command-name>/contr:orchestrate</command-name>"}}"#;

fn reply(tokens: i64) -> String {
    format!(r#"{{"type":"assistant","message":{{"model":"claude-opus-5-5","content":[],"usage":{{"input_tokens":{tokens}}}}}}}"#)
}

fn transcript(dir: &Path, lines: &[&str]) -> String {
    let p = dir.join("t.jsonl");
    fs::write(&p, lines.join("\n") + "\n").unwrap();
    path(&p).to_string()
}

/// A terminal of hc with this role.
fn terminal(role: &str) -> Fake {
    Fake::default().reply("terminal.get", json!({"address": "t1", "name": "lead", "state": "live", "labels": {"contr": "agent", "role": role}}))
}

fn hook(name: &str, vars: &[(&str, &str)], input: Value, core: &Fake) -> Run {
    call_with(&["hook", name], vars, core, &input.to_string())
}

/// The labels of the calls of terminal.label.
fn labels(core: &Fake) -> Vec<Value> {
    core.calls().into_iter().filter(|(m, _)| m == "terminal.label").map(|(_, p)| p).collect()
}

#[test]
fn a_coordinator_is_asked_one_time_to_save_its_state() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = path(dir.path());
    let vars = [("HC_TERMINAL_ID", "t1"), ("TMPDIR", tmp)];
    let core = Fake::default();
    let input = json!({"session_id": "s1", "transcript_path": transcript(dir.path(), &[PROMPT, ORCHESTRATE, &reply(231_500)])});
    let r = hook("stop", &vars, input.clone(), &core);
    assert_eq!((r.code, r.err.as_str()), (0, ""));
    let block: Value = serde_json::from_str(&r.out).unwrap();
    assert_eq!(block["decision"], "block");
    let reason = block["reason"].as_str().unwrap();
    assert!(reason.contains("231k tokens") && reason.contains("Update COORDINATOR-STATE.md now"), "{reason}");
    // The second Stop of the session asks no more. A compaction clears that, and the next Stop asks again.
    assert_eq!(hook("stop", &vars, input.clone(), &core).out, "");
    hook("session-start", &vars, json!({"session_id": "s1", "source": "compact", "transcript_path": input["transcript_path"]}), &core);
    assert!(hook("stop", &vars, input, &core).out.contains("block"));
}

#[test]
fn the_stop_of_a_coordinator_is_quiet_when_there_is_nothing_to_save() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = path(dir.path());
    let vars = [("HC_TERMINAL_ID", "t1"), ("TMPDIR", tmp)];
    let big = transcript(dir.path(), &[PROMPT, ORCHESTRATE, &reply(240_000)]);
    // The hook's own block is on: it does not block again.
    let r = hook("stop", &vars, json!({"session_id": "s1", "transcript_path": big, "stop_hook_active": true}), &Fake::default());
    assert_eq!((r.code, r.out.as_str()), (0, ""));
    // No transcript file, and no transcript path.
    let core = Fake::default();
    let r = hook("stop", &vars, json!({"session_id": "s1", "transcript_path": path(&dir.path().join("gone.jsonl"))}), &core);
    assert_eq!((r.code, r.out.as_str(), r.err.as_str()), (0, "", ""));
    assert_eq!(hook("stop", &vars, json!({"session_id": "s1"}), &core).code, 0);
    assert!(core.calls().is_empty());
    // A small context.
    let small = transcript(dir.path(), &[PROMPT, ORCHESTRATE, &reply(1000)]);
    assert_eq!(hook("stop", &vars, json!({"session_id": "s1", "transcript_path": small}), &Fake::default()).out, "");
    // CONTR_SAVE_AT moves the limit.
    let low = [("HC_TERMINAL_ID", "t1"), ("TMPDIR", tmp), ("CONTR_SAVE_AT", "500")];
    assert!(hook("stop", &low, json!({"session_id": "s2", "transcript_path": small}), &Fake::default()).out.contains("block"));
    // A session that is no coordinator: it ran no orchestrate command, and its terminal is a builder.
    let plain = transcript(dir.path(), &[PROMPT, &reply(240_000)]);
    let core = terminal("builder");
    assert_eq!(hook("stop", &vars, json!({"session_id": "s3", "transcript_path": plain}), &core).out, "");
    assert_eq!(core.methods(), ["terminal.label", "terminal.get"]);
    // The terminal's role makes a coordinator too.
    assert!(hook("stop", &vars, json!({"session_id": "s3", "transcript_path": plain}), &terminal("orchestrator")).out.contains("block"));
    // hc does not answer: an error, and no block.
    let r = hook("stop", &vars, json!({"session_id": "s4", "transcript_path": plain}), &Fake::default().err("terminal.get", "hc is down"));
    assert_eq!((r.code, r.out.as_str(), r.err.as_str()), (1, "", "herdr-contr-orch hook: hc is down\n"));
}

#[test]
fn the_stop_labels_the_terminal_with_the_context_the_model_and_the_effort() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = path(dir.path());
    let plain = transcript(dir.path(), &[PROMPT, &reply(1234)]);
    let core = Fake::default();
    hook("stop", &[("HC_TERMINAL_ID", "t1"), ("TMPDIR", tmp), ("CLAUDE_EFFORT", "low")], json!({"transcript_path": plain, "effort": {"level": "high"}}), &core);
    assert_eq!(labels(&core), [json!({"terminal": "t1", "labels": {"contr.context": "1234", "contr.context-limit": "250000", "contr.model": "opus", "contr.effort": "high"}})]);

    // The effort of the environment, the window of Claude Code, and the label of a session that orchestrates.
    let orch = transcript(dir.path(), &[PROMPT, ORCHESTRATE, &reply(1234)]);
    let core = Fake::default();
    hook("stop", &[("HC_TERMINAL_ID", "t1"), ("TMPDIR", tmp), ("CLAUDE_EFFORT", "low"), ("CLAUDE_CODE_AUTO_COMPACT_WINDOW", "400000")], json!({"transcript_path": orch}), &core);
    assert_eq!(labels(&core)[0]["labels"], json!({"contr.context": "1234", "contr.context-limit": "400000", "contr.model": "opus", "contr.effort": "low", "contr.orchestrating": "1"}));

    // No usage and no model yet: the size is removed, and the other labels keep their value.
    let core = Fake::default();
    hook("stop", &[("HC_TERMINAL_ID", "t1"), ("TMPDIR", tmp)], json!({"transcript_path": transcript(dir.path(), &[PROMPT])}), &core);
    assert_eq!(labels(&core)[0]["labels"], json!({"contr.context": ""}));
    // A label that hc refuses does not fail the hook.
    let r = hook("stop", &[("HC_TERMINAL_ID", "t1"), ("TMPDIR", tmp)], json!({"transcript_path": plain}), &Fake::default().err("terminal.label", "no"));
    assert_eq!((r.code, r.err.as_str()), (0, ""));

    assert_eq!((short_model("claude-fable-5-2"), short_model("Sonnet 5.5"), short_model("gpt-6")), ("fable".to_string(), "sonnet".to_string(), "gpt-6".to_string()));
}

/// A kit folder with the orchestrate command, in `dir`.
fn kit(dir: &Path, body: &str) -> String {
    let k = dir.join("kit");
    fs::create_dir_all(k.join("commands")).unwrap();
    fs::write(k.join("commands/orchestrate.md"), format!("---\ndescription: Make this session the coordinator.\n---\n\nYou coordinate. Goal: $ARGUMENTS\n{body}\n")).unwrap();
    path(&k).to_string()
}

fn workers() -> Value {
    json!({"terminals": [
        {"address": "t4", "name": "b-158", "state": "live", "parent": "t1", "status": {"state": "working"}, "labels": {"contr": "agent", "role": "builder", "issue": "158", "worktree": "/w/a"}},
        {"address": "t5", "name": "p-9", "state": "live", "parent": "t1", "labels": {"contr": "agent", "role": "planner"}},
        {"address": "t6", "name": "other", "state": "live", "parent": "t9", "labels": {"contr": "agent", "role": "builder"}},
    ]})
}

#[test]
fn session_start_after_a_compaction_gives_the_coordinator_its_state() {
    let dir = tempfile::tempdir().unwrap();
    let (tmp, own, kit) = (path(dir.path()), path(&dir.path().join("owner.json")).to_string(), kit(dir.path(), "- A rule."));
    let store = Store::new(&own);
    store.ask(Question { coordinator: "t1".into(), title: "Merge order".into(), question: "Which first?".into(), ..Question::default() }).unwrap();
    store.ask(Question { coordinator: "t1".into(), question: "A long\nquestion?".into(), ..Question::default() }).unwrap();
    store.ask(Question { coordinator: "t9".into(), question: "Not mine".into(), ..Question::default() }).unwrap();
    store.pin(Todo { coordinator: "t1".into(), text: "Test the build".into(), ..Todo::default() }).unwrap();
    let vars = [("HC_TERMINAL_ID", "t1"), ("TMPDIR", tmp), ("CONTR_OWNER_FILE", own.as_str()), ("CLAUDE_PLUGIN_ROOT", kit.as_str()), ("CONTR_AGENTS_CONFIG", "/none/agents.json")];
    let core = Fake::default().reply("terminal.list", workers());
    let input = json!({"session_id": "s1", "source": "compact", "cwd": "/w/main", "transcript_path": transcript(dir.path(), &[PROMPT, ORCHESTRATE])});
    let r = hook("session-start", &vars, input, &core);
    assert_eq!((r.code, r.err.as_str()), (0, ""));
    let want_head = format!(
        "{CHAT_SKILL_LINE}contr: the conversation was compacted. You are the coordinator, and the playbook follows again.\n\
         First read /w/main/COORDINATOR-STATE.md: you wrote it before the compaction.\n\n\
         Your live workers:\n- b-158 (t4) builder, working, issue 158, /w/a\n- p-9 (t5) planner, no status\n\n\
         Your open owner items:\n- D1 Merge order\n- D2 A long question?\n- T1 [do] Test the build\n\n"
    );
    assert!(r.out.starts_with(&want_head), "{}", r.out);
    let rest = &r.out[want_head.len()..];
    assert!(rest.starts_with("The flow") && rest.ends_with("\nThe playbook:\n\nYou coordinate. Goal: (see the state file)\n- A rule."), "{rest}");
    // The size from before the compaction is removed.
    assert_eq!(labels(&core), [json!({"terminal": "t1", "labels": {"contr.context": ""}})]);
}

#[test]
fn session_start_says_what_it_could_not_read_and_cuts_a_long_text() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = path(dir.path());
    fs::write(dir.path().join("owner.json"), "{not json").unwrap();
    let own = path(&dir.path().join("owner.json")).to_string();
    let vars = [("HC_TERMINAL_ID", "t1"), ("TMPDIR", tmp), ("CONTR_OWNER_FILE", own.as_str())];
    let core = terminal("orchestrator").err("terminal.list", "hc is down");
    let r = hook("session-start", &vars, json!({"source": "compact"}), &core);
    assert_eq!(r.code, 0);
    assert!(r.out.contains("Your live workers: unknown (hc is down); use the list tool.\n"), "{}", r.out);
    assert!(r.out.contains("Open owner items: unknown ("), "{}", r.out);
    assert!(r.out.ends_with("The playbook could not be read (no kit folder). Run /contr:orchestrate with your goal to read it again."), "{}", r.out);

    let long = kit(dir.path(), &"x".repeat(RESTORE_MAX));
    let vars = [("HC_TERMINAL_ID", "t1"), ("TMPDIR", tmp), ("CONTR_KIT", long.as_str()), ("CONTR_OWNER_FILE", own.as_str())];
    let r = hook("session-start", &vars, json!({"source": "compact"}), &terminal("orchestrator"));
    let text = r.out.strip_prefix(CHAT_SKILL_LINE).unwrap();
    assert_eq!(text.chars().count(), RESTORE_MAX);
    assert!(text.ends_with("x…") && text.contains("Your live workers:\n- none\n"), "{text}");
}

#[test]
fn session_start_is_quiet_for_other_sessions_and_other_sources() {
    let dir = tempfile::tempdir().unwrap();
    let tmp = path(dir.path());
    let vars = [("HC_TERMINAL_ID", "t1"), ("TMPDIR", tmp)];
    let orch = transcript(dir.path(), &[PROMPT, ORCHESTRATE]);
    // After resume and clear, a coordinator gets only the line of the chat skill.
    for source in ["resume", "clear"] {
        let core = Fake::default();
        let r = hook("session-start", &vars, json!({"source": source, "transcript_path": orch}), &core);
        assert_eq!((r.code, r.out.as_str()), (0, CHAT_SKILL_LINE), "{source}");
        assert!(core.calls().is_empty(), "{source}");
    }
    // A session that is no coordinator.
    for source in ["compact", "startup"] {
        let r = hook("session-start", &vars, json!({"source": source}), &terminal("builder"));
        assert_eq!((r.code, r.out.as_str()), (0, CHAT_SKILL_LINE), "{source}");
    }
    // A worker outside an hc terminal gets no line, and outside hc nothing runs.
    assert_eq!(hook("session-start", &[("CONTR_COORDINATOR", "t1")], json!({"source": "startup"}), &Fake::default()).out, "");
    assert_eq!(call_with(&["hook", "session-start"], &[], &Fake::default(), "not json").code, 0);
    let r = hook("session-start", &vars, json!({"source": "startup"}), &Fake::default().err("terminal.get", "hc is down"));
    assert_eq!((r.code, r.err.as_str()), (1, "herdr-contr-orch hook: hc is down\n"));
}

#[test]
fn session_start_at_startup_gives_an_orchestrator_terminal_the_playbook() {
    let dir = tempfile::tempdir().unwrap();
    let (tmp, cwd, kit) = (path(dir.path()), path(dir.path()), kit(dir.path(), "- A rule."));
    let own = path(&dir.path().join("owner.json")).to_string();
    let vars = [("HC_TERMINAL_ID", "t1"), ("TMPDIR", tmp), ("CONTR_OWNER_FILE", own.as_str()), ("CONTR_KIT", kit.as_str())];
    let core = terminal("orchestrator");
    let r = hook("session-start", &vars, json!({"session_id": "s1", "source": "startup", "cwd": cwd}), &core);
    let want = format!("{CHAT_SKILL_LINE}contr: you are the coordinator of this project; the owner gives the goal in the next message.\n\nYour live workers:\n- none\n\nYour open owner items:\n- none\n\nThe flow");
    assert!(r.code == 0 && r.out.starts_with(&want) && r.out.ends_with("- A rule."), "{}", r.out);
    // A new session is not labeled.
    assert!(labels(&core).is_empty());
    // With a state file in the folder, the text names it.
    fs::write(dir.path().join("COORDINATOR-STATE.md"), "x").unwrap();
    let r = hook("session-start", &vars, json!({"source": "startup", "cwd": cwd}), &core);
    assert!(r.out.contains(&format!("First read {cwd}/COORDINATOR-STATE.md: an earlier coordinator of this project wrote it.\n\nYour live workers:")), "{}", r.out);
}

#[test]
fn codex_receives_the_playbook_workers_and_owner_items_in_session_context() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = path(dir.path());
    let kit = kit(dir.path(), "");
    fs::write(Path::new(&kit).join("commands/orchestrate.md"), include_str!("../../../kit/commands/orchestrate.md")).unwrap();
    let owner = dir.path().join("owner.json");
    let vars = [("CONTR_CODEX_COORDINATOR", "1"), ("HC_TERMINAL_ID", "t1"), ("CONTR_KIT", kit.as_str()), ("CONTR_HOME", cwd)];
    Store::new(&owner).pin(Todo { coordinator: "t1".into(), text: "Test the build".into(), ..Todo::default() }).unwrap();
    fs::write(dir.path().join("COORDINATOR-STATE.md"), "Current plan").unwrap();
    let core = Fake::default().reply("terminal.list", workers());
    for source in ["startup", "compact", "resume"] {
        let input = json!({"session_id": "s1", "source": source, "cwd": cwd, "transcript_path": null});
        let r = hook("codex-session-start", &vars, input, &core);
        assert_eq!((r.code, r.err.as_str()), (0, ""));
        let data: Value = serde_json::from_str(&r.out).unwrap();
        assert_eq!(data["hookSpecificOutput"]["hookEventName"], "SessionStart");
        let context = data["hookSpecificOutput"]["additionalContext"].as_str().unwrap();
        for item in ["COORDINATOR-STATE.md", "skills/chat-writing/SKILL.md", "b-158", "Test the build", "open the worker's screenshot with the image viewer"] {
            assert!(context.contains(item), "missing {item} after {source}: {context}");
        }
        assert!(!context.contains("$ARGUMENTS") && !context.contains("- other"));
        assert_eq!(context.contains("conversation was compacted"), source == "compact");
    }
    assert_eq!(labels(&core), vec![json!({"terminal": "t1", "labels": {"contr.orchestrating": "1"}}); 3]);
    fs::remove_file(dir.path().join("COORDINATOR-STATE.md")).unwrap();
    let r = hook("codex-session-start", &vars, json!({"source": "compact", "cwd": cwd}), &core);
    assert!(r.out.contains("b-158") && !r.out.contains("First read"));
}

#[test]
fn codex_stop_continues_once_and_prompt_delivers_later_reports() {
    use orch::held::Report;

    let dir = tempfile::tempdir().unwrap();
    let vars = [("CONTR_CODEX_COORDINATOR", "1"), ("HC_TERMINAL_ID", "t1"), ("CONTR_HOME", path(dir.path()))];
    let env = |k: &str| vars.iter().find(|(key, _)| *key == k).map_or(String::new(), |(_, value)| value.to_string());
    let core = Fake::default();
    let queue = |text: &str| orch::held::hold(&env, "t1", &Report { from: "t4 b-158".into(), text: text.into() }).unwrap();
    queue("PR is ready");
    // Ordinary Codex sessions and workers ignore the coordinator hooks.
    let worker = [vars.as_slice(), &[("CONTR_COORDINATOR", "t2")]].concat();
    for ignored in [&vars[1..], &worker] {
        for name in ["codex-stop", "codex-prompt", "codex-session-start"] {
            let r = hook(name, ignored, json!(null), &core);
            assert_eq!((r.code, r.out.as_str(), r.err.as_str()), (0, "", ""));
        }
    }
    let mut stop = json!({"stop_hook_active": false, "transcript_path": null, "last_assistant_message": null});
    let r = hook("codex-stop", &vars, stop.clone(), &core);
    assert_eq!((r.code, r.err.as_str()), (0, ""));
    let data: Value = serde_json::from_str(&r.out).unwrap();
    let reason = data["reason"].as_str().unwrap();
    assert_eq!(data["decision"], "block");
    assert!(reason.contains("PR is ready"));
    assert!(!orch::hook::owner_words(reason));
    queue("CI passed");
    stop["stop_hook_active"] = json!(true);
    assert!(hook("codex-stop", &vars, stop, &core).out.is_empty());
    let r = hook("codex-prompt", &vars, json!({"prompt": "check the PR"}), &core);
    assert_eq!((r.code, r.err.as_str()), (0, ""));
    assert!(r.out.contains("Codex plan tool") && r.out.contains("CI passed"));
    assert!(!r.out.contains("PR is ready") && !r.out.contains("TodoWrite"));
    assert!(hook("codex-prompt", &vars, json!({"prompt": "<hc>CI passed</hc>"}), &core).out.is_empty());

    fs::write(dir.path().join("agents.json"), r#"{"flow":{"workers":2}}"#).unwrap();
    let r = hook("codex-prompt", &vars, json!({"prompt": "go on"}), &core);
    assert!(r.out.contains("the owner changed the flow"));
    assert!(core.calls().is_empty());

    let r = call_with(&["hook", "codex-prompt"], &vars, &core, "{");
    assert_eq!((r.code, r.out.as_str()), (0, ""));
    assert!(r.err.contains("hook prompt"));
}

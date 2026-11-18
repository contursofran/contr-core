//! End-to-end output speed from assistant usage and log timestamps.

use std::collections::HashMap;

use crate::util::{json::Obj, parse_time};
use crate::OutputSpeed;

#[derive(Clone, Debug, Default)]
struct Answer {
    start: Option<f64>,
    row: Option<usize>,
    order: usize,
    speed: Option<OutputSpeed>,
}

impl Answer {
    fn finish(&mut self, end: Option<f64>, output_tokens: u64, order: usize) {
        // The log parser uses millisecond timestamps. Recover that precision after subtraction.
        let seconds = self
            .start
            .zip(end)
            .map(|(s, e)| ((e - s) * 1000.0).round() / 1000.0)
            .filter(|s| *s >= 0.0);
        let tps = seconds
            .filter(|s| *s >= 1.0 && output_tokens >= 50)
            .map(|s| output_tokens as f64 / s);
        self.order = order;
        self.speed = Some(OutputSpeed {
            output_tokens,
            seconds,
            tps,
        });
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct SpeedLog {
    claude: HashMap<String, Answer>,
    claude_before: Option<f64>,
    codex: Vec<Answer>,
    codex_next: Answer,
    codex_total: Option<u64>,
    order: usize,
}

impl SpeedLog {
    pub(super) fn feed(&mut self, o: &Obj, at: Option<f64>, row: Option<usize>) {
        self.order += 1;
        if let Some(p) = o.get("payload") {
            match (
                o.get("type").and_then(|v| v.as_str()),
                p["type"].as_str(),
                p["role"].as_str(),
            ) {
                (Some("event_msg"), Some("token_count"), _) => {
                    let info = &p["info"];
                    let Some(tokens) = info["last_token_usage"]["output_tokens"].as_u64() else {
                        return;
                    };
                    let total = info["total_token_usage"]["total_tokens"].as_u64();
                    // Rate limit events can repeat the last model call's usage.
                    if total.is_some() && total == self.codex_total {
                        return;
                    }
                    self.codex_total = total;
                    let mut answer = std::mem::replace(
                        &mut self.codex_next,
                        Answer {
                            start: at,
                            ..Answer::default()
                        },
                    );
                    answer.finish(at, tokens, self.order);
                    self.codex.push(answer);
                }
                (Some("event_msg"), Some("task_started" | "user_message"), _)
                | (Some("response_item"), Some("message"), Some("user"))
                | (
                    Some("response_item"),
                    Some(
                        "function_call_output"
                        | "custom_tool_call_output"
                        | "local_shell_call_output",
                    ),
                    _,
                ) => {
                    self.codex_next = Answer {
                        start: at,
                        ..Answer::default()
                    };
                }
                (Some("event_msg"), Some("task_complete" | "turn_aborted"), _) => {
                    self.codex_next = Answer::default()
                }
                (Some("response_item"), Some("message"), Some("assistant")) if row.is_some() => {
                    self.codex_next.row = row
                }
                _ => {}
            }
            return;
        }

        if o.get("isSidechain").and_then(|v| v.as_bool()) == Some(true) {
            return;
        }
        let Some(msg) = o.get("message") else { return };
        match o.get("type").and_then(|v| v.as_str()) {
            Some("user") if o.get("isMeta").and_then(|v| v.as_bool()) != Some(true) => {
                self.claude_before = at
            }
            Some("assistant") if msg["model"] != "<synthetic>" => {
                let before = std::mem::replace(&mut self.claude_before, at);
                let Some(id) = msg["id"].as_str() else { return };
                let answer = self.claude.entry(id.into()).or_insert_with(|| Answer {
                    start: before,
                    ..Answer::default()
                });
                answer.row = row.or(answer.row);
                let tokens = msg["usage"]["output_tokens"]
                    .as_u64()
                    .or(answer.speed.map(|s| s.output_tokens));
                if let Some(tokens) = tokens {
                    answer.finish(at, tokens, self.order);
                }
            }
            _ => {}
        }
    }

    pub(super) fn rows(&self) -> impl Iterator<Item = (usize, OutputSpeed)> + '_ {
        self.answers().filter_map(|a| Some((a.row?, a.speed?)))
    }

    fn answers(&self) -> impl Iterator<Item = &Answer> {
        self.claude.values().chain(&self.codex)
    }
}

/// Up to five speeds in answer order, most recent first.
pub fn recent_output_speeds(lines: impl IntoIterator<Item = impl AsRef<str>>) -> Vec<f64> {
    let mut log = SpeedLog::default();
    for line in lines {
        if let Some(o) = crate::util::json::parse_obj(line.as_ref()) {
            let at = o.get("timestamp").and_then(|v| parse_time(v.as_str()?));
            log.feed(&o, at, None);
        }
    }
    let mut eligible: Vec<_> = log
        .answers()
        .filter_map(|a| Some((a.order, a.speed?.tps?)))
        .collect();
    eligible.sort_unstable_by_key(|&(order, _)| order);
    eligible.iter().rev().take(5).map(|&(_, tps)| tps).collect()
}

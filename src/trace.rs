//! Bounded per-thread diagnostics and opt-in CLI progress, separate from final results.
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::{cell::RefCell, collections::VecDeque, io::Write};

const MAX_TRACE_EVENTS: usize = 200;
const MAX_TRACE_BYTES: usize = 512 * 1024;
const MAX_STREAM_EVENTS: usize = 512;
const MAX_STREAM_BYTES: usize = 2 * 1024 * 1024;
const MAX_EVENT_BYTES: usize = 16 * 1024;
const MAX_TEXT_BYTES: usize = 8 * 1024;
const RESERVED_TERMINAL_EVENTS: usize = 8;
const MAX_VALUE_NODES: usize = 64;
const MAX_VALUE_DEPTH: usize = 8;

/// Copy at most `limit` bytes without splitting a UTF-8 character.
pub fn bounded_text(text: &str, limit: usize) -> String {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

fn text_limit(field: &str) -> usize {
    match field {
        "stage" | "state" | "method" | "protocol" => 128,
        "id" | "taskId" | "contextId" | "runId" => 256,
        "origin" | "endpoint" | "url" | "reason" => 2048,
        _ => MAX_TEXT_BYTES,
    }
}

fn sanitize(value: Value, field: &str, depth: usize, nodes: &mut usize, cut: &mut bool) -> Value {
    if *nodes == 0 || depth > MAX_VALUE_DEPTH {
        *cut = true;
        return Value::Null;
    }
    *nodes -= 1;
    match value {
        Value::String(text) => {
            let bounded = bounded_text(&text, text_limit(field));
            *cut |= bounded.len() != text.len();
            Value::String(bounded)
        }
        Value::Array(values) => {
            let mut result = Vec::new();
            for value in values {
                if *nodes == 0 {
                    *cut = true;
                    break;
                }
                result.push(sanitize(value, field, depth + 1, nodes, cut));
            }
            Value::Array(result)
        }
        Value::Object(mut fields) => {
            // Keep protocol identity and terminal state even when unrelated fields exhaust nodes.
            let mut first = Vec::new();
            for key in [
                "stage",
                "state",
                "taskId",
                "contextId",
                "method",
                "textTruncated",
                "text",
            ] {
                if let Some(value) = fields.remove(key) {
                    first.push((key.to_owned(), value));
                }
            }
            let mut result = Map::new();
            for (name, value) in first.into_iter().chain(fields) {
                if *nodes == 0 {
                    *cut = true;
                    break;
                }
                let key = bounded_text(&name, 64);
                *cut |= key.len() != name.len();
                let value = sanitize(value, &name, depth + 1, nodes, cut);
                if result.insert(key, value).is_some() {
                    *cut = true;
                }
            }
            Value::Object(result)
        }
        value => value,
    }
}

struct LimitedWriter {
    bytes: Vec<u8>,
}
impl Write for LimitedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.bytes.len() + bytes.len() >= MAX_EVENT_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "progress event limit",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn encode(value: &Value) -> Option<Vec<u8>> {
    #[derive(Serialize)]
    struct Progress<'a> {
        event: &'static str,
        data: &'a Value,
    }
    let mut writer = LimitedWriter { bytes: Vec::new() };
    serde_json::to_writer(
        &mut writer,
        &Progress {
            event: "progress",
            data: value,
        },
    )
    .ok()?;
    writer.bytes.push(b'\n');
    Some(writer.bytes)
}

fn bounded_event(value: Value) -> (Value, Vec<u8>, bool) {
    let mut cut = value["textTruncated"].as_bool().unwrap_or(false);
    let mut nodes = MAX_VALUE_NODES;
    let mut value = sanitize(value, "", 0, &mut nodes, &mut cut);
    if let Some(encoded) = encode(&value) {
        return (value, encoded, cut);
    }
    cut = true;
    // Escaping alone can make an 8 KiB string exceed 16 KiB. Drop excess detail first.
    let mut essential = Map::new();
    for key in [
        "stage",
        "state",
        "taskId",
        "contextId",
        "method",
        "text",
        "reason",
        "origin",
        "protocol",
        "elapsedMs",
        "authenticated",
        "rpcErrorCode",
    ] {
        if let Some(item) = value
            .get(key)
            .filter(|item| !item.is_array() && !item.is_object())
        {
            essential.insert(key.to_owned(), item.clone());
        }
    }
    if !essential.contains_key("stage") {
        essential.insert("stage".into(), json!("trace"));
    }
    essential.insert("eventTruncated".into(), Value::Bool(true));
    value = Value::Object(essential);
    let mut limit = MAX_TEXT_BYTES;
    loop {
        if let Some(encoded) = encode(&value) {
            return (value, encoded, cut);
        }
        limit /= 2;
        if let Some(fields) = value.as_object_mut() {
            for (key, item) in fields {
                if !matches!(
                    key.as_str(),
                    "stage" | "state" | "taskId" | "contextId" | "method"
                ) && let Value::String(text) = item
                {
                    *text = bounded_text(text, limit);
                }
            }
        }
    }
}

fn terminal(value: &Value) -> bool {
    value["stage"] == "transport_error"
        || (value["stage"] == "task"
            && matches!(
                value["state"].as_str(),
                Some(
                    "completed"
                        | "canceled"
                        | "failed"
                        | "rejected"
                        | "input-required"
                        | "auth-required"
                )
            ))
}

fn marker() -> Value {
    json!({"stage":"progress_truncated","message":"Further progress omitted after local output limits."})
}

#[derive(Default)]
struct StreamBudget {
    events: usize,
    bytes: usize,
    suppressed: bool,
    marker_sent: bool,
}
struct Decision {
    accepted: bool,
    marker: bool,
    omitted: bool,
}
impl StreamBudget {
    fn decide(&mut self, bytes: usize, terminal: bool, marker_bytes: usize) -> Decision {
        let reserve_marker = usize::from(!self.marker_sent);
        let accepted = if terminal {
            self.events + 1 + reserve_marker <= MAX_STREAM_EVENTS
                && self.bytes + bytes + reserve_marker * marker_bytes <= MAX_STREAM_BYTES
        } else {
            !self.suppressed
                && self.events + 1 + RESERVED_TERMINAL_EVENTS < MAX_STREAM_EVENTS
                && self.bytes + bytes + (RESERVED_TERMINAL_EVENTS + 1) * MAX_EVENT_BYTES
                    <= MAX_STREAM_BYTES
        };
        if accepted {
            self.events += 1;
            self.bytes += bytes;
            return Decision {
                accepted: true,
                marker: false,
                omitted: false,
            };
        }
        self.suppressed = true;
        let emit_marker = !self.marker_sent
            && self.events < MAX_STREAM_EVENTS
            && self.bytes + marker_bytes <= MAX_STREAM_BYTES;
        if emit_marker {
            self.marker_sent = true;
            self.events += 1;
            self.bytes += marker_bytes;
        }
        Decision {
            accepted: false,
            marker: emit_marker,
            omitted: true,
        }
    }
}

struct TraceEntry {
    value: Value,
    bytes: usize,
    terminal: bool,
}
#[derive(Default)]
struct TraceState {
    entries: VecDeque<TraceEntry>,
    trace_bytes: usize,
    stream: bool,
    budget: StreamBudget,
    truncated: bool,
}
impl TraceState {
    fn retain(&mut self, value: Value, bytes: usize) {
        let terminal = terminal(&value);
        self.entries.push_back(TraceEntry {
            value,
            bytes,
            terminal,
        });
        self.trace_bytes += bytes;
        while self.entries.len() > MAX_TRACE_EVENTS || self.trace_bytes > MAX_TRACE_BYTES {
            // Terminal diagnostics survive ordinary polling chatter; old terminal-only history is bounded too.
            let index = self
                .entries
                .iter()
                .position(|entry| !entry.terminal)
                .unwrap_or(0);
            if let Some(entry) = self.entries.remove(index) {
                self.trace_bytes -= entry.bytes;
            }
            self.truncated = true;
        }
    }

    // Return only bounded encoded lines. Tests exercise this without touching real stdout.
    fn record(&mut self, value: Value) -> Vec<Vec<u8>> {
        let (value, encoded, cut) = bounded_event(value);
        self.truncated |= cut;
        let is_terminal = terminal(&value);
        self.retain(value, encoded.len());
        if !self.stream {
            return Vec::new();
        }
        let marker = marker();
        let marker_encoded = encode(&marker).expect("fixed progress marker fits the event limit");
        let decision = self
            .budget
            .decide(encoded.len(), is_terminal, marker_encoded.len());
        self.truncated |= decision.omitted;
        if decision.marker {
            self.retain(marker, marker_encoded.len());
            return vec![marker_encoded];
        }
        if decision.accepted {
            vec![encoded]
        } else {
            Vec::new()
        }
    }

    fn take(&mut self) -> Vec<Value> {
        self.trace_bytes = 0;
        self.entries.drain(..).map(|entry| entry.value).collect()
    }
    fn reset(&mut self) {
        let stream = self.stream;
        *self = Self {
            stream,
            ..Self::default()
        };
    }
}

thread_local! { static TRACE: RefCell<TraceState> = RefCell::new(TraceState::default()); }

pub fn event(value: Value) {
    let lines = TRACE.with(|trace| trace.borrow_mut().record(value));
    if !lines.is_empty() {
        let mut stdout = std::io::stdout().lock();
        for line in lines {
            let _ = stdout.write_all(&line);
        }
        let _ = stdout.flush();
    }
}
pub fn take() -> Vec<Value> {
    TRACE.with(|trace| trace.borrow_mut().take())
}
pub fn enable_stream() {
    TRACE.with(|trace| trace.borrow_mut().stream = true);
}
pub fn reset() {
    TRACE.with(|trace| trace.borrow_mut().reset());
}
pub fn truncated() -> bool {
    TRACE.with(|trace| trace.borrow().truncated)
}
pub fn mark_truncated() {
    TRACE.with(|trace| trace.borrow_mut().truncated = true);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_text_cuts_only_at_character_boundaries() {
        assert_eq!(bounded_text("a中文🙂z", 7), "a中文");
        assert_eq!(bounded_text("🙂", 3), "");
        assert_eq!(bounded_text("中文", 0), "");
        let (value, encoded, cut) = bounded_event(
            json!({"stage":"remote_status","taskId":"i".repeat(1000),"text":"中".repeat(4 * 1024 * 1024 / 3)}),
        );
        assert!(cut);
        assert!(value["text"].as_str().unwrap().len() <= MAX_TEXT_BYTES);
        assert_eq!(value["taskId"].as_str().unwrap().len(), 256);
        assert!(encoded.len() <= MAX_EVENT_BYTES);
    }

    #[test]
    fn escaped_large_text_is_limited_in_actual_json_bytes() {
        let (value, encoded, cut) = bounded_event(
            json!({"stage":"remote_status","text":"\u{0001}".repeat(4 * 1024 * 1024)}),
        );
        assert!(cut);
        assert!(value["text"].as_str().unwrap().len() <= MAX_TEXT_BYTES);
        assert!(encoded.len() <= MAX_EVENT_BYTES);
        assert_eq!(
            serde_json::from_slice::<Value>(&encoded).unwrap()["event"],
            "progress"
        );
    }

    #[test]
    fn count_cap_emits_one_marker_and_keeps_terminal_reserve() {
        let mut state = TraceState {
            stream: true,
            ..TraceState::default()
        };
        let mut lines = Vec::new();
        for index in 0..1000 {
            lines.extend(state.record(json!({"stage":"response","elapsedMs":index})));
        }
        let before = lines.len();
        lines.extend(state.record(json!({"stage":"task","taskId":"t","state":"completed"})));
        lines
            .extend(state.record(json!({"stage":"transport_error","reason":"bounded diagnostic"})));
        assert_eq!(lines.len(), before + 2);
        assert_eq!(
            lines
                .iter()
                .filter(
                    |line| serde_json::from_slice::<Value>(line).unwrap()["data"]["stage"]
                        == "progress_truncated"
                )
                .count(),
            1
        );
        assert!(lines.len() <= MAX_STREAM_EVENTS);
        assert!(lines.iter().map(Vec::len).sum::<usize>() <= MAX_STREAM_BYTES);
        assert!(state.truncated);
    }

    #[test]
    fn byte_cap_and_terminal_only_stream_both_remain_bounded() {
        for is_terminal in [false, true] {
            let mut state = TraceState {
                stream: true,
                ..TraceState::default()
            };
            let mut events = 0;
            let mut bytes = 0;
            let mut markers = 0;
            for _ in 0..1000 {
                let value = if is_terminal {
                    json!({"stage":"task","taskId":"t","state":"failed","text":"x".repeat(MAX_TEXT_BYTES)})
                } else {
                    json!({"stage":"remote_status","text":"x".repeat(MAX_TEXT_BYTES)})
                };
                for line in state.record(value) {
                    events += 1;
                    bytes += line.len();
                    markers += usize::from(
                        serde_json::from_slice::<Value>(&line).unwrap()["data"]["stage"]
                            == "progress_truncated",
                    );
                }
            }
            assert!(events <= MAX_STREAM_EVENTS);
            assert!(bytes <= MAX_STREAM_BYTES);
            assert_eq!(markers, 1);
            assert_eq!(events, state.budget.events);
            assert_eq!(bytes, state.budget.bytes);
        }
    }

    #[test]
    fn trace_ring_tracks_count_bytes_and_retains_tail_and_terminal() {
        let mut state = TraceState::default();
        state.record(json!({"stage":"task","taskId":"t","state":"completed"}));
        for index in 0..1000 {
            assert!(state.record(json!({"stage":"remote_status","index":index,"text":"x".repeat(MAX_TEXT_BYTES)})).is_empty());
            assert!(state.entries.len() <= MAX_TRACE_EVENTS);
            assert!(state.trace_bytes <= MAX_TRACE_BYTES);
        }
        let trace = state.take();
        assert!(trace.iter().any(|value| value["state"] == "completed"));
        assert_eq!(trace.last().unwrap()["index"], 999);
        assert_eq!(state.trace_bytes, 0);
        assert!(state.truncated);
        for index in 0..1000 {
            state.record(json!({"stage":"task","state":"completed","index":index}));
        }
        assert_eq!(state.entries.len(), MAX_TRACE_EVENTS);
        assert_eq!(state.entries.back().unwrap().value["index"], 999);
    }

    #[test]
    fn all_terminal_states_are_preserved_over_normal_chatter() {
        let mut state = TraceState::default();
        for status in [
            "completed",
            "canceled",
            "failed",
            "rejected",
            "input-required",
            "auth-required",
        ] {
            state.record(json!({"stage":"task","taskId":"t","state":status}));
        }
        state.record(json!({"stage":"transport_error","reason":"connection failed"}));
        for _ in 0..1000 {
            state.record(json!({"stage":"response"}));
        }
        assert_eq!(
            state.entries.iter().filter(|entry| entry.terminal).count(),
            7
        );
        assert!(state.entries.len() <= MAX_TRACE_EVENTS);
    }

    #[test]
    fn nested_remote_strings_and_event_trees_are_bounded() {
        let values = (0..1000)
            .map(|_| json!({"value":"x".repeat(MAX_TEXT_BYTES + 1)}))
            .collect::<Vec<_>>();
        let (_, encoded, cut) =
            bounded_event(json!({"stage":"task","state":"completed","detail":values}));
        assert!(cut);
        assert!(encoded.len() <= MAX_EVENT_BYTES);
        let output: Value = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(output["data"]["state"], "completed");
    }

    #[test]
    fn reset_clears_all_budgets_and_flags_but_keeps_stream_enabled() {
        let mut state = TraceState {
            stream: true,
            ..TraceState::default()
        };
        state.record(json!({"stage":"remote_status","textTruncated":true,"text":"short"}));
        assert!(state.truncated);
        state.reset();
        assert!(state.stream);
        assert!(!state.truncated);
        assert_eq!(state.budget.events, 0);
        assert_eq!(state.budget.bytes, 0);
        assert!(state.entries.is_empty());
        assert_eq!(
            state
                .record(json!({"stage":"task","state":"completed"}))
                .len(),
            1
        );
    }
}

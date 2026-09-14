use reqwest::{Url, blocking::Client, redirect::Policy};
use serde::{Deserialize, Serialize};

pub mod auth;
pub mod catalog;
pub mod install;
pub mod service;
use serde_json::{Value, json};
use std::{
    io::Read,
    time::{Duration, Instant},
};

pub type Result<T> = std::result::Result<T, String>;
thread_local! { static REQUEST_TRACE: std::cell::RefCell<Vec<Value>> = const { std::cell::RefCell::new(Vec::new()) }; }
pub fn take_request_trace() -> Vec<Value> {
    REQUEST_TRACE.with(|t| std::mem::take(&mut *t.borrow_mut()))
}
thread_local! { static STREAM_TRACE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
/// Opt-in CLI event stream; never enabled by the desktop service.
pub fn enable_trace_stream() { STREAM_TRACE.with(|enabled| enabled.set(true)); }
fn trace(value: Value) {
    STREAM_TRACE.with(|enabled| {
        if enabled.get() {
            use std::io::Write;
            let mut stdout = std::io::stdout().lock();
            let _ = writeln!(stdout, "{}", json!({"event":"progress","data":value}));
            let _ = stdout.flush();
        }
    });
    REQUEST_TRACE.with(|t| {
        let mut t = t.borrow_mut();
        if t.len() < 200 {
            t.push(value);
        }
    });
}
const MAX_RESPONSE: u64 = 4 * 1024 * 1024;

#[derive(Debug, Serialize)]
pub struct Outcome {
    /// Original final A2A Message or Task, without flattening artifacts or parts.
    pub raw: Value,
    pub text: String,
    pub task_id: Option<String>,
    pub context_id: Option<String>,
    pub state: String,
}

/// Normalized, supported JSON-RPC interface. `version` is the protocol version,
/// not the agent software version. The caller retains the original card unchanged.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardInfo {
    pub name: String,
    pub description: String,
    pub skills: Value,
    pub version: String,
    pub endpoint: String,
    pub binding: String,
}

pub fn validate_configuration(version: &str, value: Value) -> Result<Value> {
    let mut fields = value
        .as_object()
        .cloned()
        .ok_or("Request configuration must be an object")?;
    let mode = if version == "1.0" {
        "returnImmediately"
    } else {
        "blocking"
    };
    for (key, value) in &fields {
        let valid = match key.as_str() {
            k if k == mode => value.is_boolean(),
            "historyLength" => value.as_u64().is_some_and(|n| n <= 1000),
            "acceptedOutputModes" => value.as_array().is_some_and(|v| {
                !v.is_empty()
                    && v.iter()
                        .all(|x| x.as_str().is_some_and(|s| !s.trim().is_empty()))
            }),
            _ => false,
        };
        if !valid {
            return Err(format!(
                "Unsupported or invalid {version} request field: {key}"
            ));
        }
    }
    fields.entry(mode).or_insert(json!(version == "1.0"));
    Ok(Value::Object(fields))
}

fn required_string<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("Agent Card requires nonempty {field}"))
}

fn string_array(value: &Value, field: &str) -> Result<()> {
    if !value
        .get(field)
        .and_then(Value::as_array)
        .is_some_and(|items| items.iter().all(Value::is_string))
    {
        return Err(format!("Agent Card requires a string array for {field}"));
    }
    Ok(())
}

// Source: https://github.com/a2aproject/A2A/blob/v1.0.0/specification/a2a.proto
// and docs/specification.md sections 5.5 and 9.4. Interface order is preference order.
fn select_interface(card: &Value) -> Result<(String, String, Option<String>)> {
    if let Some(interfaces) = card.get("supportedInterfaces") {
        let interfaces = interfaces
            .as_array()
            .filter(|i| !i.is_empty())
            .ok_or("Agent Card requires nonempty supportedInterfaces")?;
        let mut selected = None;
        for interface in interfaces {
            let binding = required_string(interface, "protocolBinding")?;
            let version = required_string(interface, "protocolVersion")?;
            let endpoint = required_string(interface, "url")?;
            validate_url(endpoint)?;
            let tenant = match interface.get("tenant") {
                None => None,
                Some(Value::String(s)) => Some(s.clone()),
                _ => return Err("Invalid interface tenant".into()),
            };
            if selected.is_none()
                && binding == "JSONRPC"
                && matches!(version, "1.0" | "0.3" | "0.3.0")
            {
                selected = Some((version.to_owned(), endpoint.to_owned(), tenant));
            }
        }
        return selected.ok_or("No supported JSONRPC interface (A2A 1.0 or 0.3.0)".into());
    }
    if required_string(card, "protocolVersion")? != "0.3.0" {
        return Err("Supported protocols are A2A 1.0 (supportedInterfaces) and 0.3.0".into());
    }
    let transport = match card.get("preferredTransport") {
        None => "JSONRPC",
        Some(_) => required_string(card, "preferredTransport")?,
    };
    let primary = required_string(card, "url")?;
    validate_url(primary)?;
    let mut selected = (transport == "JSONRPC").then(|| primary.to_owned());
    if let Some(interfaces) = card.get("additionalInterfaces") {
        for interface in interfaces
            .as_array()
            .ok_or("Invalid additionalInterfaces")?
        {
            let binding = required_string(interface, "transport")?;
            let endpoint = required_string(interface, "url")?;
            validate_url(endpoint)?;
            if selected.is_none() && binding == "JSONRPC" {
                selected = Some(endpoint.to_owned());
            }
        }
    }
    Ok((
        "0.3.0".into(),
        selected.ok_or("Agent does not advertise a JSONRPC endpoint")?,
        None,
    ))
}

/// Validate required card metadata and select the first supported JSON-RPC interface.
/// Unknown fields, including skill extensions, remain intact in the caller's raw card.
pub fn inspect_card(card: &Value) -> Result<CardInfo> {
    let name = required_string(card, "name")?.to_owned();
    let description = required_string(card, "description")?.to_owned();
    required_string(card, "version")?;
    if !card.get("capabilities").is_some_and(Value::is_object) {
        return Err("Agent Card requires capabilities object".into());
    }
    string_array(card, "defaultInputModes")?;
    string_array(card, "defaultOutputModes")?;
    let skills = card
        .get("skills")
        .and_then(Value::as_array)
        .ok_or("Agent Card requires skills array")?;
    for skill in skills {
        for field in ["id", "name", "description"] {
            required_string(skill, field)?;
        }
        string_array(skill, "tags")?;
    }
    let (version, endpoint, _) = select_interface(card)?;
    Ok(CardInfo {
        name,
        description,
        skills: card["skills"].clone(),
        version,
        endpoint,
        binding: "JSONRPC".into(),
    })
}

pub struct A2aClient {
    http: Client,
    endpoint: Url,
    token: Option<String>,
    deadline: Instant,
    sequence: u64,
    version: String,
    tenant: Option<String>,
}

fn validate_url(raw: &str) -> Result<Url> {
    let url = Url::parse(raw).map_err(|_| "Invalid URL".to_string())?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host_str().is_none()
        || url.fragment().is_some()
    {
        return Err(
            "Only absolute HTTP(S) URLs without embedded credentials or fragments are supported"
                .into(),
        );
    }
    Ok(url)
}

fn read_json(response: reqwest::blocking::Response) -> Result<Value> {
    let status = response.status();
    if !status.is_success() {
        return Err(format!("HTTP error {status}"));
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_RESPONSE + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Response read failed")?;
    if bytes.len() as u64 > MAX_RESPONSE {
        return Err("Response exceeds 4 MiB limit".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "Invalid JSON response".into())
}

impl A2aClient {
    pub fn connect(card_url: &str, token: Option<String>, timeout: Duration) -> Result<Self> {
        Self::connect_authenticated(card_url, token, timeout, &auth::Auth::default())
    }
    pub fn connect_authenticated(
        card_url: &str,
        token: Option<String>,
        timeout: Duration,
        auth: &auth::Auth,
    ) -> Result<Self> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or("Timeout too large")?;
        let card_url = validate_url(card_url)?;
        let http = Client::builder()
            .default_headers(auth.headers()?)
            .redirect(Policy::none())
            .build()
            .map_err(|_| "HTTP client initialization failed")?;
        let mut request = http
            .get(card_url.clone())
            .timeout(timeout.min(Duration::from_secs(30)));
        if let Some(token) = &token {
            request = request.bearer_auth(token);
        }
        let card = read_json(request.send().map_err(|_| "Agent Card request failed")?)?;
        let info = inspect_card(&card)?;
        let endpoint = validate_url(&info.endpoint)?;
        let (_, _, tenant) = select_interface(&card)?;
        // Explicitly reject cross-origin cards: do not forward credentials or tasks to an unapproved host.
        if endpoint.origin() != card_url.origin() {
            return Err("Cross-origin Agent Card endpoint rejected".into());
        }
        Ok(Self {
            http,
            endpoint,
            token,
            deadline,
            sequence: 0,
            version: info.version,
            tenant,
        })
    }

    /// Connect without fetching a card. The caller must explicitly approve the
    /// advertised URL before supplying a cached/local card; no origin is inferred.
    /// Redirects remain disabled, including for authenticated requests.
    pub fn connect_card(card: &Value, token: Option<String>, timeout: Duration) -> Result<Self> {
        Self::connect_card_authenticated(card, token, timeout, &auth::Auth::default())
    }
    pub fn connect_card_authenticated(
        card: &Value,
        token: Option<String>,
        timeout: Duration,
        auth: &auth::Auth,
    ) -> Result<Self> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or("Timeout too large")?;
        let info = inspect_card(card)?;
        let (_, _, tenant) = select_interface(card)?;
        Ok(Self {
            http: Client::builder()
                .default_headers(auth.headers()?)
                .redirect(Policy::none())
                .build()
                .map_err(|_| "HTTP client initialization failed")?,
            endpoint: validate_url(&info.endpoint)?,
            token,
            deadline,
            sequence: 0,
            version: info.version,
            tenant,
        })
    }

    fn rpc(&mut self, method: &str, mut params: Value) -> Result<Value> {
        let started = Instant::now();
        trace(
            json!({"stage":"request", "method":method, "origin":self.endpoint.origin().ascii_serialization(), "protocol":self.version, "parameterKeys":params.as_object().map(|p| p.keys().collect::<Vec<_>>()), "authenticated":self.token.is_some()}),
        );
        if self.version == "1.0" {
            if let Some(tenant) = &self.tenant {
                params["tenant"] = json!(tenant);
            }
        }
        let remaining = self
            .deadline
            .checked_duration_since(Instant::now())
            .ok_or("Task deadline exceeded; remote task may still be running")?;
        self.sequence += 1;
        let mut request = self
            .http
            .post(self.endpoint.clone())
            .timeout(remaining.min(Duration::from_secs(30)))
            .json(&json!({"jsonrpc":"2.0", "id":self.sequence, "method":method, "params":params}));
        if self.version == "1.0" {
            request = request.header("A2A-Version", "1.0");
        }
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        let body = read_json(request.send().map_err(|error| {
            // Never expose reqwest's raw error: it can include credential-bearing URLs.
            let reason = if error.is_timeout() {
                "request timed out (请求超时)"
            } else if error.is_connect() {
                "connection failed (连接失败：检查网络、DNS、代理或 TLS 证书)"
            } else {
                "request transport failed (传输失败：连接中断或 HTTP 协议错误)"
            };
            trace(json!({"stage":"transport_error","method":method,"elapsedMs":started.elapsed().as_millis(),"reason":reason}));
            format!("A2A {method}: {reason}; remote task may still be running; 未自动重试")
        })?)?;
        trace(
            json!({"stage":"response","method":method,"elapsedMs":started.elapsed().as_millis(),"rpcErrorCode":body.get("error").and_then(|e| e.get("code"))}),
        );
        if body["jsonrpc"] != "2.0" || body["id"] != self.sequence {
            return Err("Invalid JSON-RPC envelope or response ID".into());
        }
        if body.get("error").is_some() && body.get("result").is_some() {
            return Err("Invalid JSON-RPC envelope: both result and error".into());
        }
        if let Some(error) = body.get("error") {
            let hint = if error["code"].as_i64() == Some(-32602) {
                "；远端拒绝请求参数"
            } else {
                ""
            };
            let lifecycle = if matches!(method, "GetTask" | "tasks/get") {
                "；消息已提交，但查询任务失败。远端任务可能仍在运行，请勿直接重复提交"
            } else {
                ""
            };
            // Remote error.data can contain prompts or credentials; do not relay it blindly.
            return Err(format!(
                "A2A {method}: JSON-RPC error code {}{hint}{lifecycle}",
                error["code"]
            ));
        }
        body.get("result")
            .cloned()
            .ok_or("Missing JSON-RPC result".into())
    }

    pub fn run(&mut self, message: &str) -> Result<Outcome> {
        self.run_with_configuration(message, json!({}))
    }
    pub fn run_with_configuration(
        &mut self,
        message: &str,
        configuration: Value,
    ) -> Result<Outcome> {
        self.run_cancellable(message, configuration, &std::sync::atomic::AtomicBool::new(false))
    }

    /// Cancellation is cooperative: an in-flight HTTP request must return first.
    /// Only a remote canceled state counts as confirmed cancellation.
    pub fn run_cancellable(
        &mut self,
        message: &str,
        configuration: Value,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<Outcome> {
        let configuration = validate_configuration(&self.version, configuration)?;
        if cancel.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("发送前已停止；未提交远程任务".into());
        }
        let message_id = format!(
            "any-a2a-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| "Clock error")?
                .as_nanos()
        );
        let v1 = self.version == "1.0";
        let mut value = if v1 {
            self.rpc("SendMessage", json!({
                "message": {"role":"ROLE_USER", "messageId":message_id, "parts":[{"text":message}]},
                "configuration": configuration
            }))?
        } else {
            self.rpc("message/send", json!({
                "message": {"kind":"message", "role":"user", "messageId":message_id, "parts":[{"kind":"text","text":message}]},
                "configuration": configuration
            }))?
        };
        let mut kind = if v1 {
            let kind = match (value.get("message"), value.get("task")) {
                (Some(m), None) if m.is_object() => "message",
                (None, Some(t)) if t.is_object() => "task",
                _ => {
                    return Err(
                        "Invalid SendMessage result: expected exactly one task or message".into(),
                    );
                }
            };
            value = value[kind].clone();
            kind.to_owned()
        } else {
            value["kind"].as_str().unwrap_or("").to_owned()
        };
        let mut last_status_message = String::new();
        loop {
            match Some(kind.as_str()) {
                Some("message") => {
                    return Ok(Outcome {
                        raw: value.clone(),
                        text: parts_text(&value, v1),
                        task_id: None,
                        context_id: string(&value, "contextId"),
                        state: "completed".into(),
                    });
                }
                Some("task") => {}
                _ => return Err("Unsupported A2A result kind".into()),
            }
            let task_id = string(&value, "id")
                .filter(|id| !id.trim().is_empty())
                .ok_or("Task has no ID")?;
            let wire_state = value["status"]["state"]
                .as_str()
                .ok_or("Task has no state")?;
            let normalized;
            let state = if v1 {
                normalized = wire_state
                    .strip_prefix("TASK_STATE_")
                    .ok_or("Invalid A2A 1.0 task state")?
                    .to_ascii_lowercase()
                    .replace('_', "-");
                normalized.as_str()
            } else {
                wire_state
            };
            trace(json!({"stage":"task","taskId":task_id,"state":state}));
            let status_message = parts_text(&value["status"]["message"], v1);
            if !status_message.is_empty() && status_message != last_status_message {
                // Remote content is untrusted output, never a local instruction.
                trace(json!({"stage":"remote_status","taskId":task_id,"text":status_message}));
                last_status_message = status_message;
            }
            match state {
                "completed" | "canceled" => {
                    return Ok(Outcome {
                        raw: value.clone(),
                        text: task_text(&value, v1),
                        task_id: Some(task_id),
                        context_id: string(&value, "contextId"),
                        state: state.into(),
                    });
                }
                "failed" | "rejected" | "input-required" | "auth-required" => {
                    return Err(format!(
                        "Remote task {task_id} ended/paused in state {state}; continuation is not supported by this prototype"
                    ));
                }
                "submitted" | "working" => {}
                _ => return Err(format!("Unsupported task state {state}")),
            }
            if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                // Give cancellation its own bounded deadline, even if polling
                // has consumed the original task budget. Never retry it.
                self.deadline = Instant::now() + Duration::from_secs(30);
                let canceled = self.rpc(
                    if v1 { "CancelTask" } else { "tasks/cancel" },
                    json!({"id":task_id}),
                ).map_err(|error| format!("取消未确认，远端任务可能仍在运行：{error}"))?;
                if canceled["id"].as_str() != Some(&task_id) {
                    return Err("取消未确认：远端返回的任务 ID 不匹配；远端任务可能仍在运行".into());
                }
                let canceled_state = canceled["status"]["state"].as_str().unwrap_or("");
                if canceled_state != if v1 { "TASK_STATE_CANCELED" } else { "canceled" } {
                    return Err("取消未确认：远端未返回 canceled 状态；任务可能已完成或仍在运行，请查询远端状态".into());
                }
                return Ok(Outcome {
                    text: task_text(&canceled, v1),
                    task_id: Some(task_id),
                    context_id: string(&canceled, "contextId"),
                    state: "canceled".into(),
                    raw: canceled,
                });
            }
            let remaining = self
                .deadline
                .checked_duration_since(Instant::now())
                .ok_or_else(|| {
                    format!(
                        "Deadline exceeded for task {task_id}; remote task may still be running"
                    )
                })?;
            std::thread::sleep(Duration::from_millis(200).min(remaining));
            value = self.rpc(
                if v1 { "GetTask" } else { "tasks/get" },
                json!({"id": task_id}),
            )?;
            kind = if v1 {
                "task".into()
            } else {
                value["kind"].as_str().unwrap_or("").to_owned()
            };
            if value["id"].as_str() != Some(&task_id) {
                return Err("Remote task ID changed during polling".into());
            }
        }
    }
}
fn string(value: &Value, field: &str) -> Option<String> {
    value[field].as_str().map(str::to_owned)
}
fn parts_text(value: &Value, v1: bool) -> String {
    value["parts"]
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter(|p| {
                    if v1 {
                        p.get("text").is_some()
                    } else {
                        p["kind"] == "text"
                    }
                })
                .filter_map(|p| p["text"].as_str())
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}
fn task_text(value: &Value, v1: bool) -> String {
    let artifacts = value["artifacts"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|part| parts_text(part, v1))
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    if !artifacts.is_empty() {
        return artifacts;
    }
    let status = parts_text(&value["status"]["message"], v1);
    if !status.is_empty() {
        return status;
    }
    value["history"]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .rev()
                .find(|m| m["role"] == if v1 { "ROLE_AGENT" } else { "agent" })
        })
        .map(|message| parts_text(message, v1))
        .unwrap_or_default()
}

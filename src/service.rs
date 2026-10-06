//! Loopback service. HTTP framing is handled by tiny_http rather than a single TCP read.
use crate::catalog::{append_authenticated, get_agent, list_agents};
use crate::{A2aClient, Result};
use serde_json::{Value, json};
use std::{io::Read, time::Duration};

type Runs = std::sync::Mutex<
    std::collections::HashMap<String, std::sync::Arc<std::sync::atomic::AtomicBool>>,
>;

/// Own registration across every fallible preflight step, not just RPC execution.
struct RunRegistration<'a> {
    runs: &'a Runs,
    id: String,
    flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl<'a> RunRegistration<'a> {
    fn new(runs: &'a Runs, id: String) -> Result<Self> {
        validate_run_id(&id)?;
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut active = runs.lock().map_err(|_| "Run lock poisoned")?;
        if active.contains_key(&id) {
            return Err("Duplicate runId".into());
        }
        active.insert(id.clone(), flag.clone());
        Ok(Self { runs, id, flag })
    }
    fn check_before_submission(&self) -> Result<()> {
        if self.flag.load(std::sync::atomic::Ordering::SeqCst) {
            Err("发送前已停止；未提交远程任务".into())
        } else {
            Ok(())
        }
    }
}
impl Drop for RunRegistration<'_> {
    fn drop(&mut self) {
        if let Ok(mut active) = self.runs.lock() {
            active.remove(&self.id);
        }
    }
}
fn validate_run_id(id: &str) -> Result<()> {
    if id.trim().is_empty() || id.len() > 256 || id.chars().any(char::is_control) {
        return Err("Invalid runId".into());
    }
    Ok(())
}

fn dispatch(method: &str, path: &str, value: Value, runs: &Runs) -> Result<Value> {
    match (method, path) {
        ("GET", "/api/cards") => Ok(json!(list_agents()?)),
        ("POST", "/api/cards") => {
            let id = uuid::Uuid::new_v4().to_string();
            let auth: crate::auth::Auth =
                serde_json::from_value(value.get("auth").cloned().unwrap_or_else(|| json!({})))
                    .map_err(|_| "Invalid authentication configuration")?;
            auth.headers()?;
            if let Some(url) = value["cardUrl"].as_str() {
                Ok(json!(append_authenticated(
                    id,
                    "url",
                    Some(url.into()),
                    None,
                    auth
                )?))
            } else {
                let cards = value.get("agentCard").cloned().ok_or("missing agentCard")?;
                let cards = match cards {
                    Value::Array(items) => items,
                    item => vec![item],
                };
                if cards.is_empty() {
                    return Err("agentCard array is empty".into());
                }
                let mut saved = Vec::with_capacity(cards.len());
                // Validate the whole batch before appending any records.
                for card in &cards {
                    crate::inspect_card(card)?;
                }
                for card in cards {
                    let card_id = uuid::Uuid::new_v4().to_string();
                    saved.push(append_authenticated(
                        card_id,
                        "manual",
                        None,
                        Some(card),
                        auth.clone(),
                    )?);
                }
                Ok(json!(saved))
            }
        }
        ("POST", "/api/cards/delete") => {
            crate::catalog::delete_agent(value["id"].as_str().ok_or("Missing agent id")?)?;
            Ok(json!({"deleted":true}))
        }
        ("POST", "/api/cancel") => {
            let id = value["runId"].as_str().ok_or("missing runId")?;
            validate_run_id(id)?;
            let runs = runs.lock().map_err(|_| "Run lock poisoned")?;
            let flag = runs.get(id).ok_or("运行不存在或已结束；未发送取消请求")?;
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(
                json!({"requested":true,"message":"已接受取消请求；尚未提交则停止发送，已提交则等待请求返回和任务 ID 后请求远端取消，尚未确认远端取消"}),
            )
        }
        ("POST", "/api/run") => {
            let id = value["id"].as_str().ok_or("missing id")?;
            let message = value["message"].as_str().ok_or("missing message")?;
            let run_id = match value.get("runId") {
                Some(value) => value.as_str().ok_or("Invalid runId")?.to_owned(),
                None => uuid::Uuid::new_v4().to_string(),
            };
            let run = RunRegistration::new(runs, run_id)?;
            run.check_before_submission()?;
            let agent = get_agent(id)?;
            run.check_before_submission()?;
            let connected = if agent.source == "url" {
                A2aClient::connect_authenticated(
                    agent.card_url.as_deref().ok_or("missing card URL")?,
                    None,
                    Duration::from_secs(120),
                    &agent.auth,
                )
            } else {
                A2aClient::connect_card_authenticated(
                    agent.raw.as_ref().ok_or("Manual Agent has no Card")?,
                    None,
                    Duration::from_secs(120),
                    &agent.auth,
                )
            };
            // Card GET cancellation is cooperative: once it returns (even with
            // an error), honor the intent before making any SendMessage request.
            run.check_before_submission()?;
            let mut client = connected?;
            Ok(json!(
                client.run_cancellable(
                    message,
                    value
                        .get("configuration")
                        .cloned()
                        .unwrap_or_else(|| json!({})),
                    &run.flag,
                )?
            ))
        }
        _ => Err("not found".into()),
    }
}

pub fn serve() -> Result<()> {
    let token = std::env::var("ANY_A2A_SERVICE_TOKEN")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .ok_or("ANY_A2A_SERVICE_TOKEN is required")?;
    let server = tiny_http::Server::http("127.0.0.1:0").map_err(|_| "Cannot bind local service")?;
    println!("{}", server.server_addr());
    let runs = std::sync::Arc::new(Runs::default());
    let workers = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let cancel_workers = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    for request in server.incoming_requests() {
        // Every request may block on body I/O or a URL validation. Keep the accept
        // loop free, and reserve a separate bounded lane for cancellation even
        // when all ordinary workers are occupied. Store locks serialize writes.
        let (pool, limit) = if request.url() == "/api/cancel" {
            (&cancel_workers, 4)
        } else {
            (&workers, 8)
        };
        if pool.load(std::sync::atomic::Ordering::SeqCst) >= limit {
            let _ = request.respond(
                tiny_http::Response::from_string("{\"error\":\"Service busy; retry later\"}")
                    .with_status_code(503)
                    .with_header(
                        tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap(),
                    ),
            );
            continue;
        }
        pool.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (token, runs, pool) = (token.clone(), runs.clone(), pool.clone());
        std::thread::spawn(move || {
            struct Worker(std::sync::Arc<std::sync::atomic::AtomicUsize>);
            impl Drop for Worker {
                fn drop(&mut self) {
                    self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                }
            }
            let _worker = Worker(pool);
            handle_request(request, &token, &runs);
        });
    }
    Ok(())
}

fn handle_request(mut request: tiny_http::Request, token: &str, runs: &Runs) {
    let authorized = request
        .headers()
        .iter()
        .any(|h| h.field.equiv("Authorization") && h.value.as_str() == format!("Bearer {token}"));
    let (status, value) = if !authorized {
        (401, json!({"error":"unauthorized"}))
    } else {
        let mut bytes = Vec::new();
        let read = request
            .as_reader()
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes);
        if read.is_err() || bytes.len() > 4 * 1024 * 1024 {
            (413, json!({"error":"request too large or unreadable"}))
        } else {
            let body = if bytes.is_empty() {
                Ok(json!({}))
            } else {
                serde_json::from_slice(&bytes)
            };
            match body {
                Err(_) => (400, json!({"error":"invalid JSON"})),
                Ok(body) => {
                    crate::take_request_trace();
                    let started = std::time::Instant::now();
                    let result = dispatch(request.method().as_str(), request.url(), body, runs);
                    let trace = crate::take_request_trace();
                    let (status, mut value) = match result {
                        Ok(value) => (200, value),
                        Err(error) => (400, json!({"error":error})),
                    };
                    if request.url() == "/api/run" {
                        value["trace"] = json!(trace);
                        value["elapsedMs"] = json!(started.elapsed().as_millis());
                    }
                    eprintln!(
                        "[any-a2a] {} {} status={} elapsedMs={}",
                        request.method(),
                        request.url().split('?').next().unwrap_or("/"),
                        status,
                        started.elapsed().as_millis()
                    );
                    (status, value)
                }
            }
        }
    };
    let response = tiny_http::Response::from_string(value.to_string())
        .with_status_code(status)
        .with_header(tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap());
    let _ = request.respond(response);
}

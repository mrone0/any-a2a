//! Loopback service. HTTP framing is handled by tiny_http rather than a single TCP read.
use crate::catalog::{append_authenticated, list_agents};
use crate::{A2aClient, Result};
use serde_json::{Value, json};
use std::{io::Read, time::Duration};

type Runs = std::sync::Mutex<
    std::collections::HashMap<String, std::sync::Arc<std::sync::atomic::AtomicBool>>,
>;

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
            let runs = runs.lock().map_err(|_| "Run lock poisoned")?;
            let flag = runs.get(id).ok_or("运行不存在或已结束；未发送取消请求")?;
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(
                json!({"requested":true,"message":"已请求取消；等待当前请求返回并取得任务 ID 后发送远端取消，尚未确认"}),
            )
        }
        ("POST", "/api/run") => {
            let id = value["id"].as_str().ok_or("missing id")?;
            let message = value["message"].as_str().ok_or("missing message")?;
            let agent = list_agents()?
                .into_iter()
                .find(|a| a.id == id)
                .ok_or("agent not found")?;
            let mut client = if agent.source == "url" {
                A2aClient::connect_authenticated(
                    agent.card_url.as_deref().ok_or("missing card URL")?,
                    None,
                    Duration::from_secs(120),
                    &agent.auth,
                )?
            } else {
                A2aClient::connect_card_authenticated(
                    &agent.raw,
                    None,
                    Duration::from_secs(120),
                    &agent.auth,
                )?
            };
            let run_id = value["runId"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            {
                let mut active = runs.lock().map_err(|_| "Run lock poisoned")?;
                if active.contains_key(&run_id) {
                    return Err("Duplicate runId".into());
                }
                active.insert(run_id.clone(), flag.clone());
            }
            let result = client.run_cancellable(
                message,
                value
                    .get("configuration")
                    .cloned()
                    .unwrap_or_else(|| json!({})),
                &flag,
            );
            runs.lock()
                .map_err(|_| "Run lock poisoned")?
                .remove(&run_id);
            Ok(json!(result?))
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
    for request in server.incoming_requests() {
        // Keep cancellation reachable during blocking SendMessage/polling.
        // Only runs are concurrent; configuration mutations remain serialized.
        if request.url() == "/api/run" {
            if workers.load(std::sync::atomic::Ordering::SeqCst) >= 8 {
                let _ = request.respond(
                    tiny_http::Response::from_string("{\"error\":\"Too many active runs\"}")
                        .with_status_code(503),
                );
                continue;
            }
            workers.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let (token, runs, workers) = (token.clone(), runs.clone(), workers.clone());
            std::thread::spawn(move || {
                handle_request(request, &token, &runs);
                workers.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            });
        } else {
            handle_request(request, &token, &runs);
        }
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

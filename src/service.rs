//! Loopback service. HTTP framing is handled by tiny_http rather than a single TCP read.
use crate::catalog::{append_authenticated, list_agents};
use crate::{A2aClient, Result};
use serde_json::{Value, json};
use std::{io::Read, time::Duration};

fn dispatch(method: &str, path: &str, value: Value) -> Result<Value> {
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
            Ok(json!(
                client.run_with_configuration(
                    message,
                    value
                        .get("configuration")
                        .cloned()
                        .unwrap_or_else(|| json!({}))
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
    for mut request in server.incoming_requests() {
        let authorized = request.headers().iter().any(|h| {
            h.field.equiv("Authorization") && h.value.as_str() == format!("Bearer {token}")
        });
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
                        let result = dispatch(request.method().as_str(), request.url(), body);
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
            .with_header(
                tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap(),
            );
        let _ = request.respond(response);
    }
    Ok(())
}

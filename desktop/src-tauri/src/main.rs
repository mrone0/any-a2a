#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use std::{io::{BufRead, BufReader}, process::{Child, Command, Stdio}, sync::Mutex};
use tauri::Manager;
mod clients;
#[path = "../../../adapters/dsh/desktop/apply.rs"]
mod apply_dsh;
use apply_dsh::apply_dsh;
#[path = "../../../adapters/dsh/desktop/discovery.rs"]
mod dsh_discovery;
use dsh_discovery::discover_dsh;
mod config_file;
use config_file::{config_location, read_config, save_config, open_config};
use clients::scan_clients;

struct Service( Mutex<Option<Child>> );
struct Connection { address: String, token: String }

fn start_service(token: &str) -> Result<Child, String> {
    let executable = std::env::var("ANY_A2A_EXECUTABLE").unwrap_or_else(|_| {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/any-a2a").to_string_lossy().into_owned()
    });
    Command::new(executable)
        .arg("serve")
        .env("ANY_A2A_SERVICE_TOKEN", token)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("cannot start any-a2a service: {e}"))
}

#[tauri::command]
fn service_status(state: tauri::State<'_, Service>) -> Result<bool, String> {
    let mut guard = state.0.lock().map_err(|_| "service lock poisoned")?;
    if let Some(child) = guard.as_mut() {
        match child.try_wait().map_err(|_| "cannot inspect service")? {
            None => Ok(true),
            Some(_) => { *guard = None; Ok(false) }
        }
    } else { Ok(false) }
}

#[tauri::command]
async fn run_card(address: tauri::State<'_, Connection>, id: String, message: String, configuration: Option<serde_json::Value>) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::new();
    client.post(format!("http://{}/api/run", address.address)).bearer_auth(&address.token).json(&serde_json::json!({"id":id,"message":message,"configuration":configuration.unwrap_or_else(|| serde_json::json!({}))})).send().await.map_err(|_| "Service unavailable".to_string())?.json().await.map_err(|_| "Invalid service response".to_string())
}

#[tauri::command]
async fn delete_card(address: tauri::State<'_, Connection>, id: String) -> Result<serde_json::Value, String> {
    reqwest::Client::new().post(format!("http://{}/api/cards/delete", address.address))
        .bearer_auth(&address.token).json(&serde_json::json!({"id":id})).send().await
        .map_err(|_| "Service unavailable".to_string())?.json().await.map_err(|_| "Invalid service response".to_string())
}

#[tauri::command]
async fn cards_api(address: tauri::State<'_, Connection>, body: Option<serde_json::Value>) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::new();
    let url = format!("http://{}/api/cards", address.address);
    let request = match body { Some(value) => client.post(url).json(&value), None => client.get(url) };
    request.bearer_auth(&address.token).send().await.map_err(|_| "Service unavailable".to_string())?
        .json().await.map_err(|_| "Invalid service response".to_string())
}

fn main() {
    tauri::Builder::default()
        .manage(Service(Mutex::new(None)))
        .invoke_handler(tauri::generate_handler![service_status, cards_api, run_card, scan_clients, delete_card, config_location, read_config, save_config, open_config, discover_dsh, apply_dsh])
        .setup(|app| {
            let token = uuid::Uuid::new_v4().to_string();
            let mut child = start_service(&token).map_err(std::io::Error::other)?;
            let stdout = child.stdout.take().ok_or_else(|| std::io::Error::other("service stdout unavailable"))?;
            let mut lines = BufReader::new(stdout).lines();
            let address = lines.next().ok_or_else(|| std::io::Error::other("service did not publish address"))?.map_err(std::io::Error::other)?;
            app.manage(Connection { address, token });
            app.state::<Service>().0.lock().map_err(|_| std::io::Error::other("service lock poisoned"))?.replace(child);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building any-a2a")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit) {
                if let Ok(mut guard) = app.state::<Service>().0.lock() { if let Some(mut child) = guard.take() { let _ = child.kill(); let _ = child.wait(); } }
            }
        });
}

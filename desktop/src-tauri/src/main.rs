#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use std::{
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Mutex,
};
use tauri::Manager;
#[path = "../../../adapters/dsh/desktop/apply.rs"]
mod apply_dsh;
mod clients;
mod deployment;
mod pi_host;
mod setup_command;
use apply_dsh::apply_dsh;
#[path = "../../../adapters/dsh/desktop/discovery.rs"]
mod dsh_discovery;
use dsh_discovery::discover_dsh;
mod config_file;
use clients::scan_clients;
use config_file::{config_location, open_config, read_config, save_config};

struct Service(Mutex<Option<Child>>);
struct Connection {
    address: String,
    token: String,
}
struct RuntimePaths {
    executable: PathBuf,
    data: PathBuf,
}

fn start_service(executable: &Path, token: &str) -> Result<Child, String> {
    let mut command = Command::new(executable);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command
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
            Some(_) => {
                *guard = None;
                Ok(false)
            }
        }
    } else {
        Ok(false)
    }
}

#[tauri::command]
async fn run_card(
    address: tauri::State<'_, Connection>,
    id: String,
    message: String,
    configuration: Option<serde_json::Value>,
    run_id: String,
) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::new();
    client.post(format!("http://{}/api/run", address.address)).bearer_auth(&address.token).json(&serde_json::json!({"id":id,"runId":run_id,"message":message,"configuration":configuration.unwrap_or_else(|| serde_json::json!({}))})).send().await.map_err(|_| "Service unavailable".to_string())?.json().await.map_err(|_| "Invalid service response".to_string())
}

#[tauri::command]
async fn cancel_run(
    address: tauri::State<'_, Connection>,
    run_id: String,
) -> Result<serde_json::Value, String> {
    reqwest::Client::new()
        .post(format!("http://{}/api/cancel", address.address))
        .bearer_auth(&address.token)
        .json(&serde_json::json!({"runId":run_id}))
        .send()
        .await
        .map_err(|_| "取消请求失败；远端状态未知".to_string())?
        .json()
        .await
        .map_err(|_| "取消响应无效；远端状态未知".to_string())
}

#[tauri::command]
async fn delete_card(
    address: tauri::State<'_, Connection>,
    id: String,
) -> Result<serde_json::Value, String> {
    reqwest::Client::new()
        .post(format!("http://{}/api/cards/delete", address.address))
        .bearer_auth(&address.token)
        .json(&serde_json::json!({"id":id}))
        .send()
        .await
        .map_err(|_| "Service unavailable".to_string())?
        .json()
        .await
        .map_err(|_| "Invalid service response".to_string())
}

#[tauri::command]
async fn cards_api(
    address: tauri::State<'_, Connection>,
    body: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::new();
    let url = format!("http://{}/api/cards", address.address);
    let request = match body {
        Some(value) => client.post(url).json(&value),
        None => client.get(url),
    };
    request
        .bearer_auth(&address.token)
        .send()
        .await
        .map_err(|_| "Service unavailable".to_string())?
        .json()
        .await
        .map_err(|_| "Invalid service response".to_string())
}

#[tauri::command]
fn pi_setup(paths: tauri::State<'_, RuntimePaths>) -> Result<serde_json::Value, String> {
    let package = deployment::stage("pi", &paths.data, &paths.executable)?;
    let extension = package.join("index.ts");
    let executable = package
        .join("bin")
        .join(format!("any-a2a{}", std::env::consts::EXE_SUFFIX));
    let (shell, command) = setup_command::pi_command(
        &executable.to_string_lossy(),
        &paths.data.to_string_lossy(),
        &extension.to_string_lossy(),
        cfg!(target_os = "windows"),
    );
    Ok(
        serde_json::json!({"command":command,"shell":shell,"platform":std::env::consts::OS,"installed":false}),
    )
}

#[tauri::command]
fn pi_install(paths: tauri::State<'_, RuntimePaths>) -> Result<serde_json::Value, String> {
    let extension = deployment::stage("pi", &paths.data, &paths.executable)?;
    let home = std::env::var_os("PI_CODING_AGENT_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .map(|h| std::path::PathBuf::from(h).join(".pi/agent"))
        })
        .ok_or("Cannot locate Pi settings")?;
    let settings = home.join("settings.json");
    if settings.exists() {
        std::fs::copy(
            &settings,
            home.join(format!("settings.json.backup-{}", uuid::Uuid::new_v4())),
        )
        .map_err(|_| "Cannot back up Pi settings")?;
    }
    let output = pi_host::install_command()?
        .arg("install")
        .arg(&extension)
        .output()
        .map_err(|e| format!("无法执行 pi install：{e}"))?;
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    let error = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        return Err(if error.trim().is_empty() {
            format!("pi install 失败（{}）", output.status)
        } else {
            error
        });
    }
    Ok(serde_json::json!({"installed":true,"output":text,"error":error}))
}

fn main() {
    tauri::Builder::default()
        .manage(Service(Mutex::new(None)))
        .invoke_handler(tauri::generate_handler![
            pi_setup,
            pi_install,
            service_status,
            cards_api,
            run_card,
            cancel_run,
            scan_clients,
            delete_card,
            config_location,
            read_config,
            save_config,
            open_config,
            discover_dsh,
            apply_dsh
        ])
        .setup(|app| {
            let token = uuid::Uuid::new_v4().to_string();
            let data = any_a2a::catalog::data_dir().map_err(std::io::Error::other)?;
            std::fs::create_dir_all(&data)?;
            let paths = RuntimePaths {
                executable: deployment::executable().map_err(std::io::Error::other)?,
                data: data.canonicalize()?,
            };
            let mut child =
                start_service(&paths.executable, &token).map_err(std::io::Error::other)?;
            let stdout = child
                .stdout
                .take()
                .ok_or_else(|| std::io::Error::other("service stdout unavailable"))?;
            let mut lines = BufReader::new(stdout).lines();
            let address = lines
                .next()
                .ok_or_else(|| std::io::Error::other("service did not publish address"))?
                .map_err(std::io::Error::other)?;
            app.manage(Connection { address, token });
            app.manage(paths);
            app.state::<Service>()
                .0
                .lock()
                .map_err(|_| std::io::Error::other("service lock poisoned"))?
                .replace(child);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building any-a2a")
        .run(|app, event| {
            if matches!(
                event,
                tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
            ) {
                if let Ok(mut guard) = app.state::<Service>().0.lock() {
                    if let Some(mut child) = guard.take() {
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                }
            }
        });
}

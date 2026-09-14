//! Explicit local configuration editing. No arbitrary path or shell execution.
use std::{fs, io::Write, path::PathBuf, sync::Mutex};
use serde_json::{json, Value};
static EDIT: Mutex<()> = Mutex::new(());
fn path() -> Result<PathBuf, String> { Ok(any_a2a::catalog::data_dir()?.join("agent-cards.jsonl")) }
fn read() -> Result<String, String> {
    let path = path()?;
    if !path.exists() { return Ok(String::new()); }
    let meta = fs::symlink_metadata(&path).map_err(|_| "Cannot inspect configuration")?;
    if !meta.is_file() || meta.len() > 8*1024*1024 { return Err("Expected a regular configuration file under 8 MiB".into()); }
    fs::read_to_string(path).map_err(|_| "Cannot read configuration".into())
}
fn validate(text: &str) -> Result<(), String> {
    if text.len() > 8*1024*1024 { return Err("Configuration exceeds 8 MiB".into()); }
    for (index, line) in text.lines().enumerate().filter(|(_,l)| !l.trim().is_empty()) {
        let check = || -> Result<(), String> {
            let v: Value = serde_json::from_str(line).map_err(|_| "Invalid JSON")?;
            if v["id"].as_str().is_none_or(|s| s.trim().is_empty()) { return Err("Missing id".into()); }
            if v["deleted"] == true { return Ok(()); }
            match v["source"].as_str() {
                Some("manual") => { any_a2a::inspect_card(&v["agentCard"])?; },
                Some("url") => {
                    let url = reqwest::Url::parse(v["cardUrl"].as_str().ok_or("Missing cardUrl")?).map_err(|_| "Invalid URL")?;
                    if !matches!(url.scheme(), "http"|"https") || !url.username().is_empty() || url.password().is_some() { return Err("Invalid card URL".into()); }
                },
                _ => return Err("source must be url or manual".into()),
            }
            let auth: any_a2a::auth::Auth = serde_json::from_value(v.get("auth").cloned().unwrap_or(json!({}))).map_err(|_| "Invalid authentication")?;
            auth.headers()?;
            Ok(())
        };
        check().map_err(|e| format!("第 {} 行：{e}",index+1))?;
    }
    Ok(())
}
#[tauri::command]
pub fn config_location() -> Result<String,String> { Ok(path()?.to_string_lossy().into_owned()) }
#[tauri::command]
pub fn read_config() -> Result<String,String> { read() }
#[tauri::command]
pub fn save_config(content: String, original: String) -> Result<Value,String> {
    let _guard = EDIT.lock().map_err(|_| "Configuration busy")?;
    validate(&content)?;
    if read()? != original { return Err("文件已发生变化，请重新加载后编辑".into()); }
    let path = path()?;
    fs::create_dir_all(path.parent().unwrap()).map_err(|_| "Cannot create configuration directory")?;
    let backup = path.with_extension(format!("backup-{}", uuid::Uuid::new_v4()));
    let mut opts = fs::OpenOptions::new(); opts.create_new(true).write(true);
    #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; opts.mode(0o600); }
    let mut file = opts.open(&backup).map_err(|_| "Cannot create backup")?;
    file.write_all(original.as_bytes()).and_then(|_|file.sync_all()).map_err(|_| "Cannot persist backup")?;
    let content = if content.is_empty() || content.ends_with('\n') {content} else {format!("{content}\n")};
    if read()? != original { return Err("文件在保存期间被修改，请重新加载".into()); }
    any_a2a::catalog::atomic_write(&path, content.as_bytes())?;
    Ok(json!({"backup":backup}))
}
#[tauri::command]
pub fn open_config() -> Result<(),String> {
    let path = path()?;
    if !path.is_file() { return Err("请先保存至少一个 Agent，或在编辑器中保存文件".into()); }
    #[cfg(target_os="macos")]
    let result = std::process::Command::new("open").arg("-t").arg(&path).status();
    #[cfg(windows)]
    let result = std::process::Command::new("notepad.exe").arg(&path).spawn().map(|_| ());
    #[cfg(not(any(target_os="macos",windows)))]
    let result = std::process::Command::new("xdg-open").arg(&path).status();
    result.map_err(|_| "Cannot open system editor")?;
    Ok(())
}
#[cfg(test)] mod tests {
    #[test] fn reject_invalid_before_write() {
        assert!(super::validate("{broken}").is_err());
        assert!(super::validate("{\"id\":\"x\",\"deleted\":true}\n").is_ok());
        assert!(super::validate("{\"id\":\"x\",\"source\":\"manual\",\"agentCard\":{}}\n").is_err());
    }
}

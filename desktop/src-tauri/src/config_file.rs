//! Explicit local configuration editing. No arbitrary path or shell execution.
use serde_json::{json, Value};
use std::path::PathBuf;
fn path() -> Result<PathBuf, String> {
    Ok(any_a2a::catalog::data_dir()?.join("agent-cards.jsonl"))
}
fn read() -> Result<String, String> {
    any_a2a::catalog_store::Store::at(any_a2a::catalog::data_dir()?).read_text()
}
fn validate(text: &str) -> Result<(), String> {
    any_a2a::catalog::validate_store_text(text)
}
#[tauri::command]
pub fn config_location() -> Result<String, String> {
    Ok(path()?.to_string_lossy().into_owned())
}
#[tauri::command]
pub fn read_config() -> Result<String, String> {
    read()
}
#[tauri::command]
pub fn save_config(content: String, original: String) -> Result<Value, String> {
    validate(&content)?;
    let backup = any_a2a::catalog_store::Store::at(any_a2a::catalog::data_dir()?)
        .replace_text(&original, &content)?;
    Ok(json!({"backup":backup}))
}
#[tauri::command]
pub fn open_config() -> Result<(), String> {
    let path = path()?;
    if !path.is_file() {
        return Err("请先保存至少一个 Agent，或在编辑器中保存文件".into());
    }
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open")
        .arg("-t")
        .arg(&path)
        .status();
    #[cfg(windows)]
    let result = std::process::Command::new("notepad.exe")
        .arg(&path)
        .spawn()
        .map(|_| ());
    #[cfg(not(any(target_os = "macos", windows)))]
    let result = std::process::Command::new("xdg-open").arg(&path).status();
    result.map_err(|_| "Cannot open system editor")?;
    Ok(())
}
#[cfg(test)]
mod tests {
    #[test]
    fn reject_invalid_before_write() {
        assert!(super::validate("{broken}").is_err());
        assert!(super::validate("{\"id\":\"x\",\"deleted\":true}\n").is_ok());
        assert!(
            super::validate("{\"id\":\"x\",\"source\":\"manual\",\"agentCard\":{}}\n").is_err()
        );
    }
}

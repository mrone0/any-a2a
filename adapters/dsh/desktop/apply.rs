//! Explicit profile write, only after the user presses Apply. No YAML evaluation.
use serde_json::{json, Value};
use std::{fs, io::Write, path::Path};

#[tauri::command]
pub async fn apply_dsh(
    connection: tauri::State<'_, crate::Connection>,
    path: String,
) -> Result<Value, String> {
    let response = reqwest::Client::new()
        .get(format!("http://{}/api/cards", connection.address))
        .bearer_auth(&connection.token)
        .send()
        .await
        .map_err(|_| "Service unavailable")?;
    let agents: Vec<Value> = response
        .json()
        .await
        .map_err(|_| "Cannot read saved Agents")?;
    if agents.is_empty() {
        return Err("请先添加 Agent".into());
    }
    let file = Path::new(&path);
    if !file.is_absolute() {
        return Err("请选择绝对路径".into());
    }
    let meta = fs::symlink_metadata(file).map_err(|_| "Cannot inspect patch")?;
    if !meta.is_file() || meta.len() > 4 * 1024 * 1024 {
        return Err("Expected regular patch under 4 MiB".into());
    }
    let original = fs::read_to_string(file).map_err(|_| "Cannot read patch")?;
    // Only the pinned, shipped comment header over [] is accepted alongside pure JSON.
    let default = "# Your patch layer for this dsh profile, applied after every bundle layer:\n# a top-level YAML array of loader patch entries (id-targeted config\n# overrides, disables, and insert lists; `!!js` expressions allowed).\n";
    let body = original.strip_prefix(default).unwrap_or(&original);
    let mut patches: Vec<Value> = serde_json::from_str(body).map_err(|_| {
        "只支持 JSON 数组或 DSH 默认空配置；不解析通用 YAML，请使用独立 JSON overlay"
    })?;
    if original.contains("any-a2a-app-") {
        return Err("已存在 any-a2a 配置，请先核对已有配置，避免重复写入".into());
    }
    let adapter = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../adapters/dsh/src/index.js")
        .canonicalize()
        .map_err(|_| "Adapter source missing")?;
    let executable = std::env::var_os("ANY_A2A_EXECUTABLE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/debug")
                .join(format!("any-a2a{}", std::env::consts::EXE_SUFFIX))
        })
        .canonicalize()
        .map_err(|_| "CLI 不存在，请先 cargo build 或设置 ANY_A2A_EXECUTABLE 为绝对路径")?;
    let data_dir = any_a2a::catalog::data_dir()?
        .canonicalize()
        .map_err(|_| "Cannot resolve catalog directory")?;
    let adapter_url = reqwest::Url::from_file_path(&adapter)
        .map_err(|_| "Cannot convert adapter path to file URL")?
        .to_string();
    let tool_url = reqwest::Url::from_file_path(adapter.with_file_name("delegation-tool.js"))
        .map_err(|_| "Cannot convert tool path to file URL")?
        .to_string();
    let mut rows = vec![];
    let mut tools = vec![];
    for (index, agent) in agents.iter().enumerate() {
        let id = agent["id"].as_str().ok_or("Invalid Agent id")?;
        let provider = format!("any-a2a-app-{}", uuid::Uuid::new_v4());
        let name = agent["info"]["name"].as_str().unwrap_or("remote_agent");
        let normalized: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .take(48)
            .collect();
        let mut tool = if normalized.trim_matches('_').is_empty() {
            format!("remote_agent_{}", index + 1)
        } else {
            normalized
        };
        if tools.contains(&tool) {
            tool = format!("{tool}_{}", index + 1);
        }
        rows.push(json!({"id":provider,"name":adapter_url,"config":{"executable":executable,"dataDir":data_dir,"agentId":id,"providerName":provider,"toolName":tool}}));
        rows.push(json!({"id":format!("{provider}-tool"),"name":tool_url,"config":{"provider":provider,"toolName":tool,"backgroundMode":"one-shot","maxDepth":"provider-managed","enableRunInBackground":true,"modelSelectionSettings":false}}));
        tools.push(tool);
    }
    patches.push(json!({"insert":rows}));
    let backup = file.with_extension(format!("yml.backup-{}", uuid::Uuid::new_v4()));
    let mut opts = fs::OpenOptions::new();
    opts.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut out = opts.open(&backup).map_err(|_| "Cannot create backup")?;
    out.write_all(original.as_bytes())
        .and_then(|_| out.sync_all())
        .map_err(|_| "Cannot persist backup")?;
    if fs::read_to_string(file).map_err(|_| "Cannot recheck patch")? != original {
        return Err("配置已改变，请重试".into());
    }
    any_a2a::catalog::atomic_write(
        file,
        serde_json::to_string_pretty(&patches).unwrap().as_bytes(),
    )?;
    Ok(
        json!({"configured":true,"activated":false,"backup":backup,"tools":tools,"message":"Provider 和工具行已写入。Web preset 授权尚未自动配置；重启 pnpm dsh web 后仍需验证工具可见性。客户端直接启动 CLI，关闭桌面 App 不影响客户端委派。请保持 CLI、适配器及本地目录路径有效。"}),
    )
}

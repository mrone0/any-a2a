//! Explicit profile write, only after the user presses Apply. No YAML evaluation.
use serde_json::{json, Value};
use std::{collections::HashSet, fs, io::Write, path::Path};

const TOOL_BASE_LIMIT: usize = 48;
// Our interoperability ceiling; the inspected DSH registry does not impose this length limit.
const TOOL_NAME_LIMIT: usize = 64;

fn collect_tool_names(value: &Value, used: &mut HashSet<String>) {
    match value {
        Value::Object(fields) => {
            if let Some(name) = fields.get("toolName").and_then(Value::as_str) {
                used.insert(name.to_owned());
            }
            for value in fields.values() {
                collect_tool_names(value, used);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_tool_names(value, used);
            }
        }
        _ => {}
    }
}

fn occupied_tool_names(patches: &[Value]) -> HashSet<String> {
    // DSH reserves this exact name for its Code Mode transport.
    let mut used = HashSet::from(["run_code".to_owned()]);
    for patch in patches {
        collect_tool_names(patch, &mut used);
    }
    used
}

fn suffixed_tool_name(base: &str, serial: usize) -> String {
    let suffix = format!("_{serial}");
    let available = TOOL_NAME_LIMIT - suffix.len();
    // The normalized base and the fallback contain only ASCII bytes.
    format!("{}{suffix}", &base[..base.len().min(available)])
}

fn allocate_tool_name(name: &str, index: usize, used: &mut HashSet<String>) -> String {
    let normalized: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                character
            } else {
                '_'
            }
        })
        .take(TOOL_BASE_LIMIT)
        .collect();
    let base = if normalized.trim_matches('_').is_empty() {
        format!("remote_agent_{}", index.saturating_add(1))
    } else {
        normalized
    };
    let mut candidate = base.clone();
    let mut serial = 2;
    while !used.insert(candidate.clone()) {
        candidate = suffixed_tool_name(&base, serial);
        serial += 1;
    }
    candidate
}

#[tauri::command]
pub async fn apply_dsh(
    connection: tauri::State<'_, crate::Connection>,
    paths: tauri::State<'_, crate::RuntimePaths>,
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
    let package = crate::deployment::stage("dsh", &paths.data, &paths.executable)?;
    let adapter = package.join("src/index.js");
    let executable = package
        .join("bin")
        .join(format!("any-a2a{}", std::env::consts::EXE_SUFFIX));
    let data_dir = &paths.data;
    let adapter_url = reqwest::Url::from_file_path(&adapter)
        .map_err(|_| "Cannot convert adapter path to file URL")?
        .to_string();
    let tool_url = reqwest::Url::from_file_path(adapter.with_file_name("delegation-tool.js"))
        .map_err(|_| "Cannot convert tool path to file URL")?
        .to_string();
    let mut rows = vec![];
    let mut tools = vec![];
    let mut used = occupied_tool_names(&patches);
    for (index, agent) in agents.iter().enumerate() {
        let id = agent["id"].as_str().ok_or("Invalid Agent id")?;
        let provider = format!("any-a2a-app-{}", uuid::Uuid::new_v4());
        let name = agent["info"]["name"].as_str().unwrap_or("");
        let tool = allocate_tool_name(name, index, &mut used);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collision_suffix_is_checked_against_already_allocated_names() {
        let mut used = occupied_tool_names(&[]);
        let names = ["foo", "foo_3", "foo"]
            .iter()
            .enumerate()
            .map(|(index, name)| allocate_tool_name(name, index, &mut used))
            .collect::<Vec<_>>();
        assert_eq!(names, ["foo", "foo_3", "foo_2"]);
        assert_eq!(names.iter().collect::<HashSet<_>>().len(), names.len());
    }

    #[test]
    fn nested_foreign_tool_names_are_reserved_without_conflating_plugin_names() {
        let patches = vec![json!({"insert":[
            {"name":"foreign-plugin","config":{"toolName":"foo"}},
            {"disabled":true,"config":{"nested":{"toolName":"foo_2"}}},
            {"insert":[{"config":{"toolName":"foo_3"}}]},
            {"config":{"toolName":42}}
        ]})];
        let mut used = occupied_tool_names(&patches);
        assert!(!used.contains("foreign-plugin"));
        assert!(!used.contains("42"));
        assert_eq!(allocate_tool_name("foo", 0, &mut used), "foo_4");
        assert_eq!(allocate_tool_name("foo", 1, &mut used), "foo_5");
    }

    #[test]
    fn reserved_transport_name_is_never_allocated() {
        let mut used = occupied_tool_names(&[]);
        assert_eq!(allocate_tool_name("run_code", 0, &mut used), "run_code_2");
        assert_eq!(allocate_tool_name("run_code", 1, &mut used), "run_code_3");
    }

    #[test]
    fn blank_and_unicode_names_get_readable_ascii_fallbacks() {
        let mut used = occupied_tool_names(&[]);
        for (index, name) in ["", "中文🙂", "___"].iter().enumerate() {
            assert_eq!(
                allocate_tool_name(name, index, &mut used),
                format!("remote_agent_{}", index + 1)
            );
        }
        let mixed = allocate_tool_name("alpha beta/中文", 3, &mut used);
        assert_eq!(mixed, "alpha_beta___");
        assert!(mixed
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')));
    }

    #[test]
    fn long_bases_and_large_suffixes_stay_within_interoperability_limit() {
        let mut used = occupied_tool_names(&[]);
        let name = "a".repeat(500);
        let first = allocate_tool_name(&name, 0, &mut used);
        let second = allocate_tool_name(&name, 1, &mut used);
        assert_eq!(first.len(), TOOL_BASE_LIMIT);
        assert_eq!(second, format!("{first}_2"));
        let largest = suffixed_tool_name(&first, usize::MAX);
        assert!(largest.is_ascii());
        assert!(largest.len() <= TOOL_NAME_LIMIT);
        assert!(largest.ends_with(&format!("_{}", usize::MAX)));
    }

    #[test]
    fn repeated_generation_for_the_same_ordered_catalog_is_deterministic() {
        let foreign = vec![json!({"config":{"toolName":"remote_agent_3"}})];
        let allocate = || {
            let mut used = occupied_tool_names(&foreign);
            ["foo", "foo!", "中文", "foo", "foo_2", "foo!"]
                .iter()
                .enumerate()
                .map(|(index, name)| allocate_tool_name(name, index, &mut used))
                .collect::<Vec<_>>()
        };
        let first = allocate();
        assert_eq!(first, allocate());
        assert_eq!(first.iter().collect::<HashSet<_>>().len(), first.len());
        assert!(first.iter().all(|name| name.len() <= TOOL_NAME_LIMIT));
    }
}

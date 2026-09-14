//! Local catalog. Remote metadata is data, never executable configuration.
use crate::{CardInfo, Result, inspect_card};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};
const LIMIT: u64 = 8 * 1024 * 1024;

#[derive(Serialize, Deserialize, Clone)]
pub struct StoredAgent {
    pub id: String,
    pub raw: Value,
    pub info: CardInfo,
    pub source: String,
    #[serde(default)]
    pub card_url: Option<String>,
    #[serde(default, skip_serializing)]
    pub auth: crate::auth::Auth,
}
#[derive(Serialize)]
pub struct PreviewAgent {
    pub raw: Value,
    pub info: CardInfo,
}
#[derive(Serialize)]
pub struct Preview {
    pub agents: Vec<PreviewAgent>,
    pub errors: Vec<String>,
    pub empty: bool,
}

pub fn data_dir() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("ANY_A2A_DATA_DIR") {
        return Ok(PathBuf::from(path));
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or("Home directory unavailable")?;
    #[cfg(target_os = "macos")]
    let path = PathBuf::from(home).join("Library/Application Support/any-a2a");
    #[cfg(target_os = "windows")]
    let path = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or(PathBuf::from(home))
        .join("any-a2a");
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let path = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or(PathBuf::from(home).join(".local/share"))
        .join("any-a2a");
    Ok(path)
}

pub fn preview_source(url: &str, token: Option<String>) -> Result<Preview> {
    preview_authenticated(url, token, &crate::auth::Auth::default())
}
fn preview_authenticated(
    url: &str,
    token: Option<String>,
    auth: &crate::auth::Auth,
) -> Result<Preview> {
    let url = reqwest::Url::parse(url).map_err(|_| "Invalid source URL")?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Use an HTTP(S) URL without credentials, query or fragment; supply credentials in Token".into());
    }
    let client = reqwest::blocking::Client::builder()
        .default_headers(auth.headers()?)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| "HTTP client initialization failed")?;
    let mut request = client.get(url);
    if let Some(token) = token.filter(|s| !s.trim().is_empty()) {
        request = request.bearer_auth(token);
    }
    let response = request.send().map_err(|_| "Directory request failed")?;
    if !response.status().is_success() {
        return Err(format!("Directory HTTP error {}", response.status()));
    }
    let mut bytes = Vec::new();
    response
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Directory read failed")?;
    if bytes.len() as u64 > LIMIT {
        return Err("Directory exceeds 8 MiB".into());
    }
    // A URL normally returns one Agent Card object. Keep array parsing only as
    // backwards-compatible support for the old bulk-preview endpoint.
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|_| "Agent Card is not valid JSON")?;
    if value.is_object() {
        let info = inspect_card(&value)?;
        return Ok(Preview {
            agents: vec![PreviewAgent { raw: value, info }],
            errors: vec![],
            empty: false,
        });
    }
    parse_directory(&bytes)
}

pub fn parse_directory(bytes: &[u8]) -> Result<Preview> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(Preview {
            agents: vec![],
            errors: vec![],
            empty: true,
        });
    }
    let value: Value = serde_json::from_slice(bytes).map_err(|_| "Directory is not valid JSON")?;
    let cards = value
        .as_array()
        .ok_or("Expected a complete Agent Card JSON array")?;
    if cards.len() > 1000 {
        return Err("Directory exceeds 1000 cards".into());
    }
    let mut agents = vec![];
    let mut errors = vec![];
    for (index, raw) in cards.iter().enumerate() {
        match inspect_card(raw) {
            Ok(info) => agents.push(PreviewAgent {
                raw: raw.clone(),
                info,
            }),
            Err(error) => errors.push(format!("Card {}: {error}", index + 1)),
        }
    }
    Ok(Preview {
        agents,
        errors,
        empty: cards.is_empty(),
    })
}

/// Append one URL-sourced or manually supplied Agent Card to the single durable JSONL store.
pub fn append_card(
    id: String,
    source: &str,
    card_url: Option<String>,
    raw: Option<Value>,
) -> Result<StoredAgent> {
    append_authenticated(id, source, card_url, raw, crate::auth::Auth::default())
}
pub fn append_authenticated(
    id: String,
    source: &str,
    card_url: Option<String>,
    raw: Option<Value>,
    auth: crate::auth::Auth,
) -> Result<StoredAgent> {
    auth.headers()?;
    if id.trim().is_empty() || id.contains('\n') {
        return Err("Invalid agent id".into());
    }
    if source != "url" && source != "manual" {
        return Err("Source must be url or manual".into());
    }
    let (raw, card_url) = match source {
        "url" => {
            let url = card_url.ok_or("URL source requires cardUrl")?;
            let preview = preview_authenticated(&url, None, &auth)?;
            let item = preview
                .agents
                .into_iter()
                .next()
                .ok_or("URL did not return an Agent Card")?;
            (item.raw, Some(url))
        }
        _ => (raw.ok_or("Manual source requires agentCard")?, None),
    };
    let info = inspect_card(&raw)?;
    let item = StoredAgent {
        id,
        raw,
        info,
        source: source.into(),
        card_url: card_url.clone(),
        auth,
    };
    let dir = data_dir()?;
    fs::create_dir_all(&dir).map_err(|_| "Cannot create data directory")?;
    let path = dir.join("agent-cards.jsonl");
    let mut options = fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options.mode(0o600);
        if path.exists() {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
                .map_err(|_| "Cannot protect credential file")?;
        }
    }
    let mut file = options.open(path).map_err(|_| "Cannot open card store")?;
    let record = json_record(&item, card_url);
    writeln!(
        file,
        "{}",
        serde_json::to_string(&record).map_err(|_| "Cannot serialize card")?
    )
    .map_err(|_| "Cannot append card")?;
    file.sync_all().map_err(|_| "Cannot persist card")?;
    Ok(item)
}

fn json_record(item: &StoredAgent, card_url: Option<String>) -> Value {
    let mut record = serde_json::Map::new();
    record.insert("id".into(), Value::String(item.id.clone()));
    record.insert("source".into(), Value::String(item.source.clone()));
    record.insert(
        "auth".into(),
        serde_json::to_value(&item.auth).expect("auth serializes"),
    );
    if let Some(url) = card_url {
        record.insert("cardUrl".into(), Value::String(url));
    } else {
        record.insert("agentCard".into(), item.raw.clone());
    }
    Value::Object(record)
}

pub fn list_agents() -> Result<Vec<StoredAgent>> {
    read_agents(None)
}

/// Resolve only the selected agent so unrelated unavailable URLs cannot block a run.
pub fn get_agent(id: &str) -> Result<StoredAgent> {
    read_agents(Some(id))?.into_iter().next().ok_or("Agent not found in local catalog".into())
}

fn read_agents(selected: Option<&str>) -> Result<Vec<StoredAgent>> {
    let path = data_dir()?.join("agent-cards.jsonl");
    let bytes = match fs::read(path) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(_) => return Err("Cannot read card store".into()),
    };
    // Fold revisions before resolving URLs: deleted/unavailable remote cards must not block listing.
    let mut records = std::collections::BTreeMap::new();
    for line in bytes.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        let v: Value = serde_json::from_slice(line)
            .map_err(|_| "Card store contains invalid JSON".to_string())?;
        let id = v["id"]
            .as_str()
            .ok_or("Card record missing id")?
            .to_string();
        if v["deleted"] == true {
            records.remove(&id);
        } else {
            records.insert(id, v);
        }
    }
    let mut result = Vec::new();
    for (id, v) in records {
        if selected.is_some_and(|selected| selected != id) { continue; }
        let source = v["source"].as_str().ok_or("Card record missing source")?;
        let auth: crate::auth::Auth = serde_json::from_value(
            v.get("auth")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({})),
        )
        .map_err(|_| "Invalid stored authentication")?;
        let raw = if let Some(card) = v.get("agentCard") {
            card.clone()
        } else if let Some(url) = v["cardUrl"].as_str() {
            let preview = preview_authenticated(url, None, &auth)?;
            preview
                .agents
                .into_iter()
                .next()
                .ok_or("URL did not return an Agent Card")?
                .raw
        } else {
            return Err("Card record missing agentCard or cardUrl".into());
        };
        let info = inspect_card(&raw)?;
        let item = StoredAgent {
            id: id.clone(),
            raw,
            info,
            source: source.into(),
            card_url: v["cardUrl"].as_str().map(str::to_owned),
            auth,
        };
        if let Some(old) = result.iter_mut().find(|a: &&mut StoredAgent| a.id == id) {
            *old = item;
        } else {
            result.push(item);
        }
    }
    Ok(result)
}
pub fn delete_agent(id: &str) -> Result<()> {
    if id.trim().is_empty() || id.len() > 256 {
        return Err("Invalid agent id".into());
    }
    let path = data_dir()?.join("agent-cards.jsonl");
    let mut file = fs::OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(|_| "Cannot open card store")?;
    let record = format!("{}\n", serde_json::json!({"id":id,"deleted":true}));
    file.write_all(record.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|_| "Cannot persist deletion".into())
}

fn read_catalog(dir: &Path) -> Result<Vec<StoredAgent>> {
    let path = dir.join("catalog.json");
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|_| "Local catalog corrupt; existing data was not changed".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(_) => Err("Cannot read local catalog".into()),
    }
}

pub fn import_agents(cards: Vec<Value>, source: &str) -> Result<Vec<StoredAgent>> {
    import_at(&data_dir()?, cards, source)
}
fn import_at(dir: &Path, cards: Vec<Value>, source: &str) -> Result<Vec<StoredAgent>> {
    if cards.is_empty() {
        return read_catalog(dir);
    }
    if cards.len() > 1000 {
        return Err("Too many cards".into());
    }
    // Validate whole selection before writing anything.
    let incoming = cards
        .into_iter()
        .map(|raw| {
            let info = inspect_card(&raw)?;
            let digest =
                Sha256::digest(format!("{}\0{}\0{}", source, info.endpoint, info.name).as_bytes());
            let id = format!("{:x}", digest);
            Ok(StoredAgent {
                id,
                raw,
                info,
                source: source.into(),
                card_url: None,
                auth: crate::auth::Auth::default(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    fs::create_dir_all(dir).map_err(|_| "Cannot create catalog directory")?;
    // Cross-process writer exclusion; no overwrites of concurrent installer/import operations.
    let lock_path = dir.join("catalog.lock");
    let _lock = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .map_err(|_| "Catalog busy (or stale catalog.lock after interrupted operation)")?;
    struct Lock(PathBuf);
    impl Drop for Lock {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    let _guard = Lock(lock_path);
    let mut existing = read_catalog(dir)?;
    let card_dir = dir.join("cards");
    fs::create_dir_all(&card_dir).map_err(|_| "Cannot create card directory")?;
    // Full cards use content-addressed revisions: old installed snapshots never silently change.
    for item in incoming {
        let bytes = serde_json::to_vec_pretty(&item.raw).map_err(|_| "Cannot serialize Card")?;
        let stable = card_dir.join(format!("{}.json", item.id));
        if stable.exists() {
            if fs::read(&stable).map_err(|_| "Cannot read cached Card")? != bytes {
                return Err(format!(
                    "Card {} changed; explicit update workflow is not implemented yet. Existing installed snapshot retained.",
                    item.info.name
                ));
            }
        } else {
            atomic_write(&stable, &bytes)?;
        }
        if let Some(old) = existing.iter_mut().find(|a| a.id == item.id) {
            *old = item;
        } else {
            existing.push(item);
        }
    }
    atomic_write(
        &dir.join("catalog.json"),
        &serde_json::to_vec_pretty(&existing).map_err(|_| "Cannot serialize catalog")?,
    )?;
    Ok(existing)
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    let result = (|| {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts
            .open(&tmp)
            .map_err(|_| "Cannot create temporary file")?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| "Cannot persist temporary file")?;
        fs::rename(&tmp, path).map_err(|_| "Cannot atomically replace local file")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}

pub use crate::install::{install_dsh, uninstall_dsh};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_is_noop() {
        assert!(parse_directory(b" \n").unwrap().empty);
        assert!(parse_directory(b"[]").unwrap().empty);
    }
    #[test]
    fn invalid_shape() {
        assert!(parse_directory(b"{}").is_err());
        assert!(parse_directory(b"no").is_err());
    }
    #[test]
    fn invalid_card_reported() {
        let p = parse_directory(b"[{}]").unwrap();
        assert_eq!(p.errors.len(), 1);
        assert!(!p.empty);
    }
}

//! Local catalog. Remote metadata is data, never executable configuration.
use crate::{CardInfo, Result, inspect_card};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
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
/// A local catalog record. Legacy URL records may have no saved Card metadata.
/// Reading this type never contacts the remote URL or invents CardInfo fields.
#[derive(Serialize, Clone)]
pub struct CatalogAgent {
    pub id: String,
    pub raw: Option<Value>,
    pub info: Option<CardInfo>,
    pub source: String,
    pub card_url: Option<String>,
    #[serde(skip_serializing)]
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
    let url = validate_card_url(url)?;
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

/// Shared by persistence, desktop validation and explicit remote refresh.
pub fn validate_card_url(raw: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(raw).map_err(|_| "Invalid source URL")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Use an HTTP(S) URL without credentials, query or fragment; supply credentials in Token".into());
    }
    Ok(url)
}

/// Validate the same bounded JSONL schema for every reader and writer, without HTTP.
pub fn validate_store_text(text: &str) -> Result<()> {
    parse_store_text(text).map(|_| ())
}

pub fn parse_store_text(text: &str) -> Result<std::collections::BTreeMap<String, Value>> {
    if text.len() > crate::catalog_store::MAX_STORE_BYTES {
        return Err("Configuration exceeds 8 MiB".into());
    }
    let mut records = std::collections::BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let check = || -> Result<(String, Value)> {
            let record: Value = serde_json::from_str(line).map_err(|_| "Invalid JSON")?;
            let id = record["id"]
                .as_str()
                .filter(|id| {
                    !id.trim().is_empty() && id.len() <= 256 && !id.chars().any(char::is_control)
                })
                .ok_or("Invalid or missing Agent id")?
                .to_owned();
            if record
                .get("deleted")
                .is_some_and(|value| !value.is_boolean())
            {
                return Err("deleted must be a boolean".into());
            }
            Ok((id, record))
        };
        let (id, record) = check().map_err(|error| format!("第 {} 行：{error}", index + 1))?;
        if record["deleted"] == true {
            records.remove(&id);
        } else {
            records.insert(id, (index + 1, record));
        }
    }
    // Historical revisions may already have been explicitly deleted or superseded.
    // Validate their JSON/id, and only validate the effective Card/auth payloads.
    records
        .into_iter()
        .map(|(id, (line, record))| {
            let check = || -> Result<()> {
                match record["source"].as_str() {
                    Some("manual") => {
                        if record.get("cardUrl").is_some_and(|value| !value.is_null()) {
                            return Err("Manual Agent cannot contain cardUrl".into());
                        }
                        inspect_card(&record["agentCard"])?;
                    }
                    Some("url") => {
                        validate_card_url(record["cardUrl"].as_str().ok_or("Missing cardUrl")?)?;
                        // Optional last-known Card metadata can coexist with a URL source.
                        if let Some(card) = record.get("agentCard") {
                            inspect_card(card)?;
                        }
                    }
                    _ => return Err("source must be url or manual".into()),
                }
                let auth: crate::auth::Auth = serde_json::from_value(
                    record
                        .get("auth")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({})),
                )
                .map_err(|_| "Invalid authentication")?;
                auth.headers()?;
                Ok(())
            };
            check().map_err(|error| format!("第 {line} 行：{error}"))?;
            Ok((id, record))
        })
        .collect()
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
    if id.trim().is_empty() || id.len() > 256 || id.chars().any(char::is_control) {
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
    let record = json_record(&item, card_url);
    crate::catalog_store::Store::at(data_dir()?).append_records(&[record])?;
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
    }
    record.insert("agentCard".into(), item.raw.clone());
    Value::Object(record)
}

/// List local records and saved metadata without checking remote availability.
pub fn list_agents() -> Result<Vec<CatalogAgent>> {
    read_agents(None)
}

/// Read the selected local record; the caller connects only this Agent when executing.
pub fn get_agent(id: &str) -> Result<CatalogAgent> {
    read_agents(Some(id))?
        .into_iter()
        .next()
        .ok_or("Agent not found in local catalog".into())
}

fn read_agents(selected: Option<&str>) -> Result<Vec<CatalogAgent>> {
    let text = crate::catalog_store::Store::at(data_dir()?).read_text()?;
    let records = parse_store_text(&text)?;
    catalog_records(records, selected)
}

fn catalog_records(
    records: std::collections::BTreeMap<String, Value>,
    selected: Option<&str>,
) -> Result<Vec<CatalogAgent>> {
    let mut result = Vec::new();
    for (id, v) in records {
        if selected.is_some_and(|selected| selected != id) {
            continue;
        }
        let source = v["source"].as_str().ok_or("Card record missing source")?;
        let auth: crate::auth::Auth = serde_json::from_value(
            v.get("auth")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({})),
        )
        .map_err(|_| "Invalid stored authentication")?;
        let raw = v.get("agentCard").cloned();
        let info = raw.as_ref().map(inspect_card).transpose()?;
        let item = CatalogAgent {
            id: id.clone(),
            raw,
            info,
            source: source.into(),
            card_url: v["cardUrl"].as_str().map(str::to_owned),
            auth,
        };
        result.push(item);
    }
    Ok(result)
}
pub fn delete_agent(id: &str) -> Result<()> {
    if id.trim().is_empty() || id.len() > 256 || id.chars().any(char::is_control) {
        return Err("Invalid agent id".into());
    }
    crate::catalog_store::Store::at(data_dir()?)
        .append_records(&[serde_json::json!({"id":id,"deleted":true})])
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
    crate::catalog_store::atomic_write(path, bytes)
}

pub use crate::install::{install_dsh, uninstall_dsh};

#[cfg(test)]
mod tests {
    use super::*;
    fn manual() -> Value {
        serde_json::json!({"id":"fixture","source":"manual","agentCard":{
            "name":"Fixture","description":"test","version":"1","protocolVersion":"0.3.0",
            "url":"http://127.0.0.1:1/rpc","capabilities":{},"defaultInputModes":["text"],
            "defaultOutputModes":["text"],"skills":[]
        }})
    }
    #[test]
    fn local_catalog_keeps_uncached_url_records_without_fabricating_metadata() {
        let legacy = serde_json::json!({"id":"legacy-url","source":"url","cardUrl":"http://127.0.0.1:1/card","auth":{"bearerToken":"fixture-only-secret"}});
        let text = format!("{}\n{legacy}\n", manual());
        let records = parse_store_text(&text).unwrap();
        let agents = catalog_records(records.clone(), None).unwrap();
        assert_eq!(agents.len(), 2);
        let old = agents
            .iter()
            .find(|agent| agent.id == "legacy-url")
            .unwrap();
        assert!(old.info.is_none());
        assert!(old.raw.is_none());
        assert_eq!(old.card_url.as_deref(), Some("http://127.0.0.1:1/card"));
        let serialized = serde_json::to_value(old).unwrap();
        assert!(serialized["info"].is_null());
        assert!(serialized["raw"].is_null());
        assert!(serialized.get("auth").is_none());
        assert!(!serialized.to_string().contains("fixture-only-secret"));
        let selected = catalog_records(records, Some("fixture")).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].info.as_ref().unwrap().name, "Fixture");
    }

    #[test]
    fn url_serialization_preserves_validated_card_metadata_locally() {
        let raw = manual()["agentCard"].clone();
        let item = StoredAgent {
            id: "cached-url".into(),
            info: inspect_card(&raw).unwrap(),
            raw: raw.clone(),
            source: "url".into(),
            card_url: Some("http://127.0.0.1:1/card".into()),
            auth: crate::auth::Auth::default(),
        };
        let record = json_record(&item, item.card_url.clone());
        assert_eq!(record["agentCard"], raw);
        assert_eq!(record["cardUrl"], "http://127.0.0.1:1/card");
        let agents = catalog_records(parse_store_text(&record.to_string()).unwrap(), None).unwrap();
        assert_eq!(agents[0].raw.as_ref().unwrap(), &raw);
        assert_eq!(agents[0].info.as_ref().unwrap().name, "Fixture");
    }

    #[test]
    fn invalid_cached_card_is_rejected_without_remote_repair() {
        let record = serde_json::json!({"id":"bad-cache","source":"url","cardUrl":"http://127.0.0.1:1/card","agentCard":{}});
        assert!(parse_store_text(&record.to_string()).is_err());
    }
    #[test]
    fn shared_store_parser_accepts_whitespace_crlf_and_revisions() {
        let mut revised = manual();
        revised["agentCard"]["name"] = serde_json::json!("Updated");
        let text = format!(" \t\r\n{}\r\n\t\n{}\n", manual(), revised);
        validate_store_text(&text).unwrap();
        let records = parse_store_text(&text).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records["fixture"]["agentCard"]["name"], "Updated");
        let deleted = format!("{text}{{\"id\":\"fixture\",\"deleted\":true}}\n");
        assert!(parse_store_text(&deleted).unwrap().is_empty());
    }
    #[test]
    fn editor_and_runtime_reject_the_same_url_and_record_errors() {
        for url in [
            "https://example.test/card?token=private",
            "https://example.test/card#fragment",
            "https://user:private@example.test/card",
        ] {
            let record = serde_json::json!({"id":"url","source":"url","cardUrl":url});
            let error = validate_store_text(&record.to_string()).unwrap_err();
            assert!(error.contains("第 1 行"));
            assert!(!error.contains("private"));
        }
        let mut record = manual();
        record["source"] = serde_json::json!("unknown");
        assert!(validate_store_text(&record.to_string()).is_err());
        record = manual();
        record["cardUrl"] = serde_json::json!("https://example.test/card");
        assert!(validate_store_text(&record.to_string()).is_err());
        record = manual();
        record["id"] = serde_json::json!("line\nbreak");
        assert!(validate_store_text(&record.to_string()).is_err());
        record = manual();
        record["deleted"] = serde_json::json!("true");
        assert!(validate_store_text(&record.to_string()).is_err());
        record = manual();
        record["auth"] = serde_json::json!({"headers":{"Host":"private"}});
        assert!(validate_store_text(&record.to_string()).is_err());
    }
    #[test]
    fn explicitly_deleted_bad_payload_is_not_resolved() {
        assert!(
            parse_store_text(
                "{\"id\":\"old\",\"source\":\"broken\"}\n{\"id\":\"old\",\"deleted\":true}\n"
            )
            .unwrap()
            .is_empty()
        );
    }
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

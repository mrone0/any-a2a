//! Bounded DSH profile-patch installer. Never evaluates YAML tags or runs a shell.
//! JSON is a YAML subset accepted by the pinned DSH loader. General YAML is refused.
use crate::catalog::{data_dir, list_agents};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const HEADER: &str = "# any-a2a-managed-v1 ";
const PREFIX: &str = "any-a2a-";
const LIMIT: u64 = 4 * 1024 * 1024;
type Result<T> = std::result::Result<T, String>;
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn encoded(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("JSON Value serializes")
}

/// Configures providers and independent one-shot tool rows, but deliberately does
/// not grant tools to an unknown Agent Preset or claim a running installation.
pub fn install_dsh(ids: Vec<String>, config_path: &str) -> Result<Value> {
    if ids.is_empty() || ids.len() > 100 {
        return Err("Select 1..100 cached agents".into());
    }
    let mut unique = HashSet::new();
    if ids.iter().any(|id| {
        id.len() != 64 || !id.bytes().all(|b| b.is_ascii_hexdigit()) || !unique.insert(id)
    }) {
        return Err("Invalid or duplicate agent IDs".into());
    }
    let agents = list_agents()?;
    let dir = data_dir()?
        .canonicalize()
        .map_err(|_| "Cannot locate cached cards")?;
    let executable = locate_executable()?;
    let adapter = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("adapters/dsh/src/index.js")
        .canonicalize()
        .map_err(|_| "DSH adapter source is missing; keep adapters/dsh with this workspace")?;
    let mut rows = Vec::new();
    for id in ids {
        let agent = agents
            .iter()
            .find(|a| a.id == id)
            .ok_or("Selected agent is no longer in the catalog")?;
        crate::inspect_card(&agent.raw)?;
        let card = dir.join("cards").join(format!("{id}.json"));
        let bytes = read_regular(&card)?;
        let cached: Value =
            serde_json::from_slice(&bytes).map_err(|_| "Cached card is invalid JSON")?;
        if cached != agent.raw {
            return Err(
                "Cached card differs from catalog; re-import explicitly before installation".into(),
            );
        }
        let provider = format!("{PREFIX}{id}");
        rows.push(json!({"id": provider, "name": adapter, "config": {
            "cardFile": card, "providerName": provider, "executable": executable
        }}));
        rows.push(json!({"id": format!("{PREFIX}tool-{id}"), "name": "@deepseek-ai/dsh-tool-subagent", "config": {
            "provider": provider, "toolName": format!("a2a_{}", &id[..32]),
            "backgroundMode": "one-shot", "maxDepth": "provider-managed",
            "enableRunInBackground": false, "modelSelectionSettings": false
        }}));
    }
    let tool_ids: Vec<Value> = rows
        .iter()
        .skip(1)
        .step_by(2)
        .map(|r| r["id"].clone())
        .collect();
    let tool_names: Vec<Value> = rows
        .iter()
        .skip(1)
        .step_by(2)
        .map(|r| r["config"]["toolName"].clone())
        .collect();
    let backup = update(config_path, Some(json!({"insert": rows})))?;
    Ok(
        json!({"installed": false, "configured": true, "configPath": config_path, "backupPath": backup,
        "toolIds": tool_ids, "toolNames": tool_names,
        "message": "DSH provider and one-shot tool rows were written, not activated. Full automatic installation is not supported: verify this profile includes dsh-subagent and dsh-tool-subagent, compose the generated tool rows in your intended Agent Preset and grant the returned model-facing toolNames as appropriate, then restart DSH. No preset permissions were changed. This adapter targets DSH commit c291e7961a515f6d7af9304e7fd1d257929aef26; runtime/version compatibility has not been verified."}),
    )
}

pub fn uninstall_dsh(config_path: &str) -> Result<Value> {
    let backup = update(config_path, None)?;
    Ok(
        json!({"installed": false, "configured": false, "configPath": config_path, "backupPath": backup,
        "message": "Owned provider/tool rows removed (or already absent). Restart DSH. Cached cards and any manually configured Agent Preset references were retained."}),
    )
}

fn locate_executable() -> Result<PathBuf> {
    let candidates = if let Some(path) = std::env::var_os("ANY_A2A_EXECUTABLE") {
        vec![PathBuf::from(path)]
    } else {
        let name = if cfg!(windows) {
            "any-a2a.exe"
        } else {
            "any-a2a"
        };
        let mut paths = vec![];
        if let Ok(current) = std::env::current_exe() {
            if let Some(parent) = current.parent() {
                paths.push(parent.join(name));
            }
        }
        paths.push(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target/release")
                .join(name),
        );
        paths
    };
    for candidate in candidates {
        if let Ok(path) = candidate.canonicalize() {
            if let Ok(meta) = fs::metadata(&path) {
                if !meta.is_file() {
                    continue;
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if meta.permissions().mode() & 0o111 == 0 {
                        continue;
                    }
                }
                return Ok(path);
            }
        }
    }
    Err("Rust CLI executable missing or not executable. Build cargo build --release, or set ANY_A2A_EXECUTABLE to its trusted path. No command was executed.".into())
}

fn read_regular(path: &Path) -> Result<Vec<u8>> {
    let meta = fs::symlink_metadata(path).map_err(|_| "Cannot read selected file")?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > LIMIT {
        return Err("Expected a regular, non-symlink file of at most 4 MiB".into());
    }
    fs::read(path).map_err(|_| "Cannot read selected file".into())
}

fn parse(bytes: &[u8]) -> Result<(Vec<Value>, bool)> {
    let text = std::str::from_utf8(bytes).map_err(|_| "Config must be UTF-8")?;
    let (digest, body) = if let Some(rest) = text.strip_prefix(HEADER) {
        let (digest, body) = rest
            .split_once('\n')
            .ok_or("Invalid installer ownership header")?;
        (Some(digest), body)
    } else {
        (None, text)
    };
    let mut entries: Vec<Value> = serde_json::from_str(body).map_err(|_| "No changes made: automatic editing supports only a JSON array in cordis.patch.yml (JSON is valid YAML). General YAML, comments and executable !!js tags are not parsed. Use a dedicated JSON-formatted DSH --patch overlay, initially [], or compose manually; not installed.")?;
    if let Some(digest) = digest {
        let last = entries
            .pop()
            .ok_or("Owned block missing; refusing to overwrite")?;
        if hash(&encoded(&last)) != digest {
            return Err("Owned entries were modified; refusing to overwrite or uninstall. Restore/reconcile the backup manually.".into());
        }
        if last
            .as_object()
            .is_none_or(|o| o.len() != 1 || !o.contains_key("insert"))
        {
            return Err("Invalid owned composition".into());
        }
    }
    if entries
        .iter()
        .any(|v| String::from_utf8_lossy(&encoded(v)).contains(PREFIX))
    {
        return Err("Unowned any-a2a entries or references conflict with installation; reconcile them manually".into());
    }
    Ok((entries, digest.is_some()))
}

fn render(mut entries: Vec<Value>, owned: Option<Value>) -> Vec<u8> {
    let header = owned
        .as_ref()
        .map(|v| format!("{HEADER}{}\n", hash(&encoded(v))))
        .unwrap_or_default();
    if let Some(owned) = owned {
        entries.push(owned);
    }
    format!(
        "{header}{}\n",
        serde_json::to_string_pretty(&entries).unwrap()
    )
    .into_bytes()
}
struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn create_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|_| "Cannot create installer backup/temporary/lock file")?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| "Cannot persist installer file".into())
}
fn update(config_path: &str, owned: Option<Value>) -> Result<Option<String>> {
    let requested = Path::new(config_path);
    if !requested.is_absolute() || requested.file_name().is_none() {
        return Err("Select an absolute existing DSH patch file path".into());
    }
    let parent = requested
        .parent()
        .unwrap()
        .canonicalize()
        .map_err(|_| "Config parent does not exist")?;
    let path = parent.join(requested.file_name().unwrap());
    let lock = path.with_file_name(format!(
        "{}.any-a2a.lock",
        path.file_name().unwrap().to_string_lossy()
    ));
    create_private(
        &lock,
        b"Installer in progress. Remove only after verifying no installer is running.\n",
    )?;
    let _lock = Cleanup(lock);
    let original = read_regular(&path)?;
    let (entries, was_owned) = parse(&original)?;
    if owned.is_none() && !was_owned {
        return Ok(None);
    }
    let next = render(entries, owned);
    if original == next {
        return Ok(None);
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "Clock error")?
        .as_nanos();
    let base = path.file_name().unwrap().to_string_lossy();
    let backup = parent.join(format!(
        "{base}.any-a2a-backup-{nonce}-{}",
        std::process::id()
    ));
    create_private(&backup, &original)?;
    let tmp = parent.join(format!("{base}.any-a2a-tmp-{nonce}-{}", std::process::id()));
    let _tmp = Cleanup(tmp.clone());
    create_private(&tmp, &next)?;
    fs::set_permissions(
        &tmp,
        fs::metadata(&path)
            .map_err(|_| "Config disappeared")?
            .permissions(),
    )
    .map_err(|_| "Cannot preserve config permissions")?;
    // Cooperative lock plus optimistic check. External editors must not write during
    // the final check/rename window; std has no portable compare-and-swap rename.
    if read_regular(&path)? != original {
        return Err(
            "Config changed concurrently; no replacement made. Retry after closing the editor."
                .into(),
        );
    }
    // All fallible preparation precedes commit. A failed rename retains original;
    // the byte-for-byte backup supports manual rollback after successful commit.
    fs::rename(&tmp, &path).map_err(|_| "Atomic config replacement failed; original retained")?;
    Ok(Some(backup.to_string_lossy().into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn block() -> Value {
        json!({"insert":[{"id":"any-a2a-test","name":"/trusted/index.js","config":{"cardFile":"/cards/test.json"}}]})
    }
    #[test]
    fn round_trip_and_modified_ownership() {
        let foreign = json!({"id":"existing","config":{"value":42}});
        let bytes = render(vec![foreign.clone()], Some(block()));
        let (entries, owned) = parse(&bytes).unwrap();
        assert!(owned);
        assert_eq!(entries, vec![foreign]);
        assert!(
            parse(
                &String::from_utf8(bytes)
                    .unwrap()
                    .replace("/cards/test.json", "/evil.json")
                    .into_bytes()
            )
            .is_err()
        );
    }
    #[test]
    fn refuses_yaml_and_conflicts() {
        for input in [
            "- insert: []",
            "!!js process.exit()",
            "{}",
            "[{\"id\":\"any-a2a-foreign\"}]",
        ] {
            assert!(parse(input.as_bytes()).is_err());
        }
        assert_eq!(parse(b"[]").unwrap(), (vec![], false));
    }
    #[test]
    fn backup_idempotency_uninstall_and_failure() {
        let dir = std::env::temp_dir().join(format!(
            "a2a-installer-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("cordis.patch.yml");
        fs::write(&path, b"[ ]\n").unwrap();
        let backup = update(path.to_str().unwrap(), Some(block()))
            .unwrap()
            .unwrap();
        assert_eq!(fs::read(backup).unwrap(), b"[ ]\n");
        assert!(
            update(path.to_str().unwrap(), Some(block()))
                .unwrap()
                .is_none()
        );
        update(path.to_str().unwrap(), None).unwrap();
        assert_eq!(parse(&fs::read(&path).unwrap()).unwrap(), (vec![], false));
        fs::write(&path, "- id: foreign\n").unwrap();
        assert!(update(path.to_str().unwrap(), Some(block())).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "- id: foreign\n");
        fs::remove_dir_all(dir).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn refuses_symlink() {
        use std::os::unix::fs::symlink;
        let path = std::env::temp_dir().join(format!("a2a-link-{}", std::process::id()));
        symlink("/dev/null", &path).unwrap();
        assert!(read_regular(&path).is_err());
        fs::remove_file(path).unwrap();
    }
}

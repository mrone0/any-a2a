//! Read-only local CLI discovery. Never executes discovered binaries or reads credentials.
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Client {
    id: &'static str,
    name: &'static str,
    executable: Option<String>,
    detected: bool,
    integration: &'static str,
}

fn executable(path: &Path) -> bool {
    let Ok(meta) = path.metadata() else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 == 0 {
            return false;
        }
    }
    true
}

#[tauri::command]
pub fn scan_clients() -> Vec<Client> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let home = PathBuf::from(home);
        for suffix in [
            ".local/bin",
            ".cargo/bin",
            ".bun/bin",
            ".opencode/bin",
            ".hermes/venv/bin",
        ] {
            dirs.push(home.join(suffix));
        }
    }
    #[cfg(target_os = "macos")]
    {
        dirs.push("/opt/homebrew/bin".into());
        dirs.push("/usr/local/bin".into());
    }
    #[cfg(windows)]
    if let Some(appdata) = std::env::var_os("APPDATA") {
        dirs.push(PathBuf::from(appdata).join("npm"));
    }
    let suffixes = if cfg!(windows) {
        vec![".exe", ".cmd", ".bat", ""]
    } else {
        vec![""]
    };
    [
        ("claude", "Claude Code"),
        ("codex", "Codex"),
        ("pi", "pi"),
        ("dsh", "DeepSeek Harness"),
        ("opencode", "OpenCode"),
        ("hermes", "Hermes"),
    ]
    .into_iter()
    .map(|(id, name)| {
        let found = dirs
            .iter()
            .flat_map(|dir| {
                suffixes
                    .iter()
                    .map(move |suffix| dir.join(format!("{id}{suffix}")))
            })
            .find(|path| executable(path));
        Client {
            id,
            name,
            detected: found.is_some(),
            executable: found.map(|p| p.to_string_lossy().into_owned()),
            integration: "not_verified",
        }
    })
    .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn six_clients_without_activation_claims() {
        let clients = super::scan_clients();
        assert_eq!(clients.len(), 6);
        for client in clients {
            assert_eq!(client.detected, client.executable.is_some());
            assert_eq!(client.integration, "not_verified");
        }
    }
}

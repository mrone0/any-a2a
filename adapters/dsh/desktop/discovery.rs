//! Bounded, read-only discovery of profile patches, independent of CLI/Pnpm launch mode.
use std::path::PathBuf;
#[tauri::command]
pub fn discover_dsh() -> Vec<String> {
    let mut homes = Vec::new();
    if let Some(home) = std::env::var_os("DSH_HOME") {
        homes.push(PathBuf::from(home));
    }
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        homes.push(PathBuf::from(home).join(".dsh"));
    }
    let mut candidates = Vec::new();
    for home in homes {
        // Prefer concrete profiles over the machine-wide layer.
        if let Ok(entries) = std::fs::read_dir(home.join("profiles")) {
            for entry in entries.flatten().take(100) {
                if entry.file_name() == "node_modules" {
                    continue;
                }
                if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                    continue;
                }
                candidates.push(entry.path().join("cordis.patch.yml"));
            }
        }
        candidates.push(home.join("cordis.patch.yml"));
    }
    let mut found: Vec<String> = candidates
        .into_iter()
        .filter(|p| p.is_file())
        .filter_map(|p| p.canonicalize().ok())
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    found.sort();
    found.dedup();
    found
}

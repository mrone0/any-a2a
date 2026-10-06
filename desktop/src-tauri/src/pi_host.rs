//! Invoke the installed Pi entry point directly, including npm installs on Windows.
use std::process::Command;

pub fn install_command() -> Result<Command, String> {
    #[cfg(not(windows))]
    {
        Ok(Command::new("pi"))
    }
    #[cfg(windows)]
    {
        let mut dirs: Vec<_> = std::env::var_os("PATH")
            .map(|value| std::env::split_paths(&value).collect())
            .unwrap_or_default();
        if let Some(appdata) = std::env::var_os("APPDATA") {
            dirs.push(std::path::PathBuf::from(appdata).join("npm"));
        }
        resolve(&dirs)
    }
}

#[cfg(windows)]
fn resolve(dirs: &[std::path::PathBuf]) -> Result<Command, String> {
    use std::{fs, path::Component};
    if let Some(executable) = dirs
        .iter()
        .map(|dir| dir.join("pi.exe"))
        .find(|path| path.is_file())
    {
        return Ok(Command::new(executable));
    }
    let node = dirs
        .iter()
        .map(|dir| dir.join("node.exe"))
        .find(|path| path.is_file())
        .ok_or("找不到 Node.js；请安装 Node.js 和 Pi 后重启桌面应用")?;
    for dir in dirs {
        let package = dir.join("node_modules/@earendil-works/pi-coding-agent");
        let metadata = package.join("package.json");
        let Ok(meta) = fs::metadata(&metadata) else {
            continue;
        };
        if meta.len() > 1024 * 1024 {
            continue;
        }
        let Ok(bytes) = fs::read(&metadata) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        if value["name"] != "@earendil-works/pi-coding-agent" {
            continue;
        }
        let Some(bin) = value["bin"]["pi"]
            .as_str()
            .or_else(|| value["bin"].as_str())
        else {
            continue;
        };
        let relative = std::path::Path::new(bin);
        if relative.is_absolute()
            || relative
                .components()
                .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
        {
            continue;
        }
        let Ok(package) = package.canonicalize() else {
            continue;
        };
        let Ok(entry) = package.join(relative).canonicalize() else {
            continue;
        };
        if !entry.starts_with(&package) || !entry.is_file() {
            continue;
        }
        let mut command = Command::new(node.clone());
        command.arg(entry);
        return Ok(command);
    }
    Err(
        "找不到 Pi 的原生程序或 npm 包；请安装 @earendil-works/pi-coding-agent 后重启桌面应用"
            .into(),
    )
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn npm_entry_uses_native_node_and_preserves_paths_with_spaces() {
        let root = std::env::temp_dir().join(format!("a2a pi host {}", uuid::Uuid::new_v4()));
        let package = root.join("node_modules/@earendil-works/pi-coding-agent");
        fs::create_dir_all(package.join("dist/bundle")).unwrap();
        fs::write(root.join("node.exe"), b"fixture").unwrap();
        fs::write(package.join("dist/bundle/cli.js"), b"fixture").unwrap();
        fs::write(
            package.join("package.json"),
            br#"{"name":"@earendil-works/pi-coding-agent","bin":{"pi":"dist/bundle/cli.js"}}"#,
        )
        .unwrap();
        let command = resolve(std::slice::from_ref(&root)).unwrap();
        assert_eq!(command.get_program(), root.join("node.exe"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![package
                .join("dist/bundle/cli.js")
                .canonicalize()
                .unwrap()
                .as_os_str()]
        );
        fs::write(
            package.join("package.json"),
            br#"{"name":"@earendil-works/pi-coding-agent","bin":{"pi":"../../outside.js"}}"#,
        )
        .unwrap();
        assert!(resolve(std::slice::from_ref(&root)).is_err());
        let absolute = root.canonicalize().unwrap();
        assert!(absolute.starts_with(std::env::temp_dir().canonicalize().unwrap()));
        fs::remove_dir_all(absolute).unwrap();
    }
}

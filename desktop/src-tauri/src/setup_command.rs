//! Shell-specific quoting for display-only setup commands. Never executed by the app.
pub fn pi_command(executable: &str, data_dir: &str, extension: &str, windows: bool) -> (&'static str, String) {
    let quote = |value: &str| {
        let escaped = if windows { value.replace('\'', "''") } else { value.replace('\'', "'\"'\"'") };
        format!("'{escaped}'")
    };
    if windows {
        ("PowerShell", format!("$env:ANY_A2A_EXECUTABLE = {}\n$env:ANY_A2A_DATA_DIR = {}\npi -e {}", quote(executable), quote(data_dir), quote(extension)))
    } else {
        ("bash / zsh", format!("ANY_A2A_EXECUTABLE={} ANY_A2A_DATA_DIR={} pi -e {}", quote(executable), quote(data_dir), quote(extension)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_quotes_paths_without_expansion() {
        let (shell, command) = pi_command("C:\\O'Brien\\any-a2a.exe", "C:\\数据 $dir", "C:\\pi extension.ts", true);
        assert_eq!(shell, "PowerShell");
        assert!(command.contains("'C:\\O''Brien\\any-a2a.exe'"));
        assert!(command.contains("'C:\\数据 $dir'"));
    }
    #[test]
    fn posix_quotes_paths_without_expansion() {
        let (shell, command) = pi_command("/opt/O'Brien/any-a2a", "/tmp/$data", "/tmp/扩展 file.ts", false);
        assert_eq!(shell, "bash / zsh");
        assert_eq!(command, "ANY_A2A_EXECUTABLE='/opt/O'\"'\"'Brien/any-a2a' ANY_A2A_DATA_DIR='/tmp/$data' pi -e '/tmp/扩展 file.ts'");
    }
}

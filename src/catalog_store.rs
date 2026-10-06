//! Bounded, cooperating-process storage for the local Agent Card JSONL file.
//! The permanent sidecar lock survives snapshot replacement; OS locks do not
//! need stale-file deletion after a crashed owner.
use crate::Result;
use serde_json::Value;
use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub const MAX_STORE_BYTES: usize = 8 * 1024 * 1024;
const LOCK_TIMEOUT: Duration = Duration::from_millis(500);
const STORE_NAME: &str = "agent-cards.jsonl";

pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn at(dir: impl AsRef<Path>) -> Self {
        Self {
            dir: dir.as_ref().to_path_buf(),
        }
    }

    fn path(&self) -> PathBuf {
        self.dir.join(STORE_NAME)
    }

    fn lock(&self, shared: bool) -> Result<LockGuard> {
        fs::create_dir_all(&self.dir).map_err(|_| "Cannot create card store directory")?;
        LockGuard::acquire(&self.dir.join(format!("{STORE_NAME}.lock")), shared)
    }

    pub fn read_text(&self) -> Result<String> {
        let _lock = self.lock(true)?;
        read_text_unlocked(&self.path())
    }

    /// Validate every new record, then replace a full bounded snapshot under one
    /// exclusive lock. A tombstone may intentionally remove a bad old record.
    pub fn append_records(&self, records: &[Value]) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        let mut additions = BoundedBytes::default();
        for record in records {
            let mut encoded = BoundedBytes::default();
            serde_json::to_writer(&mut encoded, record)
                .map_err(|_| "Card record exceeds configuration limit")?;
            let text = std::str::from_utf8(&encoded.bytes)
                .map_err(|_| "Card record is not valid UTF-8")?;
            crate::catalog::validate_store_text(text)?;
            additions
                .write_all(&encoded.bytes)
                .and_then(|_| additions.write_all(b"\n"))
                .map_err(|_| "Configuration exceeds 8 MiB")?;
        }
        let additions =
            String::from_utf8(additions.bytes).map_err(|_| "Card records are not valid UTF-8")?;
        let _lock = self.lock(false)?;
        let path = self.path();
        let original = read_text_unlocked(&path)?;
        let separator = usize::from(!original.is_empty() && !original.ends_with('\n'));
        if original.len() + separator + additions.len() > MAX_STORE_BYTES {
            return Err("Configuration exceeds 8 MiB".into());
        }
        let mut next = String::with_capacity(original.len() + separator + additions.len());
        next.push_str(&original);
        if separator != 0 {
            next.push('\n');
        }
        next.push_str(&additions);
        crate::catalog::validate_store_text(&next)?;
        atomic_write_checked(&path, next.as_bytes(), || check_expected(&path, &original))
    }

    /// Preserve the exact previous bytes before an explicit editor replacement.
    /// The lock coordinates our writers. A final comparison also detects edits
    /// by non-cooperating programs before that check, but is not a portable CAS.
    pub fn replace_text(&self, expected: &str, content: &str) -> Result<PathBuf> {
        if content.len() > MAX_STORE_BYTES || expected.len() > MAX_STORE_BYTES {
            return Err("Configuration exceeds 8 MiB".into());
        }
        crate::catalog::validate_store_text(content)?;
        let separator = usize::from(!content.is_empty() && !content.ends_with('\n'));
        if content.len() + separator > MAX_STORE_BYTES {
            return Err("Configuration exceeds 8 MiB".into());
        }
        let mut next = String::with_capacity(content.len() + separator);
        next.push_str(content);
        if separator != 0 {
            next.push('\n');
        }
        let _lock = self.lock(false)?;
        let path = self.path();
        check_expected(&path, expected)?;
        let backup = self
            .dir
            .join(format!("{STORE_NAME}.backup-{}", uuid::Uuid::new_v4()));
        // A backup is deliberately retained, including if a later commit fails.
        write_backup(&backup, expected.as_bytes())?;
        atomic_write_checked(&path, next.as_bytes(), || check_expected(&path, expected))?;
        Ok(backup)
    }
}

struct LockGuard(File);

impl LockGuard {
    fn acquire(path: &Path, shared: bool) -> Result<Self> {
        match fs::symlink_metadata(path) {
            Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => {
                return Err("Expected a regular card store lock file".into());
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return Err("Cannot inspect card store lock".into()),
        }
        let mut options = private_options();
        options.read(true).write(true).create(true).truncate(false);
        let file = options
            .open(path)
            .map_err(|_| "Cannot open card store lock")?;
        if !file
            .metadata()
            .map_err(|_| "Cannot inspect card store lock")?
            .is_file()
        {
            return Err("Expected a regular card store lock file".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|_| "Cannot protect card store lock")?;
        }
        let deadline = Instant::now() + LOCK_TIMEOUT;
        loop {
            let attempt = if shared {
                file.try_lock_shared()
            } else {
                file.try_lock()
            };
            match attempt {
                Ok(()) => return Ok(Self(file)),
                Err(TryLockError::WouldBlock) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err("Card store busy; no changes made".into());
                    }
                    std::thread::sleep(Duration::from_millis(10).min(remaining));
                }
                Err(TryLockError::Error(_)) => return Err("Cannot lock card store".into()),
            }
        }
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = self.0.unlock();
        // Keep the sidecar: deleting it could split future owners across inodes.
    }
}

fn read_text_unlocked(path: &Path) -> Result<String> {
    match fs::symlink_metadata(path) {
        Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => {
            return Err("Expected a regular card store file".into());
        }
        Ok(meta) if meta.len() > MAX_STORE_BYTES as u64 => {
            return Err("Configuration exceeds 8 MiB".into());
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(String::new()),
        Err(_) => return Err("Cannot inspect card store".into()),
    }
    let file = File::open(path).map_err(|_| "Cannot read card store")?;
    if !file
        .metadata()
        .map_err(|_| "Cannot inspect card store")?
        .is_file()
    {
        return Err("Expected a regular card store file".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_STORE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Cannot read card store")?;
    if bytes.len() > MAX_STORE_BYTES {
        return Err("Configuration exceeds 8 MiB".into());
    }
    String::from_utf8(bytes).map_err(|_| "Configuration must be UTF-8".into())
}

fn check_expected(path: &Path, expected: &str) -> Result<()> {
    if read_text_unlocked(path)? != expected {
        return Err("Configuration changed concurrently; reload before saving".into());
    }
    Ok(())
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

fn write_backup(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = private_options();
    options.write(true).create_new(true);
    let mut file = options
        .open(path)
        .map_err(|_| "Cannot create configuration backup")?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| "Cannot persist configuration backup".into())
}

struct TemporaryFile {
    path: PathBuf,
    file: Option<File>,
}

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        // Close first on all failure paths as well as before the Windows rename.
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}

/// Atomically replace a bounded file using a private, uniquely owned temporary
/// file. Callers coordinate read/modify/write transactions with their own lock.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    atomic_write_checked(path, bytes, || Ok(()))
}

fn atomic_write_checked(
    path: &Path,
    bytes: &[u8],
    before_commit: impl FnOnce() -> Result<()>,
) -> Result<()> {
    if bytes.len() > MAX_STORE_BYTES {
        return Err("Configuration exceeds 8 MiB".into());
    }
    let name = path.file_name().ok_or("Local file requires a file name")?;
    let temporary = path.with_file_name(format!(
        ".{}.tmp-{}",
        name.to_string_lossy(),
        uuid::Uuid::new_v4()
    ));
    let mut options = private_options();
    options.write(true).create_new(true);
    // Cleanup is established only after we successfully create our own file.
    let file = options
        .open(&temporary)
        .map_err(|_| "Cannot create temporary file")?;
    let mut temporary = TemporaryFile {
        path: temporary,
        file: Some(file),
    };
    let file = temporary.file.as_mut().expect("new temporary file is open");
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| "Cannot persist temporary file")?;
    drop(temporary.file.take());
    before_commit()?;
    fs::rename(&temporary.path, path).map_err(|_| "Cannot atomically replace local file")?;
    Ok(())
}

#[derive(Default)]
struct BoundedBytes {
    bytes: Vec<u8>,
}

impl Write for BoundedBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_STORE_BYTES.saturating_sub(self.bytes.len()) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "size limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{
        io::{BufRead, BufReader},
        process::{Child, Command, Stdio},
    };

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("any-a2a-store-test-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let target = fs::canonicalize(&self.0).unwrap();
            let root = fs::canonicalize(std::env::temp_dir()).unwrap();
            assert!(target.starts_with(root));
            assert!(
                target
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("any-a2a-store-test-")
            );
            fs::remove_dir_all(target).unwrap();
        }
    }

    fn record(id: &str) -> Value {
        json!({"id":id,"source":"manual","agentCard":{
            "name":"fixture","description":"fixture","version":"1",
            "protocolVersion":"0.3.0","preferredTransport":"JSONRPC",
            "url":"http://127.0.0.1:9/rpc","capabilities":{},
            "defaultInputModes":["text/plain"],"defaultOutputModes":["text/plain"],"skills":[]
        }})
    }

    fn worker(dir: &Path, action: &str, prefix: &str) -> Child {
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "catalog_store::tests::process_worker",
                "--ignored",
                "--nocapture",
            ])
            .env("ANY_A2A_STORE_TEST_DIR", dir)
            .env("ANY_A2A_STORE_TEST_ACTION", action)
            .env("ANY_A2A_STORE_TEST_PREFIX", prefix)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    }

    fn signal_ready() {
        println!("STORE_LOCK_READY");
        io::stdout().flush().unwrap();
    }

    fn wait_until_ready(child: &mut Child) {
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        loop {
            let mut line = String::new();
            assert_ne!(stdout.read_line(&mut line).unwrap(), 0);
            if line.trim() == "STORE_LOCK_READY" {
                break;
            }
        }
        child.stdout = Some(stdout.into_inner());
    }

    #[test]
    #[ignore = "isolated child process helper"]
    fn process_worker() {
        let dir = PathBuf::from(std::env::var_os("ANY_A2A_STORE_TEST_DIR").unwrap());
        let store = Store::at(&dir);
        match std::env::var("ANY_A2A_STORE_TEST_ACTION").unwrap().as_str() {
            action @ ("append" | "append-wait") => {
                if action == "append-wait" {
                    signal_ready();
                    let mut line = String::new();
                    io::stdin().read_line(&mut line).unwrap();
                }
                let prefix = std::env::var("ANY_A2A_STORE_TEST_PREFIX").unwrap();
                let records: Vec<_> = (0..8).map(|i| record(&format!("{prefix}-{i}"))).collect();
                store.append_records(&records).unwrap();
            }
            "hold" => {
                let _lock = store.lock(false).unwrap();
                signal_ready();
                let mut line = String::new();
                io::stdin().read_line(&mut line).unwrap();
            }
            _ => panic!("unknown isolated test action"),
        }
    }

    #[test]
    fn cross_process_appends_preserve_all_records() {
        let dir = TestDir::new();
        let mut children: Vec<_> = (0..4)
            .map(|i| worker(&dir.0, "append-wait", &format!("child-{i}")))
            .collect();
        for child in &mut children {
            wait_until_ready(child);
        }
        for child in &mut children {
            child.stdin.as_mut().unwrap().write_all(b"start\n").unwrap();
        }
        for child in children {
            let result = child.wait_with_output().unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let text = Store::at(&dir.0).read_text().unwrap();
        crate::catalog::validate_store_text(&text).unwrap();
        let ids: std::collections::HashSet<_> = text
            .lines()
            .map(|line| {
                serde_json::from_str::<Value>(line).unwrap()["id"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(ids.len(), 32);
    }

    #[test]
    fn stale_editor_rejected_and_backup_is_exact() {
        let dir = TestDir::new();
        let store = Store::at(&dir.0);
        let mut old = record("old");
        old["auth"] = json!({"bearerToken":"isolated-fixture-only"});
        let original = format!(" {old} \r\n\r\n");
        fs::write(store.path(), &original).unwrap();
        let child = worker(&dir.0, "append", "other");
        assert!(child.wait_with_output().unwrap().status.success());
        let current = store.read_text().unwrap();
        assert!(store.replace_text(&original, "").is_err());
        assert_eq!(store.read_text().unwrap(), current);
        let backup = store
            .replace_text(&current, &record("edited").to_string())
            .unwrap();
        assert_eq!(fs::read(backup).unwrap(), current.as_bytes());
        assert!(store.read_text().unwrap().ends_with('\n'));
    }

    #[test]
    fn killed_owner_releases_lock_without_deleting_sidecar() {
        let dir = TestDir::new();
        let store = Store::at(&dir.0);
        let mut child = worker(&dir.0, "hold", "");
        wait_until_ready(&mut child);
        let started = Instant::now();
        assert!(store.append_records(&[record("blocked")]).is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(!store.path().exists());
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(dir.0.join(format!("{STORE_NAME}.lock")).is_file());
        store.append_records(&[record("after-kill")]).unwrap();
    }

    #[test]
    fn invalid_and_oversized_updates_keep_original() {
        let dir = TestDir::new();
        let store = Store::at(&dir.0);
        store.append_records(&[record("original")]).unwrap();
        let original = store.read_text().unwrap();
        assert!(
            store
                .append_records(&[json!({"id":"bad","source":"manual","agentCard":{}})])
                .is_err()
        );
        assert!(store.replace_text(&original, "{broken}").is_err());
        let large = " ".repeat(MAX_STORE_BYTES + 1);
        assert!(store.replace_text(&original, &large).is_err());
        assert!(atomic_write(&store.path(), large.as_bytes()).is_err());
        assert_eq!(store.read_text().unwrap(), original);
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 2);
        fs::write(store.path(), large.as_bytes()).unwrap();
        assert!(store.read_text().is_err());
        assert!(store.append_records(&[record("later")]).is_err());
        assert_eq!(
            fs::metadata(store.path()).unwrap().len(),
            large.len() as u64
        );
    }

    #[test]
    fn append_supplies_missing_eof_newline() {
        let dir = TestDir::new();
        let store = Store::at(&dir.0);
        fs::write(store.path(), record("first").to_string()).unwrap();
        store.append_records(&[record("second")]).unwrap();
        let text = store.read_text().unwrap();
        assert_eq!(text.lines().count(), 2);
        crate::catalog::validate_store_text(&text).unwrap();
    }

    #[test]
    fn failed_commit_check_keeps_original_and_cleans_only_owned_temp() {
        let dir = TestDir::new();
        let path = dir.0.join("settings.json");
        fs::write(&path, b"original").unwrap();
        let foreign = dir.0.join(".settings.json.tmp-foreign");
        fs::write(&foreign, b"other writer").unwrap();
        let backup = dir.0.join("settings.backup");
        write_backup(&backup, b"original").unwrap();
        assert!(
            atomic_write_checked(&path, b"next", || {
                fs::write(&path, b"external editor").unwrap();
                check_expected(&path, "original")
            })
            .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), b"external editor");
        assert_eq!(fs::read(&backup).unwrap(), b"original");
        assert_eq!(fs::read(&foreign).unwrap(), b"other writer");
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 3);
        atomic_write(&path, b"next").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"next");
    }

    #[cfg(unix)]
    #[test]
    fn store_lock_and_backup_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TestDir::new();
        let store = Store::at(&dir.0);
        store.append_records(&[record("first")]).unwrap();
        let original = store.read_text().unwrap();
        let backup = store.replace_text(&original, "").unwrap();
        for path in [
            store.path(),
            dir.0.join(format!("{STORE_NAME}.lock")),
            backup,
        ] {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}

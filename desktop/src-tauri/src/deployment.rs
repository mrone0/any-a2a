//! Source-independent, bounded deployment of embedded adapter packages.
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

const MAX_VERSIONS: usize = 32;
const MAX_EXECUTABLE_BYTES: u64 = 256 * 1024 * 1024;
type Asset = (&'static str, &'static [u8]);

const COMMON: &[Asset] = &[
    (
        "adapters/common/src/cli-transport.js",
        include_bytes!("../../../adapters/common/src/cli-transport.js"),
    ),
    (
        "adapters/common/package.json",
        include_bytes!("../../../adapters/common/package.json"),
    ),
    (
        "adapters/common/src/service-client.js",
        include_bytes!("../../../adapters/common/src/service-client.js"),
    ),
];
const PI: &[Asset] = &[
    (
        "adapters/pi/package.json",
        include_bytes!("../../../adapters/pi/package.json"),
    ),
    (
        "adapters/pi/index.ts",
        include_bytes!("../../../adapters/pi/index.ts"),
    ),
    (
        "adapters/pi/transport.mjs",
        include_bytes!("../../../adapters/pi/transport.mjs"),
    ),
    (
        "adapters/pi/runs.mjs",
        include_bytes!("../../../adapters/pi/runs.mjs"),
    ),
    (
        "adapters/pi/lifecycle.mjs",
        include_bytes!("../../../adapters/pi/lifecycle.mjs"),
    ),
    (
        "adapters/pi/capabilities.mjs",
        include_bytes!("../../../adapters/pi/capabilities.mjs"),
    ),
    (
        "adapters/pi/presentation.mjs",
        include_bytes!("../../../adapters/pi/presentation.mjs"),
    ),
    (
        "adapters/pi/delivery.mjs",
        include_bytes!("../../../adapters/pi/delivery.mjs"),
    ),
];
const DSH: &[Asset] = &[
    (
        "adapters/dsh/package.json",
        include_bytes!("../../../adapters/dsh/package.json"),
    ),
    (
        "adapters/dsh/cordis.patch.yml",
        include_bytes!("../../../adapters/dsh/cordis.patch.yml"),
    ),
    (
        "adapters/dsh/src/index.js",
        include_bytes!("../../../adapters/dsh/src/index.js"),
    ),
    (
        "adapters/dsh/src/index.d.ts",
        include_bytes!("../../../adapters/dsh/src/index.d.ts"),
    ),
    (
        "adapters/dsh/src/delegation-tool.js",
        include_bytes!("../../../adapters/dsh/src/delegation-tool.js"),
    ),
    (
        "adapters/dsh/src/remote-session.js",
        include_bytes!("../../../adapters/dsh/src/remote-session.js"),
    ),
    (
        "adapters/dsh/src/progress.js",
        include_bytes!("../../../adapters/dsh/src/progress.js"),
    ),
    (
        "adapters/dsh/src/capabilities.js",
        include_bytes!("../../../adapters/dsh/src/capabilities.js"),
    ),
];

fn executable_name() -> String {
    format!("any-a2a{}", std::env::consts::EXE_SUFFIX)
}

fn validate_executable(path: &Path) -> Result<(), String> {
    let meta = fs::metadata(path).map_err(|_| "Cannot inspect native any-a2a CLI")?;
    if !meta.is_file() {
        return Err("Native any-a2a CLI must be a regular file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o111 == 0 {
            return Err("Native any-a2a CLI is not executable".into());
        }
    }
    Ok(())
}

fn resolve_executable(
    explicit: Option<&Path>,
    desktop_exe: Option<&Path>,
    source_root: Option<&Path>,
) -> Result<PathBuf, String> {
    // Invalid explicit deployment settings must not silently select another CLI.
    if let Some(path) = explicit {
        validate_executable(path)?;
        return path
            .canonicalize()
            .map_err(|_| "Cannot resolve native CLI".into());
    }
    let name = executable_name();
    let bundled = desktop_exe
        .and_then(Path::parent)
        .map(|parent| parent.join(&name));
    let mut candidates = bundled.into_iter().collect::<Vec<_>>();
    if let Some(root) = source_root {
        candidates.push(root.join("target/debug").join(&name));
        candidates.push(root.join("target/release").join(&name));
    }
    for candidate in candidates {
        if validate_executable(&candidate).is_ok() {
            return candidate
                .canonicalize()
                .map_err(|_| "Cannot resolve native CLI".into());
        }
    }
    Err(
        "Native any-a2a CLI unavailable; reinstall the desktop app or set ANY_A2A_EXECUTABLE"
            .into(),
    )
}

/// Explicit environment setting, then installed sidecar; source fallbacks are debug-only.
pub fn executable() -> Result<PathBuf, String> {
    let explicit = std::env::var_os("ANY_A2A_EXECUTABLE").map(PathBuf::from);
    let desktop_exe = std::env::current_exe().ok();
    #[cfg(debug_assertions)]
    let source_root = Some(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."));
    #[cfg(not(debug_assertions))]
    let source_root: Option<PathBuf> = None;
    resolve_executable(
        explicit.as_deref(),
        desktop_exe.as_deref(),
        source_root.as_deref(),
    )
}

fn assets(kind: &str) -> Result<&'static [Asset], String> {
    match kind {
        "pi" => Ok(PI),
        "dsh" => Ok(DSH),
        _ => Err("Unsupported adapter deployment; expected pi or dsh".into()),
    }
}

fn file_digest(path: &Path) -> Result<[u8; 32], String> {
    let mut file = fs::File::open(path).map_err(|_| "Cannot read native CLI")?;
    let mut hash = Sha256::new();
    let mut total = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| "Cannot read native CLI")?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > MAX_EXECUTABLE_BYTES {
            return Err("Native CLI exceeds 256 MiB".into());
        }
        hash.update(&buffer[..count]);
    }
    Ok(hash.finalize().into())
}

fn copy_executable(source: &Path, destination: &Path) -> Result<(), String> {
    let input = fs::File::open(source).map_err(|_| "Cannot read native CLI for deployment")?;
    let permissions = input
        .metadata()
        .map_err(|_| "Cannot inspect native CLI permissions")?
        .permissions();
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|_| "Cannot create deployed native CLI")?;
    let length = std::io::copy(&mut input.take(MAX_EXECUTABLE_BYTES + 1), &mut output)
        .map_err(|_| "Cannot copy native CLI into adapter package")?;
    if length > MAX_EXECUTABLE_BYTES {
        return Err("Native CLI exceeds 256 MiB".into());
    }
    output
        .sync_all()
        .map_err(|_| "Cannot persist deployed native CLI")?;
    // Preserve executable/read-only permissions after flushing through the writable handle.
    fs::set_permissions(destination, permissions)
        .map_err(|_| "Cannot preserve native CLI permissions")?;
    Ok(())
}

fn deployment_key(kind: &str, entries: &[Asset], cli_digest: &[u8; 32]) -> String {
    let mut hash = Sha256::new();
    hash.update(b"any-a2a-deployment-v1\0");
    hash.update(kind.as_bytes());
    hash.update([0]);
    for (name, bytes) in COMMON.iter().chain(entries) {
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name.as_bytes());
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    hash.update(cli_digest);
    format!("{}-{:x}", env!("CARGO_PKG_VERSION"), hash.finalize())
}

fn verify_deployment(
    version: &Path,
    kind: &str,
    entries: &[Asset],
    cli_digest: &[u8; 32],
) -> Result<PathBuf, String> {
    if !fs::symlink_metadata(version)
        .is_ok_and(|meta| meta.is_dir() && !meta.file_type().is_symlink())
    {
        return Err("Deployed adapter version is not a regular directory".into());
    }
    for (name, bytes) in COMMON.iter().chain(entries) {
        let path = version.join(name);
        let meta = fs::symlink_metadata(&path).map_err(|_| "Deployed adapter file missing")?;
        if !meta.is_file()
            || meta.len() != bytes.len() as u64
            || fs::read(&path).map_err(|_| "Cannot verify deployed adapter")? != *bytes
        {
            return Err("Deployed adapter differs from its embedded version; preserve it and inspect the deployment directory".into());
        }
    }
    let package = version.join("adapters").join(kind);
    let cli = package.join("bin").join(executable_name());
    validate_executable(&cli)?;
    if file_digest(&cli)? != *cli_digest {
        return Err("Deployed CLI differs from its version; preserve it and inspect the deployment directory".into());
    }
    Ok(package)
}

fn remove_staging(staging: &Path, versions: &Path) {
    // Only a generated, direct child of the already canonicalized versions root is removed.
    let safe_name = staging
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with(".staging-"));
    let safe_target = staging.parent() == Some(versions)
        && safe_name
        && fs::symlink_metadata(staging)
            .is_ok_and(|meta| meta.is_dir() && !meta.file_type().is_symlink())
        && staging
            .canonicalize()
            .is_ok_and(|target| target.parent() == Some(versions));
    if safe_target {
        let _ = fs::remove_dir_all(staging);
    }
}

struct DeploymentLock {
    _file: fs::File,
}
impl DeploymentLock {
    fn acquire(versions: &Path) -> Result<Self, String> {
        let path = versions.join(".deployment.lock");
        if path
            .try_exists()
            .map_err(|_| "Cannot inspect deployment lock")?
            && !fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_file())
        {
            return Err("Deployment lock must be a regular file".into());
        }
        let file = fs::OpenOptions::new()
            .write(true)
            .read(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|_| "Cannot open adapter deployment lock")?;
        file.try_lock()
            .map_err(|_| "Adapter deployment busy; retry after the current install finishes")?;
        Ok(Self { _file: file })
    }
}

/// Publish a complete embedded package and CLI atomically, reusing matching versions.
/// Existing versions are never replaced or removed, because clients may still reference them.
pub fn stage(kind: &str, data: &Path, executable: &Path) -> Result<PathBuf, String> {
    let entries = assets(kind)?;
    validate_executable(executable)?;
    let cli_digest = file_digest(executable)?;
    let versions = data.join("integrations").join(kind).join("versions");
    fs::create_dir_all(&versions).map_err(|_| "Cannot create adapter versions directory")?;
    let versions = versions
        .canonicalize()
        .map_err(|_| "Cannot resolve adapter versions directory")?;
    let _lock = DeploymentLock::acquire(&versions)?;
    let destination = versions.join(deployment_key(kind, entries, &cli_digest));
    if destination
        .try_exists()
        .map_err(|_| "Cannot inspect adapter deployment")?
    {
        return verify_deployment(&destination, kind, entries, &cli_digest);
    }
    let count = fs::read_dir(&versions)
        .map_err(|_| "Cannot inspect adapter versions")?
        .filter_map(Result::ok)
        .filter(|entry| {
            !entry.file_name().to_string_lossy().starts_with(".staging-")
                && entry.file_type().is_ok_and(|kind| kind.is_dir())
        })
        .take(MAX_VERSIONS)
        .count();
    if count >= MAX_VERSIONS {
        return Err(format!("Adapter deployment limit ({MAX_VERSIONS} versions) reached; stop clients and archive versions you no longer reference"));
    }
    let staging = versions.join(format!(".staging-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&staging).map_err(|_| "Cannot create adapter staging directory")?;
    let result = (|| {
        for (name, bytes) in COMMON.iter().chain(entries) {
            let path = staging.join(name);
            fs::create_dir_all(path.parent().ok_or("Invalid embedded adapter path")?)
                .map_err(|_| "Cannot create adapter package directory")?;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map_err(|_| "Cannot create embedded adapter file")?;
            file.write_all(bytes)
                .and_then(|_| file.sync_all())
                .map_err(|_| "Cannot persist embedded adapter file")?;
        }
        let bin = staging.join("adapters").join(kind).join("bin");
        fs::create_dir(&bin).map_err(|_| "Cannot create adapter bin directory")?;
        let cli = bin.join(executable_name());
        copy_executable(executable, &cli)?;
        verify_deployment(&staging, kind, entries, &cli_digest)?;
        if fs::rename(&staging, &destination).is_err() {
            // Another installer may have published the same complete content concurrently.
            verify_deployment(&destination, kind, entries, &cli_digest)?;
        }
        verify_deployment(&destination, kind, entries, &cli_digest)
    })();
    remove_staging(&staging, &versions);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        root: PathBuf,
        temp_root: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let temp_root = std::env::temp_dir().canonicalize().unwrap();
            let root = temp_root.join(format!("any-a2a-deployment-test-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            Self { root, temp_root }
        }
        fn cli(&self, relative: &str, content: &[u8]) -> PathBuf {
            let path = self.root.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, content).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            }
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if self
                .root
                .canonicalize()
                .is_ok_and(|path| path.parent() == Some(self.temp_root.as_path()))
            {
                let _ = fs::remove_dir_all(&self.root);
            }
        }
    }

    #[test]
    fn explicit_and_bundled_resolution_need_no_source_tree() {
        let fixture = Fixture::new();
        let desktop = fixture.root.join("installed/desktop");
        let bundled = fixture.cli(
            &format!("installed/{}", executable_name()),
            b"bundled CLI fixture",
        );
        let explicit = fixture.cli("explicit-native-cli", b"explicit CLI fixture");
        assert_eq!(
            resolve_executable(Some(&explicit), Some(&desktop), None).unwrap(),
            explicit.canonicalize().unwrap()
        );
        assert_eq!(
            resolve_executable(None, Some(&desktop), None).unwrap(),
            bundled.canonicalize().unwrap()
        );
        assert!(
            resolve_executable(Some(&fixture.root.join("missing")), Some(&desktop), None).is_err()
        );
        assert!(resolve_executable(Some(&fixture.root), Some(&desktop), None).is_err());
    }

    #[test]
    fn debug_fallback_is_explicit_and_prefers_debug() {
        let fixture = Fixture::new();
        let debug = fixture.cli(
            &format!("target/debug/{}", executable_name()),
            b"debug fixture",
        );
        fixture.cli(
            &format!("target/release/{}", executable_name()),
            b"release fixture",
        );
        assert_eq!(
            resolve_executable(None, None, Some(&fixture.root)).unwrap(),
            debug.canonicalize().unwrap()
        );
        assert!(resolve_executable(None, None, None).is_err());
    }

    #[test]
    fn embedded_pi_and_dsh_are_complete_and_reused_without_sources() {
        let fixture = Fixture::new();
        let cli = fixture.cli("fixture-cli", b"native CLI fixture");
        let data = fixture.root.join("data");
        for (kind, expected) in [("pi", PI), ("dsh", DSH)] {
            let package = stage(kind, &data, &cli).unwrap();
            let version = package.parent().unwrap().parent().unwrap();
            for (name, bytes) in COMMON.iter().chain(expected) {
                assert_eq!(fs::read(version.join(name)).unwrap(), *bytes);
            }
            assert_eq!(
                fs::read(package.join("bin").join(executable_name())).unwrap(),
                b"native CLI fixture"
            );
            assert_eq!(stage(kind, &data, &cli).unwrap(), package);
            assert_eq!(
                fs::read_dir(version.parent().unwrap())
                    .unwrap()
                    .filter_map(Result::ok)
                    .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                    .count(),
                1
            );
        }
        let dsh = stage("dsh", &data, &cli).unwrap();
        assert!(dsh.join("src/../../common/src/service-client.js").is_file());
    }

    #[test]
    fn changed_cli_gets_a_new_version_and_modified_cache_is_preserved() {
        let fixture = Fixture::new();
        let cli = fixture.cli("fixture-cli", b"first CLI fixture");
        let data = fixture.root.join("data");
        let first = stage("pi", &data, &cli).unwrap();
        fs::write(&cli, b"second CLI fixture").unwrap();
        let second = stage("pi", &data, &cli).unwrap();
        assert_ne!(first, second);
        assert_eq!(
            fs::read(first.join("bin").join(executable_name())).unwrap(),
            b"first CLI fixture"
        );
        fs::write(second.join("index.ts"), b"modified deployment").unwrap();
        assert!(stage("pi", &data, &cli).is_err());
        assert_eq!(
            fs::read(second.join("index.ts")).unwrap(),
            b"modified deployment"
        );
    }

    #[test]
    fn invalid_kind_and_missing_cli_do_not_create_deployments() {
        let fixture = Fixture::new();
        let data = fixture.root.join("data");
        assert!(stage("../../other", &data, &fixture.root).is_err());
        assert!(stage("pi", &data, &fixture.root.join("missing")).is_err());
        assert!(!data.exists());
    }

    #[test]
    fn deployment_lock_releases_without_deleting_its_sidecar() {
        let fixture = Fixture::new();
        let lock = DeploymentLock::acquire(&fixture.root).unwrap();
        assert!(DeploymentLock::acquire(&fixture.root).is_err());
        drop(lock);
        assert!(fixture.root.join(".deployment.lock").is_file());
        assert!(DeploymentLock::acquire(&fixture.root).is_ok());
    }

    #[test]
    fn cleanup_only_removes_generated_direct_children() {
        let fixture = Fixture::new();
        let versions = fixture.root.join("versions");
        fs::create_dir(&versions).unwrap();
        let staging = versions.join(format!(".staging-{}", uuid::Uuid::new_v4()));
        let unrelated = fixture
            .root
            .join(format!(".staging-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&staging).unwrap();
        fs::create_dir(&unrelated).unwrap();
        remove_staging(&unrelated, &versions);
        assert!(unrelated.is_dir());
        remove_staging(&staging, &versions);
        assert!(!staging.exists());
    }

    #[test]
    fn deployed_native_cli_runs_without_a_source_checkout() {
        let fixture = Fixture::new();
        let cli = std::env::var_os("ANY_A2A_TEST_BINARY")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../target/debug")
                    .join(executable_name())
            });
        assert!(cli.is_file(), "Build the native CLI with cargo build or set ANY_A2A_TEST_BINARY before running deployment acceptance tests");
        let isolated_data = fixture.root.join("catalog");
        fs::create_dir(&isolated_data).unwrap();
        let package = stage("pi", &fixture.root.join("deployments"), &cli).unwrap();
        let native_cli = package.join("bin").join(executable_name());
        for command in ["--help", "catalog"] {
            let output = std::process::Command::new(&native_cli)
                .arg(command)
                .current_dir(&fixture.root)
                .env("ANY_A2A_DATA_DIR", &isolated_data)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "Deployed CLI {command} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            if command == "catalog" {
                assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "[]");
            } else {
                assert!(String::from_utf8(output.stdout)
                    .unwrap()
                    .contains("A2A JSON-RPC client"));
            }
        }
    }

    #[test]
    fn deployment_limit_preserves_existing_versions() {
        let fixture = Fixture::new();
        let cli = fixture.cli("fixture-cli", b"native CLI fixture");
        let data = fixture.root.join("data");
        let package = stage("pi", &data, &cli).unwrap();
        let versions = package
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        for index in 1..MAX_VERSIONS {
            fs::create_dir(versions.join(format!("old-{index}"))).unwrap();
        }
        assert_eq!(stage("pi", &data, &cli).unwrap(), package);
        fs::write(&cli, b"new native CLI fixture").unwrap();
        assert!(stage("pi", &data, &cli).unwrap_err().contains("limit"));
        assert_eq!(
            fs::read_dir(versions)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                .count(),
            MAX_VERSIONS
        );
    }

    #[cfg(unix)]
    #[test]
    fn resolver_rejects_non_executable_files() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        let cli = fixture.cli("fixture-cli", b"CLI fixture");
        fs::set_permissions(&cli, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(resolve_executable(Some(&cli), None, None).is_err());
    }
}

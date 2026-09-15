//! Versioned Pi deployment. Local source is only needed while applying.
use std::{
    fs,
    path::{Path, PathBuf},
};
pub fn stage(root: &Path, data: &Path, executable: &Path) -> Result<PathBuf, String> {
    let destination = data
        .join("integrations/pi/versions")
        .join(format!("0.1.0-{}", uuid::Uuid::new_v4()))
        .join("any-a2a");
    fs::create_dir_all(destination.join("bin")).map_err(|e| e.to_string())?;
    let result = (|| -> Result<(), String> {
        for name in [
            "package.json",
            "index.ts",
            "transport.mjs",
            "runs.mjs",
            "capabilities.mjs",
            "presentation.mjs",
            "delivery.mjs",
        ] {
            fs::copy(root.join("adapters/pi").join(name), destination.join(name))
                .map_err(|e| e.to_string())?;
        }
        fs::copy(
            executable,
            destination
                .join("bin")
                .join(format!("any-a2a{}", std::env::consts::EXE_SUFFIX)),
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_dir_all(&destination);
        return Err(error);
    }
    Ok(destination)
}

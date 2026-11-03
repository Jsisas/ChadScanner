//! Self-update from GitHub Releases: manifest check, checksummed download, staged swap.
//!
//! A release ships an app zip (`chadscanner.exe` + `onnxruntime.dll`) and a data zip (`data/`).
//! `stage` downloads into `<install>\update\`, `swap_exe` renames the running exe aside and the
//! new one into place, and `finish` (next start, before the engine loads the DLL and data)
//! moves the rest. A staging dir without the `ready` marker is an aborted download and is dropped.
use anyhow::{bail, Context};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

pub const REPO: &str = "Jsisas/ChadScanner";
/// `<install>\update\`: same volume as the install, so everything moves with plain renames.
const STAGING: &str = "update";
/// Marker inside the staging dir: the exe is already swapped, `finish` does the rest.
const READY: &str = "ready";
const DATA_DOWNLOAD: &str = "data.download";
const EXE: &str = "chadscanner.exe";
const EXE_OLD: &str = "chadscanner.exe.old";

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct Asset {
    pub url: String,
    pub sha256: String,
    pub size: u64,
    #[serde(default)]
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct Manifest {
    pub version: String,
    pub app: Asset,
    pub data: Asset,
}

pub fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let mut parts = s.strip_prefix('v').unwrap_or(s).split('.');
    let v = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
    parts.next().is_none().then_some(v)
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    matches!((parse_version(candidate), parse_version(current)), (Some(a), Some(b)) if a > b)
}

/// No overall timeout: it would cut off the 150 MB data download. ureq follows GitHub's redirect
/// to its asset host.
pub fn agent() -> anyhow::Result<ureq::Agent> {
    Ok(ureq::AgentBuilder::new()
        .tls_connector(Arc::new(native_tls::TlsConnector::new()?))
        .timeout_connect(Duration::from_secs(30))
        .timeout_read(Duration::from_secs(60))
        .user_agent("ChadScanner/0.1 (+https://github.com/Jsisas/ChadScanner)")
        .build())
}

/// The latest release's manifest, or the one of release `tag`.
pub fn fetch_manifest(agent: &ureq::Agent, tag: Option<&str>) -> anyhow::Result<Manifest> {
    let url = match tag {
        None => format!("https://github.com/{REPO}/releases/latest/download/manifest.json"),
        Some(t) => format!("https://github.com/{REPO}/releases/download/{t}/manifest.json"),
    };
    agent.get(&url).call().with_context(|| format!("GET {url}"))?.into_json().with_context(|| format!("parsing {url}"))
}

fn download(agent: &ureq::Agent, a: &Asset, dest: &Path) -> anyhow::Result<()> {
    let resp = agent.get(&a.url).call().with_context(|| format!("GET {}", a.url))?;
    let mut reader = resp.into_reader();
    let mut file = File::create(dest).with_context(|| format!("creating {}", dest.display()))?;
    let mut hash = Sha256::new();
    let mut total = 0u64;
    let mut buf = vec![0u8; 64 * 1024];
    let streamed = (|| -> anyhow::Result<()> {
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                return Ok(file.flush()?);
            }
            hash.update(&buf[..n]);
            file.write_all(&buf[..n])?;
            total += n as u64;
        }
    })();
    drop(file);
    if let Err(e) = streamed {
        let _ = std::fs::remove_file(dest);
        return Err(e.context(format!("downloading {}", a.url)));
    }
    let digest: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if total != a.size || digest != a.sha256.to_ascii_lowercase() {
        let _ = std::fs::remove_file(dest);
        bail!("checksum mismatch for {}", a.url);
    }
    Ok(())
}

/// `ZipArchive::extract` rejects entries that would escape `dir`.
fn extract(zip: &Path, dir: &Path) -> anyhow::Result<()> {
    zip::ZipArchive::new(File::open(zip)?)?.extract(dir).with_context(|| format!("extracting {}", zip.display()))
}

/// Covers the old process still unmapping its DLL.
fn rename_retry(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut tries = 0;
    loop {
        match std::fs::rename(from, to) {
            Err(e) if tries >= 9 => return Err(e),
            Err(_) => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(200));
            }
            ok => return ok,
        }
    }
}

fn remove_dir_if_exists(dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

/// `data/key.txt` of the install. Local deploys have none, so they always download data.
pub fn installed_data_key(install: &Path) -> Option<String> {
    let key = std::fs::read_to_string(install.join("data").join("key.txt")).ok()?;
    let key = key.trim();
    (!key.is_empty()).then(|| key.to_string())
}

/// Downloads and unpacks release `m` into `<install>\update\`. The data zip is fetched only when
/// its key differs from the installed one. Leaves nothing behind on error.
pub fn stage(agent: &ureq::Agent, m: &Manifest, install: &Path) -> anyhow::Result<()> {
    let dir = install.join(STAGING);
    let run = || -> anyhow::Result<()> {
        remove_dir_if_exists(&dir)?;
        std::fs::create_dir_all(&dir)?;
        let zip = dir.join("app.zip");
        download(agent, &m.app, &zip)?;
        extract(&zip, &dir)?;
        std::fs::remove_file(&zip)?;
        if installed_data_key(install).as_deref() != Some(m.data.key.as_str()) {
            let zip = dir.join("data.zip");
            download(agent, &m.data, &zip)?;
            extract(&zip, &dir)?;
            std::fs::remove_file(&zip)?;
        }
        if !dir.join(EXE).is_file() {
            bail!("release app zip has no {EXE}");
        }
        Ok(())
    };
    run().inspect_err(|_| {
        let _ = std::fs::remove_dir_all(&dir);
    })
}

/// Renames the running exe aside and the staged one into place (Windows allows renaming a
/// running exe), then marks the stage ready for `finish`.
pub fn swap_exe(exe: &Path, install: &Path) -> anyhow::Result<()> {
    let old = exe.with_extension("exe.old");
    match std::fs::remove_file(&old) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e).context("removing the previous exe.old"),
        _ => {}
    }
    std::fs::rename(exe, &old).context("moving the running exe aside")?;
    if let Err(e) = std::fs::rename(install.join(STAGING).join(EXE), exe) {
        let _ = std::fs::rename(&old, exe);
        return Err(e).context("moving the new exe into place");
    }
    if let Err(e) = File::create(install.join(STAGING).join(READY)) {
        let _ = std::fs::rename(exe, install.join(STAGING).join(EXE));
        let _ = std::fs::rename(&old, exe);
        return Err(e).context("marking the update ready");
    }
    Ok(())
}

/// Completes (or drops) a staged update. Runs at every start, before the engine loads.
/// `Ok(true)` when an update was applied.
pub fn finish(install: &Path) -> anyhow::Result<bool> {
    let dir = install.join(STAGING);
    let old_exe = install.join(EXE_OLD);
    if !dir.join(READY).exists() {
        // No stage, or an aborted one: nothing was applied.
        let _ = remove_dir_if_exists(&dir);
        let _ = std::fs::remove_file(&old_exe);
        return Ok(false);
    }
    let dll = dir.join("onnxruntime.dll");
    if dll.exists() {
        rename_retry(&dll, &install.join("onnxruntime.dll")).context("replacing onnxruntime.dll")?;
    }
    let data = dir.join("data");
    if data.exists() {
        let live = install.join("data");
        let backup = install.join("data.old");
        remove_dir_if_exists(&backup)?;
        let had_data = live.exists();
        if had_data {
            rename_retry(&live, &backup).context("moving the old data aside")?;
        }
        if let Err(e) = rename_retry(&data, &live) {
            if had_data {
                let _ = std::fs::rename(&backup, &live);
            }
            return Err(e).context("moving the new data into place");
        }
        if let Err(e) = remove_dir_if_exists(&backup) {
            log::warn!("removing {}: {e}", backup.display());
        }
    }
    remove_dir_if_exists(&dir)?;
    let _ = std::fs::remove_file(&old_exe);
    Ok(true)
}

/// First run: downloads the item data of this exact release, so the data format matches the code.
pub fn install_data(agent: &ureq::Agent, install: &Path) -> anyhow::Result<()> {
    let m = fetch_manifest(agent, Some(&format!("v{}", env!("CARGO_PKG_VERSION"))))?;
    let dir = install.join(DATA_DOWNLOAD);
    let run = || -> anyhow::Result<()> {
        remove_dir_if_exists(&dir)?;
        std::fs::create_dir_all(&dir)?;
        let zip = dir.join("data.zip");
        download(agent, &m.data, &zip)?;
        extract(&zip, &dir)?;
        std::fs::rename(dir.join("data"), install.join("data")).context("moving data into place")?;
        Ok(())
    };
    let r = run();
    let _ = std::fs::remove_dir_all(&dir);
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("chadscanner-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn put(path: PathBuf, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn newer_versions_compare_numerically() {
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(!is_newer("0.2", "0.1.0"));
        assert!(!is_newer("x", "0.1.0"));
    }

    #[test]
    fn finish_applies_a_ready_stage() {
        let d = tmp("finish-ready");
        put(d.join("update/ready"), "");
        put(d.join("update/onnxruntime.dll"), "new");
        put(d.join("update/data/key.txt"), "new");
        put(d.join("data/key.txt"), "old");
        put(d.join("onnxruntime.dll"), "old");
        put(d.join("chadscanner.exe.old"), "old");
        assert!(finish(&d).unwrap());
        assert_eq!(std::fs::read_to_string(d.join("onnxruntime.dll")).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(d.join("data/key.txt")).unwrap(), "new");
        assert!(!d.join("update").exists());
        assert!(!d.join("data.old").exists());
        assert!(!d.join("chadscanner.exe.old").exists());
    }

    #[test]
    fn finish_drops_an_aborted_stage() {
        let d = tmp("finish-aborted");
        put(d.join("update/data/key.txt"), "new");
        put(d.join("data/key.txt"), "old");
        assert!(!finish(&d).unwrap());
        assert_eq!(std::fs::read_to_string(d.join("data/key.txt")).unwrap(), "old");
        assert!(!d.join("update").exists());
    }

    #[test]
    fn parse_version_shapes() {
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("1.2.3"), Some((1, 2, 3)));
        for bad in ["1.2", "1.2.3.4", "1.2.3-beta", ""] {
            assert_eq!(parse_version(bad), None, "{bad}");
        }
    }

    #[test]
    fn installed_data_key_trims_and_rejects_empty() {
        let d = tmp("data-key");
        assert_eq!(installed_data_key(&d), None);
        put(d.join("data/key.txt"), "  \n");
        assert_eq!(installed_data_key(&d), None);
        put(d.join("data/key.txt"), " abc\n");
        assert_eq!(installed_data_key(&d).as_deref(), Some("abc"));
    }

    #[test]
    fn swap_exe_rolls_back_when_marker_fails() {
        let d = tmp("swap-rollback");
        put(d.join(EXE), "old");
        put(d.join(STAGING).join(EXE), "new");
        std::fs::create_dir_all(d.join(STAGING).join(READY)).unwrap();
        assert!(swap_exe(&d.join(EXE), &d).is_err());
        assert_eq!(std::fs::read_to_string(d.join(EXE)).unwrap(), "old");
        assert_eq!(std::fs::read_to_string(d.join(STAGING).join(EXE)).unwrap(), "new");
    }

    #[test]
    fn swap_exe_success_marks_ready() {
        let d = tmp("swap-ok");
        put(d.join(EXE), "old");
        put(d.join(STAGING).join(EXE), "new");
        swap_exe(&d.join(EXE), &d).unwrap();
        assert_eq!(std::fs::read_to_string(d.join(EXE)).unwrap(), "new");
        assert!(d.join(STAGING).join(READY).exists());
    }
}

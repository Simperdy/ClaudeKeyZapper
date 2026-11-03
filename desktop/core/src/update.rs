//! Checks the public GitHub releases of KeyZapper and downloads the installer of a newer release
//! (`.pkg` on macOS, `.msi` on Windows), verified against the SHA-256 digest GitHub publishes and
//! against the release's minisign signature (`<package>.minisig`), made in CI with a key whose public
//! half is built into the app (`desktop/update-key.pub`). The hash alone only proves the download is
//! what GitHub serves; the signature proves the release was built by our pipeline.

use crate::{gateway, l};
use minisign_verify::{PublicKey, Signature};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

pub const REPOSITORY: &str = "bl0rb/ClaudeKeyZapper";
pub const INSTALLER_EXTENSION: &str = if cfg!(windows) { ".msi" } else { ".pkg" };
const SIGNATURE_EXTENSION: &str = ".minisig";
/// minisign public key of the release pipeline (the file `minisign -G` writes).
const UPDATE_PUBLIC_KEY: &str = include_str!("../../update-key.pub");

#[derive(Clone, Debug, PartialEq)]
pub struct ReleaseInfo {
    pub version: String,
    /// GitHub pre-release (beta tags like `1.2.0-beta.1`).
    pub prerelease: bool,
    pub page_url: String,
    pub package_url: Option<String>,
    /// Hex SHA-256 of the package as published by GitHub (`digest` of the release asset), if available.
    pub package_sha256: Option<String>,
    /// `<package>.minisig` of the same release, if published.
    pub signature_url: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum UpdateError {
    Unavailable(u16),
    Network(String),
    NoPackage,
    ChecksumMismatch,
    MissingChecksum,
    MissingSignature,
    InvalidSignature,
    /// The administrator prompt was cancelled.
    Cancelled,
    InstallFailed(String),
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&match self {
            UpdateError::Unavailable(status) => l!("Update-Server nicht erreichbar (HTTP %@).", status),
            UpdateError::Network(reason) => reason.clone(),
            UpdateError::NoPackage => l!("Das Release enthält kein Installationspaket."),
            UpdateError::ChecksumMismatch => l!("Prüfsumme des heruntergeladenen Pakets stimmt nicht. Installation abgebrochen."),
            UpdateError::MissingChecksum => {
                l!("Für das Paket ist keine Prüfsumme veröffentlicht. Bitte manuell von der Release-Seite installieren.")
            }
            UpdateError::MissingSignature => {
                l!("Für das Paket ist keine Signatur veröffentlicht. Bitte manuell von der Release-Seite installieren.")
            }
            UpdateError::InvalidSignature => l!("Die Signatur des Pakets ist ungültig. Installation abgebrochen."),
            UpdateError::Cancelled => l!("Installation abgebrochen."),
            UpdateError::InstallFailed(reason) => l!("Installation fehlgeschlagen: %@", reason),
        })
    }
}

/// The newest release; with `include_prereleases` also beta releases (whichever version is highest).
pub fn latest_release(include_prereleases: bool) -> Result<ReleaseInfo, UpdateError> {
    if !include_prereleases {
        let body = get_json(&format!("https://api.github.com/repos/{REPOSITORY}/releases/latest"))?;
        return parse(&body).ok_or(UpdateError::Unavailable(200));
    }
    let body = get_json(&format!("https://api.github.com/repos/{REPOSITORY}/releases?per_page=30"))?;
    newest(&body).ok_or(UpdateError::Unavailable(200))
}

fn get_json(url: &str) -> Result<String, UpdateError> {
    let mut response = gateway::internet_agent(Duration::from_secs(15))
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "KeyZapper")
        .call()
        .map_err(|e| UpdateError::Network(network_error(&e)))?;
    if response.status().as_u16() != 200 {
        return Err(UpdateError::Unavailable(response.status().as_u16()));
    }
    response.body_mut().read_to_string().map_err(|e| UpdateError::Network(e.to_string()))
}

/// The connection error, naming the proxy it went through so IT can tell a proxy problem from a blocked host.
fn network_error(error: &ureq::Error) -> String {
    let text = match crate::proxy::describe() {
        Some(proxy) => l!("%@ (über Proxy %@)", error, proxy),
        None => error.to_string(),
    };
    if crate::proxy::is_certificate_error(&text) && !crate::proxy::ignores_certificate_errors() {
        return l!("%@ – Zertifikat eines Proxys mit TLS-Inspektion? Dessen Stammzertifikat im System hinterlegen oder per Intune „IgnoreDownloadCertificateErrors“ setzen.", text);
    }
    text
}

pub fn parse(body: &str) -> Option<ReleaseInfo> {
    release_info(&serde_json::from_str(body).ok()?)
}

/// The highest-versioned published release in a `GET /releases` list.
pub fn newest(body: &str) -> Option<ReleaseInfo> {
    let releases: Vec<serde_json::Value> = serde_json::from_str(body).ok()?;
    releases
        .iter()
        .filter(|r| r["draft"].as_bool() != Some(true))
        .filter_map(release_info)
        .max_by(|a, b| compare_versions(&a.version, &b.version))
}

fn release_info(release: &serde_json::Value) -> Option<ReleaseInfo> {
    // Only accept packages served by GitHub itself.
    let assets = release["assets"].as_array()?;
    let from_github = |a: &&serde_json::Value| {
        a["browser_download_url"].as_str().and_then(|u| url::Url::parse(u).ok()).is_some_and(|u| u.host_str() == Some("github.com"))
    };
    let asset = assets.iter().filter(from_github).find(|a| a["name"].as_str().is_some_and(|n| n.ends_with(INSTALLER_EXTENSION)));
    let signature = asset.and_then(|a| a["name"].as_str()).and_then(|package| {
        let name = format!("{package}{SIGNATURE_EXTENSION}");
        assets.iter().filter(from_github).find(|a| a["name"].as_str() == Some(name.as_str()))
    });
    Some(ReleaseInfo {
        version: normalized(release["tag_name"].as_str()?),
        prerelease: release["prerelease"].as_bool().unwrap_or(false),
        // The release page is opened in the browser: only GitHub's own, like the packages.
        page_url: release["html_url"].as_str().filter(|u| url::Url::parse(u).is_ok_and(|u| u.scheme() == "https" && u.host_str() == Some("github.com")))?.to_string(),
        package_url: asset.and_then(|a| a["browser_download_url"].as_str()).map(String::from),
        package_sha256: asset
            .and_then(|a| a["digest"].as_str())
            .and_then(|d| d.strip_prefix("sha256:"))
            .map(str::to_lowercase),
        signature_url: signature.and_then(|a| a["browser_download_url"].as_str()).map(String::from),
    })
}

fn normalized(version: &str) -> String {
    version.strip_prefix('v').unwrap_or(version).to_string()
}

/// Semantic version order: numeric fields, then a pre-release (`1.2.0-beta.2`) before its release (`1.2.0`),
/// pre-release fields compared numerically where both are numbers.
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let split = |v: &str| {
        let v = normalized(v);
        let (core, pre) = v.split_once('-').map(|(c, p)| (c.to_string(), Some(p.to_string()))).unwrap_or((v.clone(), None));
        (core.split('.').map(|p| p.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>(), pre)
    };
    let ((ca, pa), (cb, pb)) = (split(a), split(b));
    for i in 0..ca.len().max(cb.len()) {
        let order = ca.get(i).unwrap_or(&0).cmp(cb.get(i).unwrap_or(&0));
        if order.is_ne() {
            return order;
        }
    }
    match (pa, pb) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(pa), Some(pb)) => {
            let (fa, fb): (Vec<&str>, Vec<&str>) = (pa.split('.').collect(), pb.split('.').collect());
            for i in 0..fa.len().max(fb.len()) {
                let order = match (fa.get(i), fb.get(i)) {
                    (None, _) => Ordering::Less,
                    (_, None) => Ordering::Greater,
                    (Some(x), Some(y)) => match (x.parse::<u64>(), y.parse::<u64>()) {
                        (Ok(x), Ok(y)) => x.cmp(&y),
                        _ => x.cmp(y),
                    },
                };
                if order.is_ne() {
                    return order;
                }
            }
            Ordering::Equal
        }
    }
}

pub fn is_older(a: &str, b: &str) -> bool {
    compare_versions(a, b) == Ordering::Less
}

/// Beta and other pre-release versions (`1.2.0-beta.1`).
pub fn is_prerelease(version: &str) -> bool {
    version.contains('-')
}

pub fn is_newer(candidate: &str, installed: &str) -> bool {
    is_older(&normalized(installed), &normalized(candidate))
}

/// Downloads the package into a temporary folder and verifies its SHA-256 and its minisign signature
/// (refuses without a published checksum or signature).
pub fn download_package(release: &ReleaseInfo) -> Result<PathBuf, UpdateError> {
    let package_url = release.package_url.as_ref().ok_or(UpdateError::NoPackage)?;
    let expected = release.package_sha256.as_ref().ok_or(UpdateError::MissingChecksum)?;
    let signature_url = release.signature_url.as_ref().ok_or(UpdateError::MissingSignature)?;
    let data = download(package_url, 512 * 1024 * 1024)?;
    if hex_sha256(&data) != *expected {
        return Err(UpdateError::ChecksumMismatch);
    }
    let name = package_url.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or("KeyZapper-Update");
    let signature = String::from_utf8(download(signature_url, 16 * 1024)?).map_err(|_| UpdateError::InvalidSignature)?;
    verify_signature(UPDATE_PUBLIC_KEY, &data, &signature, name)?;
    let dir = std::env::temp_dir().join(format!("KeyZapper-Update-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).map_err(|e| UpdateError::Network(e.to_string()))?;
    let target = dir.join(name);
    std::fs::write(&target, &data).map_err(|e| UpdateError::Network(e.to_string()))?;
    Ok(target)
}

fn download(url: &str, limit: u64) -> Result<Vec<u8>, UpdateError> {
    let mut response = gateway::internet_agent(Duration::from_secs(300))
        .get(url)
        .header("User-Agent", "KeyZapper")
        .call()
        .map_err(|e| UpdateError::Network(network_error(&e)))?;
    if response.status().as_u16() != 200 {
        return Err(UpdateError::Unavailable(response.status().as_u16()));
    }
    let mut data = Vec::new();
    response.body_mut().with_config().limit(limit).reader().read_to_end(&mut data).map_err(|e| UpdateError::Network(e.to_string()))?;
    Ok(data)
}

/// Installs a downloaded package, then the caller restarts the app.
/// macOS: `installer` behind the system administrator prompt; returns when the installation is done.
#[cfg(target_os = "macos")]
pub fn install(package: &std::path::Path, version: &str) -> Result<(), UpdateError> {
    let script = [
        "on run argv",
        "do shell script \"/usr/sbin/installer -pkg \" & quoted form of item 1 of argv & \" -target /\" with prompt (item 2 of argv) with administrator privileges",
        "end run",
    ];
    let output = std::process::Command::new("/usr/bin/osascript")
        .args(script.into_iter().flat_map(|line| ["-e", line]))
        .arg(package)
        .arg(l!("KeyZapper möchte das Update auf Version %@ installieren.", version))
        .output()
        .map_err(|e| UpdateError::InstallFailed(e.to_string()))?;
    let error = String::from_utf8_lossy(&output.stderr);
    match output.status.success() {
        true => Ok(()),
        false if error.contains("(-128)") => Err(UpdateError::Cancelled),
        false => Err(UpdateError::InstallFailed(error.trim().to_string())),
    }
}

/// Windows: starts the Windows Installer (progress bar, UAC prompt) and returns at once; the app must exit
/// so its files are not in use. `AUTOLAUNCHAPP` makes Tauri's MSI start the new version afterwards.
#[cfg(windows)]
pub fn install(package: &std::path::Path, _version: &str) -> Result<(), UpdateError> {
    crate::git::command("msiexec")
        .arg("/i")
        .arg(package)
        .args(["/passive", "/norestart", "AUTOLAUNCHAPP=True"])
        .spawn()
        .map(|_| ())
        .map_err(|e| UpdateError::InstallFailed(e.to_string()))
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn install(_package: &std::path::Path, _version: &str) -> Result<(), UpdateError> {
    Err(UpdateError::InstallFailed("unsupported platform".into()))
}

/// Checks `data` against a minisign signature by `public_key`. The trusted comment (signed too) must
/// name the package file, so a signature of an older release cannot be passed off for a newer one.
pub fn verify_signature(public_key: &str, data: &[u8], signature: &str, package_name: &str) -> Result<(), UpdateError> {
    let key = PublicKey::decode(public_key.trim()).map_err(|_| UpdateError::InvalidSignature)?;
    let signature = Signature::decode(signature.trim()).map_err(|_| UpdateError::InvalidSignature)?;
    key.verify(data, &signature, false).map_err(|_| UpdateError::InvalidSignature)?;
    if signature.trusted_comment().trim() != package_name {
        return Err(UpdateError::InvalidSignature);
    }
    Ok(())
}

pub fn hex_sha256(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_and_picks_platform_package() {
        assert!(is_newer("v1.0.10", "1.0.9"));
        assert!(!is_newer("1.0.7", "1.0.7"));
        assert!(is_newer("1.1.0-beta.2", "1.1.0-beta.1"));
        assert!(is_newer("1.1.0-beta.10", "1.1.0-beta.9"));
        assert!(is_newer("1.1.0", "1.1.0-beta.2"));
        assert!(is_newer("1.1.0-beta.1", "1.0.7"));
        assert!(!is_newer("1.1.0-beta.1", "1.1.0"));
        assert!(is_older("2.1.206", "2.1.288"));
        let body = format!(
            r#"{{"tag_name":"1.2.0","html_url":"https://github.com/x/y/releases/1.2.0","assets":[
              {{"name":"KeyZapper-1.2.0{ext}","browser_download_url":"https://github.com/x/y/KeyZapper-1.2.0{ext}","digest":"sha256:ABC"}},
              {{"name":"Evil{ext}","browser_download_url":"https://evil.example/Evil{ext}"}}]}}"#,
            ext = INSTALLER_EXTENSION
        );
        let release = parse(&body).unwrap();
        assert_eq!(release.version, "1.2.0");
        assert!(!release.prerelease);
        assert_eq!(release.package_sha256.as_deref(), Some("abc"));
        assert!(release.package_url.unwrap().starts_with("https://github.com/"));
        assert_eq!(release.signature_url, None);
    }

    #[test]
    fn release_page_must_be_on_github() {
        let body = r#"{"tag_name":"1.2.0","html_url":"https://evil.example/releases/1.2.0","assets":[]}"#;
        assert!(parse(body).is_none());
        let body = r#"{"tag_name":"1.2.0","html_url":"http://github.com/x/y/releases/1.2.0","assets":[]}"#;
        assert!(parse(body).is_none());
    }

    #[test]
    fn finds_the_signature_of_the_package() {
        let body = format!(
            r#"{{"tag_name":"1.2.0","html_url":"https://github.com/x/y/releases/1.2.0","assets":[
              {{"name":"KeyZapper-1.2.0{ext}.minisig","browser_download_url":"https://evil.example/s.minisig"}},
              {{"name":"KeyZapper-1.2.0{ext}","browser_download_url":"https://github.com/x/y/KeyZapper-1.2.0{ext}","digest":"sha256:ABC"}},
              {{"name":"KeyZapper-1.2.0{ext}.minisig","browser_download_url":"https://github.com/x/y/KeyZapper-1.2.0{ext}.minisig"}}]}}"#,
            ext = INSTALLER_EXTENSION
        );
        let release = parse(&body).unwrap();
        assert!(release.signature_url.unwrap().starts_with("https://github.com/x/y/"));
    }

    // Throwaway key pair made with `minisign -G -W` for this test only.
    const TEST_KEY: &str = "untrusted comment: minisign public key 5E3DB03C39D0F256\nRWRW8tA5PLA9Xj3/OVZXGzEPv3l9N93BJ/HpxOz55CQlKq3CcEXmTH/u\n";
    const TEST_SIGNATURE: &str = "untrusted comment: signature from minisign secret key
RURW8tA5PLA9XuoEKLOOQvJmARLNH2zMIUo9LGr70Vlo6yUgLSG0WBcuyF/f4mo1EBuBb5DPCK1eR58h53bKF6wOw8ikrzI8BQE=
trusted comment: KeyZapper-1.2.0.pkg
LjyFp1v74V8PJ2nql9KXVirepzkrfrlmzLlriBmaIfwv5B8dSA5dQqYfwQf53ToI95gIhvxWrZmB1VFJU49BBw==
";

    #[test]
    fn verifies_signature_and_package_name() {
        assert_eq!(verify_signature(TEST_KEY, b"hello\n", TEST_SIGNATURE, "KeyZapper-1.2.0.pkg"), Ok(()));
        assert_eq!(verify_signature(TEST_KEY, b"tampered\n", TEST_SIGNATURE, "KeyZapper-1.2.0.pkg"), Err(UpdateError::InvalidSignature));
        assert_eq!(verify_signature(TEST_KEY, b"hello\n", TEST_SIGNATURE, "KeyZapper-1.3.0.pkg"), Err(UpdateError::InvalidSignature));
        assert_eq!(verify_signature(TEST_KEY, b"hello\n", "garbage", "KeyZapper-1.2.0.pkg"), Err(UpdateError::InvalidSignature));
    }

    #[test]
    fn built_in_update_key_is_valid() {
        assert!(PublicKey::decode(UPDATE_PUBLIC_KEY.trim()).is_ok());
    }

    #[test]
    fn newest_includes_betas_but_skips_drafts() {
        let body = r#"[
          {"tag_name":"1.0.7","prerelease":false,"html_url":"https://github.com/x/y/releases/1.0.7","assets":[]},
          {"tag_name":"1.1.0-beta.2","prerelease":true,"html_url":"https://github.com/x/y/releases/b2","assets":[]},
          {"tag_name":"1.1.0-beta.10","prerelease":true,"html_url":"https://github.com/x/y/releases/b10","assets":[]},
          {"tag_name":"1.2.0","draft":true,"html_url":"https://github.com/x/y/releases/d","assets":[]}]"#;
        let release = newest(body).unwrap();
        assert_eq!(release.version, "1.1.0-beta.10");
        assert!(release.prerelease);
    }
}

use std::path::{Path, PathBuf};

pub fn home_dir() -> PathBuf {
    std::env::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// Expands a leading `~` to the home folder.
pub fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        return home_dir();
    }
    match path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        Some(rest) => home_dir().join(rest),
        None => PathBuf::from(path),
    }
}

/// `~/…` for display.
pub fn abbreviate_home(path: &str) -> String {
    let home = home_dir().to_string_lossy().into_owned();
    match path.strip_prefix(&home) {
        Some(rest) if rest.is_empty() || rest.starts_with(['/', '\\']) => format!("~{rest}"),
        _ => path.to_string(),
    }
}

/// Absolute path with symlinks resolved; the normalised path if it does not exist.
pub fn canonical_path(path: &str) -> String {
    let expanded = expand_tilde(path);
    match std::fs::canonicalize(&expanded) {
        Ok(resolved) => strip_verbatim(resolved).to_string_lossy().into_owned(),
        Err(_) => normalize(&expanded).to_string_lossy().into_owned(),
    }
}

/// Windows `canonicalize` returns `\\?\C:\…`; Claude Code, git and users know `C:\…`.
fn strip_verbatim(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => path,
    }
}

fn normalize(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir().unwrap_or_default().join(path) };
    let mut out = PathBuf::new();
    for part in absolute.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// `~/Library/Application Support/KeyZapper` on macOS, `%LOCALAPPDATA%\KeyZapper` on Windows;
/// `KEYZAPPER_HOME` overrides it (tests).
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("KEYZAPPER_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }
    #[cfg(windows)]
    if let Some(local) = std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty()) {
        return PathBuf::from(local).join("KeyZapper");
    }
    #[cfg(target_os = "macos")]
    return home_dir().join("Library/Application Support/KeyZapper");
    #[allow(unreachable_code)]
    home_dir().join(".keyzapper")
}

pub fn folder_name(path: &str) -> String {
    Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string())
}

/// Writes `data` to a temporary file next to `path` and renames it into place (owner-only on Unix).
pub fn write_atomic(path: &Path, data: &[u8], private: bool) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{}.keyzapper-{}", path.file_name().unwrap_or_default().to_string_lossy(), uuid::Uuid::new_v4()));
    // A private file is created owner-only from the first byte, not chmod'ed after the write: the umask
    // would leave the key material world-readable for a moment otherwise.
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = private;
    {
        use std::io::Write;
        let mut file = options.open(&tmp)?;
        file.write_all(data).and_then(|_| file.sync_all()).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })?;
    }
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Creates the folder, owner-only on Unix.
pub fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn private_files_are_owner_only_and_replace_existing_ones() {
        let dir = std::env::temp_dir().join(format!("keyzapper-paths-{}", uuid::Uuid::new_v4()));
        let file = dir.join("keys.json");
        write_atomic(&file, b"{}", true).unwrap();
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        write_atomic(&file, b"{\"a\":\"b\"}", true).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{\"a\":\"b\"}");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temporary file left behind");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

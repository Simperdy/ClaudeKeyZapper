use std::process::{Command, Stdio};

/// `/usr/bin/git` on macOS (apps do not get the shell PATH), `git` from PATH elsewhere.
fn git_executable() -> &'static str {
    if cfg!(target_os = "macos") { "/usr/bin/git" } else { "git" }
}

/// Runs a command without flashing a console window on Windows.
pub fn command(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// `wsl.exe` from `System32` (the app does not get the shell PATH).
#[cfg(windows)]
fn wsl_executable() -> String {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    std::path::PathBuf::from(root).join("System32").join("wsl.exe").to_string_lossy().into_owned()
}

/// Git for a folder: Windows git, or for a folder inside WSL the distribution's own git through `wsl.exe` (Windows git
/// reports "dubious ownership" on `\\wsl.localhost` shares and knows nothing about the files' Linux owners).
/// `--exec` runs git without a shell, so the folder path needs no quoting beyond the Windows command line.
fn git_in(dir: &str) -> Command {
    #[cfg(windows)]
    if let Some(wsl) = crate::paths::wsl_path(dir) {
        let mut cmd = command(&wsl_executable());
        cmd.arg("-d").arg(&wsl.distro).arg("--exec").arg("git").arg("-C").arg(&wsl.path);
        return cmd;
    }
    let mut cmd = command(git_executable());
    cmd.arg("-C").arg(dir);
    cmd
}

fn exec(args: &[&str], dir: &str) -> Option<(i32, String)> {
    let mut cmd = git_in(dir);
    cmd.args(args).stdin(Stdio::null()).stderr(Stdio::null());
    for key in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR"] {
        cmd.env_remove(key);
    }
    let output = cmd.output().ok()?;
    Some((output.status.code().unwrap_or(-1), String::from_utf8_lossy(&output.stdout).into_owned()))
}

/// Trimmed stdout on exit 0, None otherwise (also when git is unavailable).
pub fn run(args: &[&str], dir: &str) -> Option<String> {
    match exec(args, dir)? {
        (0, out) => Some(out.trim().to_string()),
        _ => None,
    }
}

/// `run` for commands that print a path (`rev-parse`): git inside WSL answers with the Linux path, which is returned
/// as the Windows path of the same folder so callers can open it.
pub fn run_path(args: &[&str], dir: &str) -> Option<String> {
    run(args, dir).map(|out| windows_path_of_output(dir, out))
}

fn windows_path_of_output(dir: &str, out: String) -> String {
    match crate::paths::wsl_path(dir) {
        Some(wsl) if out.starts_with('/') => wsl.windows_path(&out),
        _ => out,
    }
}

pub fn status(args: &[&str], dir: &str) -> i32 {
    exec(args, dir).map(|(code, _)| code).unwrap_or(-1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `rev-parse --git-common-dir` / `--show-toplevel` print for a WSL folder, as Windows paths; a worktree's
    /// common dir lies in the main repository. Other folders and relative output (`ls-files`) pass through.
    #[test]
    fn git_paths_from_wsl_come_back_as_windows_paths() {
        let wt = r"\\wsl$\Ubuntu\home\u\proj wt";
        assert_eq!(windows_path_of_output(wt, "/home/u/proj/.git".into()), r"\\wsl$\Ubuntu\home\u\proj\.git");
        assert_eq!(windows_path_of_output(r"\\wsl.localhost\Ubuntu\home\u\proj\sub", "/home/u/proj".into()), r"\\wsl.localhost\Ubuntu\home\u\proj");
        assert_eq!(windows_path_of_output(wt, "src/main.rs\0README.md".into()), "src/main.rs\0README.md");
        assert_eq!(windows_path_of_output("/Users/u/proj", "/Users/u/proj/.git".into()), "/Users/u/proj/.git");
        assert_eq!(windows_path_of_output(r"C:\proj", "C:/proj/.git".into()), "C:/proj/.git");
    }
}

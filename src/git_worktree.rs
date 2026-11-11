use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) fn command() -> Command {
    let command = Command::new("git");
    #[cfg(test)]
    let command = {
        let mut command = command;
        // Unit tests must not inherit personal config or Git's default global ignore file.
        command
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["-c", "core.excludesFile=/dev/null"]);
        command
    };
    command
}

/// Git worktree root reported by Git, including linked worktrees.
pub(crate) fn worktree_root(start: &Path) -> Result<PathBuf, String> {
    let output = command()
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(start)
        .output()
        .map_err(|error| format!("cannot run git: {error}"))?;
    if !output.status.success() {
        return Err("project management requires a Git worktree".into());
    }
    let raw = String::from_utf8(output.stdout)
        .map_err(|_| "git returned a non-UTF-8 worktree path".to_owned())?;
    Ok(PathBuf::from(raw.trim()))
}

/// Main worktree discovery uses Git reports; separate Git directories and submodules retain their own root.
pub(crate) fn main_worktree(root: &Path) -> Result<PathBuf, String> {
    let output = git(root, &["worktree", "list", "--porcelain", "-z"])?;
    let path = output
        .strip_prefix("worktree ")
        .and_then(|output| output.split_once('\0'))
        .map(|(path, _)| path)
        .filter(|path| !path.is_empty())
        .ok_or_else(|| "git did not report a main worktree".to_owned())?;
    let path = fs::canonicalize(path)
        .map_err(|error| format!("cannot resolve reported main worktree: {error}"))?;
    let is_worktree = git(&path, &["rev-parse", "--is-inside-work-tree"])?;
    if is_worktree.trim() == "true" {
        Ok(path)
    } else {
        // Submodules and separate Git directories report their Git directory here.
        Ok(root.to_owned())
    }
}

/// Git discovery and Local mutation queries retain command failure details.
pub(crate) fn git(directory: &Path, arguments: &[&str]) -> Result<String, String> {
    let output = command()
        .args(arguments)
        .current_dir(directory)
        .output()
        .map_err(|error| format!("cannot run git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout).map_err(|_| "git returned non-UTF-8 output".to_owned())
}

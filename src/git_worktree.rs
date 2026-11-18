use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

thread_local! {
    // Installed only inside `Topology::scope`; elsewhere every query runs Git.
    static TOPOLOGY: RefCell<Option<Topology>> = const { RefCell::new(None) };
}

/// Worktree root and main worktree answers shared by every consumer in one TUI refresh or watch
/// check, so each consumer does not spawn its own Git processes.
#[derive(Clone, Default)]
pub(crate) struct Topology {
    marker: Option<u64>,
    answers: HashMap<(&'static str, PathBuf), Result<PathBuf, String>>,
}

impl Topology {
    /// Keeps remembered answers only while the `.git` entries at and above `directory` are
    /// unchanged; adding, removing or repointing one asks Git again. Returns whether the
    /// remembered answers were kept.
    pub(crate) fn revalidate(&mut self, directory: &Path) -> bool {
        let marker = topology_marker(directory);
        let kept = self.marker == Some(marker);
        if !kept {
            self.answers.clear();
            self.marker = Some(marker);
        }
        kept
    }

    pub(crate) fn forget(&mut self) {
        self.answers.clear();
    }

    /// Answers this thread's worktree discovery from `self`, recording new answers, while
    /// `body` runs.
    pub(crate) fn scope<T>(&mut self, body: impl FnOnce() -> T) -> T {
        TOPOLOGY.set(Some(std::mem::take(self)));
        let result = body();
        *self = TOPOLOGY.take().expect("scope installed the topology");
        result
    }
}

fn topology_marker(directory: &Path) -> u64 {
    let mut hasher = DefaultHasher::new();
    let directory = fs::canonicalize(directory).unwrap_or_else(|_| directory.to_owned());
    for ancestor in directory.ancestors() {
        let dot_git = ancestor.join(".git");
        let Ok(metadata) = fs::symlink_metadata(&dot_git) else {
            continue;
        };
        ancestor.hash(&mut hasher);
        metadata.is_dir().hash(&mut hasher);
        if metadata.is_dir() {
            continue;
        }
        // Linked worktrees and submodules point at their Git directory, whose `commondir`
        // locates the main worktree.
        let pointer = fs::read_to_string(&dot_git).ok();
        pointer.hash(&mut hasher);
        if let Some(git_dir) = pointer
            .as_deref()
            .and_then(|pointer| pointer.strip_prefix("gitdir:"))
        {
            fs::read_to_string(ancestor.join(git_dir.trim()).join("commondir"))
                .ok()
                .hash(&mut hasher);
        }
    }
    hasher.finish()
}

fn remembered(
    query: &'static str,
    path: &Path,
    discover: impl FnOnce() -> Result<PathBuf, String>,
) -> Result<PathBuf, String> {
    let key = (
        query,
        fs::canonicalize(path).unwrap_or_else(|_| path.to_owned()),
    );
    if let Some(answer) = TOPOLOGY.with_borrow(|topology| {
        topology
            .as_ref()
            .and_then(|topology| topology.answers.get(&key).cloned())
    }) {
        return answer;
    }
    let answer = discover();
    TOPOLOGY.with_borrow_mut(|topology| {
        if let Some(topology) = topology {
            topology.answers.insert(key, answer.clone());
        }
    });
    answer
}

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
    remembered("worktree root", start, || discover_worktree_root(start))
}

fn discover_worktree_root(start: &Path) -> Result<PathBuf, String> {
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
    remembered("main worktree", root, || discover_main_worktree(root))
}

fn discover_main_worktree(root: &Path) -> Result<PathBuf, String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topology_asks_git_again_after_a_repository_appears() {
        let directory = tempfile::TempDir::new().unwrap();
        let mut topology = Topology::default();
        assert!(!topology.revalidate(directory.path()));
        assert!(topology.scope(|| worktree_root(directory.path())).is_err());
        assert!(topology.revalidate(directory.path()));

        git(directory.path(), &["init", "-q"]).unwrap();
        // Without revalidation the remembered answer stands.
        assert!(topology.scope(|| worktree_root(directory.path())).is_err());
        assert!(!topology.revalidate(directory.path()));
        assert_eq!(
            topology.scope(|| worktree_root(directory.path())).unwrap(),
            fs::canonicalize(directory.path()).unwrap()
        );
    }
}

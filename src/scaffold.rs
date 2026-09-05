use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::{GlobalConfig, Paths, atomic_create};

pub(crate) const COMMON_CONTENT: &str =
    "### Common\n\n<!-- Add instructions shared by your agents here before applying. -->\n";
const GITIGNORE: &str = "/projects/\n/state/\n/backups/\n";

pub(crate) fn manifest(targets: &[String]) -> Result<String, String> {
    let mut source = "[ui]\ntheme = \"gruvbox-dark\"\n\n[[sections]]\nid = \"common\"\nname = \"Common\"\npath = \"sections/common.md\"\n".to_owned();
    let mut seen = HashSet::new();
    for id in targets {
        if !seen.insert(id.as_str()) {
            return Err(format!("Global target {id} was given more than once"));
        }
        let (path, title) = match id.as_str() {
            "claude" => ("~/.claude/CLAUDE.md", "Global Claude"),
            "codex" => ("~/.codex/AGENTS.md", "Global Codex"),
            "pi" => ("~/.pi/agent/AGENTS.md", "Global Pi"),
            "cursor" => {
                return Err(
                    "Cursor has no Global Markdown target; use `mdmanager project adopt agents` for an existing AGENTS.md or `mdmanager project create agents --from FILE` to create one, and `mdmanager context --runtime cursor` to inspect it"
                        .into(),
                );
            }
            _ => {
                return Err(format!(
                    "unknown Global target {id}; expected claude, codex, or pi"
                ));
            }
        };
        source.push_str(&format!(
            "\n[targets.{id}]\npath = \"{path}\"\ntitle = \"{title}\"\n"
        ));
    }
    if !targets.is_empty() {
        source.push_str("\n[profiles.default]\n");
        for id in targets {
            source.push_str(&format!("{id} = [\"common\"]\n"));
        }
    }
    Ok(source)
}

pub(crate) fn common_path(paths: &Paths) -> Result<PathBuf, String> {
    paths
        .config
        .parent()
        .map(|parent| parent.join("sections/common.md"))
        .ok_or_else(|| "configuration path has no parent directory".to_owned())
}

pub(crate) fn create(paths: &Paths, targets: &[String]) -> Result<GlobalConfig, String> {
    let source = manifest(targets)?;
    let common = common_path(paths)?;
    let gitignore = paths.data_dir.join(".gitignore");
    require_absent(&paths.config)?;
    require_absent(&common)?;
    require_absent(&gitignore)?;
    atomic_create(&common, COMMON_CONTENT.as_bytes())?;
    if let Err(error) = atomic_create(&gitignore, GITIGNORE.as_bytes()) {
        let _ = fs::remove_file(&common);
        return Err(error);
    }
    if let Err(error) = atomic_create(&paths.config, source.as_bytes()) {
        let _ = fs::remove_file(&common);
        let _ = fs::remove_file(&gitignore);
        return Err(error);
    }
    GlobalConfig::load(paths)
}

fn require_absent(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(format!("refusing to overwrite {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("cannot inspect {}: {error}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn selected_target_starter_is_valid_and_does_not_deploy() {
        let home = TempDir::new().unwrap();
        let paths = Paths::for_home(home.path());
        let targets = vec!["codex".into(), "pi".into()];
        let global = create(&paths, &targets).unwrap();
        assert_eq!(global.target_names().collect::<Vec<_>>(), ["codex", "pi"]);
        assert_eq!(
            fs::read_to_string(&paths.config).unwrap(),
            manifest(&targets).unwrap()
        );
        assert_eq!(
            fs::read_to_string(common_path(&paths).unwrap()).unwrap(),
            COMMON_CONTENT
        );
        assert_eq!(
            fs::read_to_string(paths.data_dir.join(".gitignore")).unwrap(),
            GITIGNORE
        );
        assert!(!home.path().join(".claude/CLAUDE.md").exists());
        assert!(!home.path().join(".codex/AGENTS.md").exists());
        assert!(!home.path().join(".pi/agent/AGENTS.md").exists());
    }

    #[test]
    fn library_only_starter_has_no_global_profile() {
        let home = TempDir::new().unwrap();
        let paths = Paths::for_home(home.path());
        let global = create(&paths, &[]).unwrap();

        assert_eq!(global.target_names().count(), 0);
        assert_eq!(global.profile_names().count(), 0);
        assert!(
            !fs::read_to_string(&paths.config)
                .unwrap()
                .contains("[profiles.")
        );
    }

    #[test]
    fn invalid_target_selection_does_not_create_files() {
        let home = TempDir::new().unwrap();
        let paths = Paths::for_home(home.path());
        let error = create(&paths, &["cursor".into()]).unwrap_err();

        assert!(error.contains("Cursor has no Global Markdown target"));
        assert!(!paths.data_dir.exists());
    }

    #[test]
    fn existing_source_is_not_overwritten() {
        let home = TempDir::new().unwrap();
        let paths = Paths::for_home(home.path());
        fs::create_dir_all(paths.config.parent().unwrap()).unwrap();
        fs::write(&paths.config, "existing").unwrap();
        let error = create(&paths, &[]).unwrap_err();
        assert!(error.contains("refusing to overwrite"));
        assert_eq!(fs::read_to_string(&paths.config).unwrap(), "existing");
        assert!(!common_path(&paths).unwrap().exists());
    }

    #[test]
    fn existing_common_section_is_not_overwritten() {
        let home = TempDir::new().unwrap();
        let paths = Paths::for_home(home.path());
        let common = common_path(&paths).unwrap();
        fs::create_dir_all(common.parent().unwrap()).unwrap();
        fs::write(&common, "existing").unwrap();
        let error = create(&paths, &[]).unwrap_err();
        assert!(error.contains("refusing to overwrite"));
        assert_eq!(fs::read_to_string(common).unwrap(), "existing");
        assert!(!paths.config.exists());
    }
}

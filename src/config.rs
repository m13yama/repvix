use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

pub const CONFIG_FILE: &str = "repvix.yaml";
pub const CONFIG_VERSION: u32 = 2;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    pub workspace: WorkspaceConfig,
    #[serde(default)]
    pub repos: Vec<RepoConfig>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub commands: BTreeMap<String, StoredCommand>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfig {
    pub name: String,
    pub base_path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepoConfig {
    pub name: String,
    pub url: String,
    /// A parent directory relative to workspace.base_path. This never renames
    /// the repository's own directory.
    pub path: PathBuf,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyConfig {
    version: u32,
    workspace: WorkspaceConfig,
    #[serde(default)]
    repos: Vec<LegacyRepoConfig>,
    #[serde(default)]
    commands: BTreeMap<String, StoredCommand>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyRepoConfig {
    url: String,
    #[serde(default)]
    path: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredCommand {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub program: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

impl Config {
    pub fn new(name: String, base_path: PathBuf) -> Self {
        Self {
            version: CONFIG_VERSION,
            workspace: WorkspaceConfig { name, base_path },
            repos: Vec::new(),
            commands: BTreeMap::new(),
        }
    }

    pub fn load(workspace_root: &Path) -> Result<Self> {
        let path = workspace_root.join(CONFIG_FILE);
        let source = fs::read_to_string(&path)
            .map_err(|error| Error::io(format!("failed to read {}", path.display()), error))?;
        let value: serde_yaml::Value = serde_yaml::from_str(&source).map_err(|error| {
            Error::Config(format!("failed to parse {}: {error}", path.display()))
        })?;
        let version = value
            .get("version")
            .and_then(serde_yaml::Value::as_u64)
            .ok_or_else(|| Error::Config("version must be an integer".into()))?;
        let config = match version {
            1 => migrate_legacy_config(value)?,
            version if version == u64::from(CONFIG_VERSION) => serde_yaml::from_value(value)
                .map_err(|error| {
                    Error::Config(format!("failed to parse {}: {error}", path.display()))
                })?,
            other => {
                return Err(Error::Config(format!(
                    "unsupported version {other}; expected {CONFIG_VERSION}"
                )));
            }
        };
        config.validate(workspace_root)?;
        Ok(config)
    }

    pub fn save(&self, workspace_root: &Path) -> Result<()> {
        self.validate(workspace_root)?;
        let path = workspace_root.join(CONFIG_FILE);
        let temporary = workspace_root.join(format!(".{CONFIG_FILE}.tmp"));
        let yaml = serde_yaml::to_string(self).map_err(|error| {
            Error::Config(format!("failed to serialize configuration: {error}"))
        })?;
        fs::write(
            &temporary,
            format!("# repvix workspace configuration\n{yaml}"),
        )
        .map_err(|error| Error::io(format!("failed to write {}", temporary.display()), error))?;
        fs::rename(&temporary, &path)
            .map_err(|error| Error::io(format!("failed to replace {}", path.display()), error))?;
        Ok(())
    }

    pub fn validate(&self, workspace_root: &Path) -> Result<()> {
        if self.version != CONFIG_VERSION {
            return Err(Error::Config(format!(
                "unsupported version {}; expected {CONFIG_VERSION}",
                self.version
            )));
        }
        if self.workspace.name.trim().is_empty() {
            return Err(Error::Config("workspace.name must not be empty".into()));
        }
        if self.workspace.base_path.as_os_str().is_empty() {
            return Err(Error::Config(
                "workspace.base_path must not be empty".into(),
            ));
        }

        let mut destinations = HashSet::new();
        let mut urls = HashSet::new();
        for repo in &self.repos {
            if repo.url.trim().is_empty() {
                return Err(Error::Config("repository URL must not be empty".into()));
            }
            validate_repo_name(&repo.name)?;
            validate_repo_parent(&repo.path)?;
            let derived_name = repo_name(&repo.url)?;
            if repo.name != derived_name {
                return Err(Error::Config(format!(
                    "repository name `{}` must match URL-derived name `{derived_name}` for {}",
                    repo.name, repo.url
                )));
            }
            if !urls.insert(repo.url.trim().to_owned()) {
                return Err(Error::Config(format!(
                    "repository URL is configured more than once: {}",
                    repo.url
                )));
            }
            let destination = self.repo_destination(workspace_root, repo)?;
            if !destinations.insert(destination.clone()) {
                return Err(Error::Config(format!(
                    "more than one repository resolves to {}",
                    destination.display()
                )));
            }
        }

        for (name, command) in &self.commands {
            if name.trim().is_empty() {
                return Err(Error::Config("command name must not be empty".into()));
            }
            if command.program.trim().is_empty() {
                return Err(Error::Config(format!(
                    "commands.{name}.program must not be empty"
                )));
            }
        }
        Ok(())
    }

    pub fn base_path(&self, workspace_root: &Path) -> PathBuf {
        if self.workspace.base_path.is_absolute() {
            self.workspace.base_path.clone()
        } else {
            workspace_root.join(&self.workspace.base_path)
        }
    }

    pub fn repo_destination(&self, workspace_root: &Path, repo: &RepoConfig) -> Result<PathBuf> {
        validate_repo_name(&repo.name)?;
        validate_repo_parent(&repo.path)?;
        let mut destination = self.base_path(workspace_root);
        if repo.path != Path::new(".") {
            destination.push(&repo.path);
        }
        destination.push(&repo.name);
        Ok(destination)
    }
}

fn migrate_legacy_config(value: serde_yaml::Value) -> Result<Config> {
    let legacy: LegacyConfig = serde_yaml::from_value(value).map_err(|error| {
        Error::Config(format!("failed to parse version 1 configuration: {error}"))
    })?;
    if legacy.version != 1 {
        return Err(Error::Config(format!(
            "cannot migrate configuration version {}",
            legacy.version
        )));
    }
    let repos = legacy
        .repos
        .into_iter()
        .map(|repo| {
            Ok(RepoConfig {
                name: repo_name(&repo.url)?,
                url: repo.url,
                path: repo.path.unwrap_or_else(|| PathBuf::from(".")),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Config {
        version: CONFIG_VERSION,
        workspace: legacy.workspace,
        repos,
        commands: legacy.commands,
    })
}

pub fn find_workspace(start: &Path) -> Result<PathBuf> {
    let mut current = if start.is_file() {
        start.parent().unwrap_or(start).to_path_buf()
    } else {
        start.to_path_buf()
    };

    loop {
        if current.join(CONFIG_FILE).is_file() {
            return Ok(current);
        }
        if !current.pop() {
            return Err(Error::Usage(format!(
                "not in a repvix workspace; run `repvix init` first (looked for {CONFIG_FILE})"
            )));
        }
    }
}

pub fn repo_name(url: &str) -> Result<String> {
    let trimmed = url.trim().trim_end_matches('/');
    let segment = trimmed
        .rsplit(['/', ':'])
        .next()
        .unwrap_or("")
        .strip_suffix(".git")
        .unwrap_or_else(|| trimmed.rsplit(['/', ':']).next().unwrap_or(""));

    if segment.is_empty() || segment == "." || segment == ".." {
        return Err(Error::Config(format!(
            "cannot determine repository name from URL: {url}"
        )));
    }
    if segment.contains(['/', '\\']) {
        return Err(Error::Config(format!(
            "invalid repository name derived from URL: {segment}"
        )));
    }
    Ok(segment.to_owned())
}

fn validate_repo_name(name: &str) -> Result<()> {
    if name.trim().is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
        return Err(Error::Config(format!("invalid repository name: {name}")));
    }
    Ok(())
}

fn validate_repo_parent(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() {
        return Err(Error::Config("repository path must not be empty".into()));
    }
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(Error::Config(format!(
            "repository path must stay inside workspace.base_path: {}",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn derives_repository_names_without_allowing_renames() {
        assert_eq!(repo_name("https://github.com/acme/api.git").unwrap(), "api");
        assert_eq!(repo_name("git@github.com:acme/web.git").unwrap(), "web");
        assert_eq!(
            repo_name("ssh://git@example.com/acme/worker/").unwrap(),
            "worker"
        );
    }

    #[test]
    fn destination_combines_base_parent_and_remote_name() {
        let root = Path::new("/workspace");
        let config = Config::new("demo".into(), PathBuf::from("../sources"));
        let repo = RepoConfig {
            name: "api".into(),
            url: "git@github.com:acme/api.git".into(),
            path: PathBuf::from("services/backend"),
        };
        assert_eq!(
            config.repo_destination(root, &repo).unwrap(),
            PathBuf::from("/workspace/../sources/services/backend/api")
        );
    }

    #[test]
    fn repository_parent_cannot_escape_base_path() {
        let root = Path::new("/workspace");
        let config = Config::new("demo".into(), PathBuf::from("repos"));
        let repo = RepoConfig {
            name: "api".into(),
            url: "https://github.com/acme/api.git".into(),
            path: PathBuf::from("../renamed"),
        };
        assert!(config.repo_destination(root, &repo).is_err());
    }

    #[test]
    fn yaml_rejects_removed_tag_field() {
        let yaml = r#"
version: 1
workspace:
  name: demo
  base_path: repos
repos:
  - url: https://github.com/acme/api.git
    tags: [backend]
"#;
        assert!(serde_yaml::from_str::<Config>(yaml).is_err());
    }

    #[test]
    fn saves_and_loads_configuration() {
        let directory = tempdir().unwrap();
        let mut config = Config::new("demo".into(), PathBuf::from("repos"));
        config.repos.push(RepoConfig {
            name: "api".into(),
            url: "https://github.com/acme/api.git".into(),
            path: PathBuf::from("services"),
        });
        config.save(directory.path()).unwrap();
        assert_eq!(Config::load(directory.path()).unwrap(), config);
    }

    #[test]
    fn loads_version_one_with_explicit_version_two_fields() {
        let directory = tempdir().unwrap();
        fs::write(
            directory.path().join(CONFIG_FILE),
            r#"
version: 1
workspace:
  name: demo
  base_path: repos
repos:
- url: https://github.com/acme/api.git
"#,
        )
        .unwrap();

        let config = Config::load(directory.path()).unwrap();
        assert_eq!(config.version, 2);
        assert_eq!(config.repos[0].name, "api");
        assert_eq!(config.repos[0].path, PathBuf::from("."));
    }

    #[test]
    fn rejects_a_name_that_would_rename_the_repository() {
        let directory = tempdir().unwrap();
        let mut config = Config::new("demo".into(), PathBuf::from("repos"));
        config.repos.push(RepoConfig {
            name: "renamed-api".into(),
            url: "https://github.com/acme/api.git".into(),
            path: PathBuf::from("."),
        });
        assert!(config.validate(directory.path()).is_err());
    }
}

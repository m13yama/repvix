use crate::config::{CONFIG_FILE, Config, RepoConfig, StoredCommand};
use crate::process::{CommandRunner, ProcessOutput, ProcessSpec};
use crate::{Error, Result};
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn init_workspace(directory: &Path, name: String, base_path: PathBuf) -> Result<Config> {
    let config_path = directory.join(CONFIG_FILE);
    if config_path.exists() {
        return Err(Error::Usage(format!(
            "{} already exists",
            config_path.display()
        )));
    }
    fs::create_dir_all(directory).map_err(|error| {
        Error::io(
            format!(
                "failed to create workspace directory {}",
                directory.display()
            ),
            error,
        )
    })?;
    let config = Config::new(name, base_path);
    config.validate(directory)?;
    fs::create_dir_all(config.base_path(directory)).map_err(|error| {
        Error::io(
            format!(
                "failed to create base path {}",
                config.base_path(directory).display()
            ),
            error,
        )
    })?;
    config.save(directory)?;
    ignore_local_base_path(directory, &config.workspace.base_path)?;
    Ok(config)
}

fn ignore_local_base_path(workspace_root: &Path, base_path: &Path) -> Result<()> {
    use std::path::Component;

    if base_path.is_absolute()
        || base_path == Path::new(".")
        || base_path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Ok(());
    }

    let entry = format!("{}/", base_path.to_string_lossy().trim_end_matches('/'));
    let gitignore = workspace_root.join(".gitignore");
    let existing = match fs::read_to_string(&gitignore) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(Error::io(
                format!("failed to read {}", gitignore.display()),
                error,
            ));
        }
    };
    if existing.lines().any(|line| line.trim() == entry) {
        return Ok(());
    }

    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(&entry);
    updated.push('\n');
    fs::write(&gitignore, updated)
        .map_err(|error| Error::io(format!("failed to write {}", gitignore.display()), error))
}

pub fn add_repository(
    config: &mut Config,
    workspace_root: &Path,
    url: String,
    path: Option<PathBuf>,
) -> Result<PathBuf> {
    let repo = RepoConfig {
        name: crate::config::repo_name(&url)?,
        url,
        path: path.unwrap_or_else(|| PathBuf::from(".")),
    };
    let destination = config.repo_destination(workspace_root, &repo)?;
    config.repos.push(repo);
    if let Err(error) = config.validate(workspace_root) {
        config.repos.pop();
        return Err(error);
    }
    config.save(workspace_root)?;
    Ok(destination)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportAction {
    Added,
    Updated,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportResult {
    pub action: ImportAction,
    pub repo: RepoConfig,
    pub destination: PathBuf,
}

pub fn import_repository(
    runner: &impl CommandRunner,
    config: &mut Config,
    workspace_root: &Path,
    repository_path: &Path,
    url_override: Option<String>,
) -> Result<ImportResult> {
    let repository_path = fs::canonicalize(repository_path).map_err(|error| {
        Error::io(
            format!(
                "failed to resolve repository path {}",
                repository_path.display()
            ),
            error,
        )
    })?;
    let base_path = fs::canonicalize(config.base_path(workspace_root)).map_err(|error| {
        Error::io(
            format!(
                "failed to resolve workspace base path {}",
                config.base_path(workspace_root).display()
            ),
            error,
        )
    })?;

    let top_level = git_capture(runner, &repository_path, &["rev-parse", "--show-toplevel"])?;
    let top_level = fs::canonicalize(top_level.trim()).map_err(|error| {
        Error::io(
            format!("failed to resolve Git top-level path: {}", top_level.trim()),
            error,
        )
    })?;
    if top_level != repository_path {
        return Err(Error::Usage(format!(
            "import path must be the repository root {}; got {}",
            top_level.display(),
            repository_path.display()
        )));
    }

    let relative = repository_path.strip_prefix(&base_path).map_err(|_| {
        Error::Usage(format!(
            "repository {} is outside workspace base path {}",
            repository_path.display(),
            base_path.display()
        ))
    })?;
    let name = relative
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Error::Usage("repository directory name must be valid UTF-8".into()))?
        .to_owned();
    let parent = relative.parent().unwrap_or_else(|| Path::new(""));
    let path = if parent.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        parent.to_path_buf()
    };

    let url = match url_override {
        Some(url) => url,
        None => git_capture(runner, &repository_path, &["remote", "get-url", "origin"])?
            .trim()
            .to_owned(),
    };
    if url.is_empty() {
        return Err(Error::Usage(
            "repository origin URL is empty; pass an explicit URL with --url".into(),
        ));
    }
    let remote_name = crate::config::repo_name(&url)?;
    if name != remote_name {
        return Err(Error::Usage(format!(
            "repository directory `{name}` must match origin repository name `{remote_name}`"
        )));
    }

    let repo = RepoConfig { name, url, path };
    let destination = config.repo_destination(workspace_root, &repo)?;
    let existing = config.repos.iter().position(|configured| {
        config
            .repo_destination(workspace_root, configured)
            .ok()
            .and_then(|path| fs::canonicalize(path).ok())
            .is_some_and(|path| path == repository_path)
    });

    let action = if let Some(index) = existing {
        let previous = std::mem::replace(&mut config.repos[index], repo.clone());
        if let Err(error) = config.validate(workspace_root) {
            config.repos[index] = previous;
            return Err(error);
        }
        ImportAction::Updated
    } else {
        config.repos.push(repo.clone());
        if let Err(error) = config.validate(workspace_root) {
            config.repos.pop();
            return Err(error);
        }
        ImportAction::Added
    };
    config.save(workspace_root)?;
    Ok(ImportResult {
        action,
        repo,
        destination,
    })
}

fn git_capture(runner: &impl CommandRunner, cwd: &Path, args: &[&str]) -> Result<String> {
    let result = runner.run(&ProcessSpec {
        program: "git".into(),
        args: args.iter().map(OsString::from).collect(),
        cwd: Some(cwd.to_path_buf()),
        capture: true,
    })?;
    if !result.success {
        let message = String::from_utf8_lossy(&result.stderr);
        return Err(Error::Operation(format!(
            "git {} failed in {}: {}",
            args.join(" "),
            cwd.display(),
            message.trim()
        )));
    }
    String::from_utf8(result.stdout)
        .map_err(|_| Error::Operation("Git output was not valid UTF-8".into()))
}

pub fn add_command(
    config: &mut Config,
    workspace_root: &Path,
    name: String,
    command: StoredCommand,
) -> Result<()> {
    if config.commands.contains_key(&name) {
        return Err(Error::Usage(format!("command `{name}` already exists")));
    }
    config.commands.insert(name.clone(), command);
    if let Err(error) = config.validate(workspace_root) {
        config.commands.remove(&name);
        return Err(error);
    }
    config.save(workspace_root)
}

pub fn remove_command(config: &mut Config, workspace_root: &Path, name: &str) -> Result<()> {
    if config.commands.remove(name).is_none() {
        return Err(Error::Usage(format!("unknown stored command: {name}")));
    }
    config.save(workspace_root)
}

pub fn clone_repositories(
    runner: &impl CommandRunner,
    config: &Config,
    workspace_root: &Path,
    output: &mut dyn Write,
) -> Result<()> {
    let mut failures = 0usize;
    for repo in &config.repos {
        let destination = config.repo_destination(workspace_root, repo)?;
        let name = &repo.name;
        if destination.exists() {
            if destination.join(".git").exists() {
                writeln!(
                    output,
                    "skip  {name}: already cloned at {}",
                    destination.display()
                )
                .map_err(|error| Error::io("failed to write output", error))?;
            } else {
                failures += 1;
                writeln!(
                    output,
                    "error {name}: destination exists and is not a Git repository: {}",
                    destination.display()
                )
                .map_err(|error| Error::io("failed to write output", error))?;
            }
            continue;
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                Error::io(format!("failed to create {}", parent.display()), error)
            })?;
        }
        writeln!(output, "clone {name} -> {}", destination.display())
            .map_err(|error| Error::io("failed to write output", error))?;
        let result = runner.run(&ProcessSpec {
            program: "git".into(),
            args: vec![
                "clone".into(),
                "--".into(),
                repo.url.clone().into(),
                destination.clone().into_os_string(),
            ],
            cwd: None,
            capture: true,
        })?;
        write_captured(output, &result)?;
        if !result.success {
            failures += 1;
        }
    }
    finish_operation("clone", failures)
}

pub fn list_repositories(
    config: &Config,
    workspace_root: &Path,
    output: &mut dyn Write,
) -> Result<()> {
    for repo in &config.repos {
        let name = &repo.name;
        let destination = config.repo_destination(workspace_root, repo)?;
        writeln!(output, "{name}\t{}\t{}", destination.display(), repo.url)
            .map_err(|error| Error::io("failed to write output", error))?;
    }
    Ok(())
}

pub fn git_status(
    runner: &impl CommandRunner,
    config: &Config,
    workspace_root: &Path,
    output: &mut dyn Write,
) -> Result<()> {
    run_git_for_all(
        runner,
        config,
        workspace_root,
        &["status", "--short", "--branch"],
        "status",
        output,
    )
}

pub fn create_branch(
    runner: &impl CommandRunner,
    config: &Config,
    workspace_root: &Path,
    branch: &str,
    output: &mut dyn Write,
) -> Result<()> {
    run_git_for_all(
        runner,
        config,
        workspace_root,
        &["checkout", "-b", branch],
        "branch",
        output,
    )
}

pub fn checkout(
    runner: &impl CommandRunner,
    config: &Config,
    workspace_root: &Path,
    branch: &str,
    output: &mut dyn Write,
) -> Result<()> {
    run_git_for_all(
        runner,
        config,
        workspace_root,
        &["checkout", branch],
        "checkout",
        output,
    )
}

pub fn sync(
    runner: &impl CommandRunner,
    config: &Config,
    workspace_root: &Path,
    rebase: bool,
    output: &mut dyn Write,
) -> Result<()> {
    let args = if rebase {
        vec!["pull", "--rebase"]
    } else {
        vec!["pull"]
    };
    run_git_for_all(runner, config, workspace_root, &args, "sync", output)
}

pub fn exec_for_all(
    runner: &impl CommandRunner,
    config: &Config,
    workspace_root: &Path,
    program: OsString,
    args: Vec<OsString>,
    output: &mut dyn Write,
) -> Result<()> {
    run_process_for_all(
        runner,
        config,
        workspace_root,
        program,
        args,
        false,
        "exec",
        output,
    )
}

pub fn run_stored_command(
    runner: &impl CommandRunner,
    config: &Config,
    workspace_root: &Path,
    name: &str,
    extra_args: Vec<OsString>,
    output: &mut dyn Write,
) -> Result<()> {
    let command = config
        .commands
        .get(name)
        .ok_or_else(|| Error::Usage(format!("unknown stored command: {name}")))?;
    let mut args: Vec<OsString> = command.args.iter().map(OsString::from).collect();
    args.extend(extra_args);
    run_process_for_all(
        runner,
        config,
        workspace_root,
        command.program.clone().into(),
        args,
        false,
        name,
        output,
    )
}

fn run_git_for_all(
    runner: &impl CommandRunner,
    config: &Config,
    workspace_root: &Path,
    args: &[&str],
    operation: &str,
    output: &mut dyn Write,
) -> Result<()> {
    let args = args.iter().map(OsString::from).collect();
    run_process_for_all(
        runner,
        config,
        workspace_root,
        "git".into(),
        args,
        true,
        operation,
        output,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_process_for_all(
    runner: &impl CommandRunner,
    config: &Config,
    workspace_root: &Path,
    program: OsString,
    args: Vec<OsString>,
    capture: bool,
    operation: &str,
    output: &mut dyn Write,
) -> Result<()> {
    let mut failures = 0usize;
    for repo in &config.repos {
        let destination = config.repo_destination(workspace_root, repo)?;
        let name = &repo.name;
        writeln!(output, "== {name} ({}) ==", destination.display())
            .map_err(|error| Error::io("failed to write output", error))?;
        if !destination.join(".git").exists() {
            failures += 1;
            writeln!(output, "not cloned")
                .map_err(|error| Error::io("failed to write output", error))?;
            continue;
        }
        let result = runner.run(&ProcessSpec {
            program: program.clone(),
            args: args.clone(),
            cwd: Some(destination),
            capture,
        })?;
        if capture {
            write_captured(output, &result)?;
        }
        if !result.success {
            failures += 1;
            writeln!(
                output,
                "command exited with status {}",
                display_code(result.code)
            )
            .map_err(|error| Error::io("failed to write output", error))?;
        }
    }
    finish_operation(operation, failures)
}

fn write_captured(output: &mut dyn Write, result: &ProcessOutput) -> Result<()> {
    output
        .write_all(&result.stdout)
        .map_err(|error| Error::io("failed to write command stdout", error))?;
    output
        .write_all(&result.stderr)
        .map_err(|error| Error::io("failed to write command stderr", error))?;
    Ok(())
}

fn display_code(code: Option<i32>) -> String {
    code.map(|value| value.to_string())
        .unwrap_or_else(|| "signal".into())
}

fn finish_operation(operation: &str, failures: usize) -> Result<()> {
    if failures == 0 {
        Ok(())
    } else {
        Err(Error::Operation(format!(
            "{operation} failed for {failures} repository/repositories"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::ProcessOutput;
    use std::cell::RefCell;
    use tempfile::tempdir;

    #[derive(Default)]
    struct RecordingRunner {
        calls: RefCell<Vec<ProcessSpec>>,
    }

    impl CommandRunner for RecordingRunner {
        fn run(&self, spec: &ProcessSpec) -> Result<ProcessOutput> {
            self.calls.borrow_mut().push(spec.clone());
            Ok(ProcessOutput {
                success: true,
                code: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            })
        }
    }

    #[test]
    fn clone_always_uses_remote_repository_name() {
        let directory = tempdir().unwrap();
        let root = directory.path();
        let mut config = Config::new("demo".into(), PathBuf::from("sources"));
        config.repos.push(RepoConfig {
            name: "api".into(),
            url: "git@github.com:acme/api.git".into(),
            path: PathBuf::from("backend"),
        });
        let runner = RecordingRunner::default();
        clone_repositories(&runner, &config, root, &mut Vec::new()).unwrap();

        let calls = runner.calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].args.last().unwrap(),
            &root.join("sources/backend/api").into_os_string()
        );
    }

    #[test]
    fn status_uses_repository_as_working_directory() {
        let directory = tempdir().unwrap();
        let root = directory.path();
        let mut config = Config::new("demo".into(), PathBuf::from("repos"));
        config.repos.push(RepoConfig {
            name: "api".into(),
            url: "https://github.com/acme/api.git".into(),
            path: PathBuf::from("."),
        });
        fs::create_dir_all(root.join("repos/api/.git")).unwrap();
        let runner = RecordingRunner::default();
        git_status(&runner, &config, root, &mut Vec::new()).unwrap();

        let calls = runner.calls.borrow();
        assert_eq!(calls[0].cwd, Some(root.join("repos/api")));
        assert_eq!(calls[0].program, OsString::from("git"));
    }

    #[test]
    fn init_ignores_a_base_path_inside_the_workspace() {
        let directory = tempdir().unwrap();
        init_workspace(directory.path(), "demo".into(), PathBuf::from("sources")).unwrap();
        assert_eq!(
            fs::read_to_string(directory.path().join(".gitignore")).unwrap(),
            "sources/\n"
        );
    }

    #[test]
    fn init_does_not_ignore_a_base_path_outside_the_workspace() {
        let directory = tempdir().unwrap();
        init_workspace(directory.path(), "demo".into(), PathBuf::from("../sources")).unwrap();
        assert!(!directory.path().join(".gitignore").exists());
    }
}

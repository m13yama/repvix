use crate::config::{CONFIG_FILE, Config, RepoConfig, StoredCommand};
use crate::process::{CommandRunner, ProcessOutput, ProcessSpec};
use crate::{Error, Result};
use regex::Regex;
use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OutputOptions {
    pub color: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepositorySelector {
    Names(Vec<String>),
    Regex(String),
}

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
    branch: Option<String>,
) -> Result<PathBuf> {
    let repo = RepoConfig {
        name: crate::config::repo_name(&url)?,
        url,
        path: path.unwrap_or_else(|| PathBuf::from(".")),
        branch,
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
    branch_override: Option<String>,
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

    let branch = match branch_override {
        Some(branch) => branch,
        None => {
            let branch = git_capture(runner, &repository_path, &["branch", "--show-current"])?;
            let branch = branch.trim_end_matches(['\r', '\n']).to_owned();
            if branch.is_empty() {
                return Err(Error::Usage(format!(
                    "repository {name} has a detached HEAD; specify the branch to record with --branch"
                )));
            }
            branch
        }
    };
    let repo = RepoConfig {
        name,
        url,
        path,
        branch: Some(branch),
    };
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

pub fn import_repositories(
    runner: &impl CommandRunner,
    config: &mut Config,
    workspace_root: &Path,
    repository_paths: &[PathBuf],
    branch_override: Option<String>,
    output: &mut dyn Write,
    options: OutputOptions,
) -> Result<()> {
    if repository_paths.is_empty() {
        return Err(Error::Usage(
            "import requires at least one repository path".into(),
        ));
    }

    let mut rows = Vec::with_capacity(repository_paths.len());
    let mut imported = 0usize;
    let mut updated = 0usize;
    let mut failures = Vec::new();

    for repository_path in repository_paths {
        match import_repository(
            runner,
            config,
            workspace_root,
            repository_path,
            None,
            branch_override.clone(),
        ) {
            Ok(result) => {
                let action = match result.action {
                    ImportAction::Added => {
                        imported += 1;
                        "imported"
                    }
                    ImportAction::Updated => {
                        updated += 1;
                        "updated"
                    }
                };
                rows.push([
                    TableCell::new(action, CellStyle::Good),
                    TableCell::new(result.repo.name, CellStyle::Accent),
                    TableCell::plain(display_workspace_path(&result.destination, workspace_root)),
                ]);
            }
            Err(error) => {
                rows.push([
                    TableCell::new("failed", CellStyle::Bad),
                    TableCell::new(
                        display_workspace_path(repository_path, workspace_root),
                        CellStyle::Accent,
                    ),
                    TableCell::new("-", CellStyle::Muted),
                ]);
                failures.push((repository_path.clone(), error.to_string()));
            }
        }
    }

    write_table(
        output,
        ["RESULT", "REPOSITORY", "DESTINATION"],
        &rows,
        options.color,
    )?;
    writeln!(output).map_err(|error| Error::io("failed to write output", error))?;
    write_import_summary(output, imported, updated, failures.len(), options.color)?;

    if !failures.is_empty() {
        writeln!(output).map_err(|error| Error::io("failed to write output", error))?;
        write_styled(output, "Errors", CellStyle::Header, options.color)?;
        writeln!(output, ":").map_err(|error| Error::io("failed to write output", error))?;
        for (path, message) in &failures {
            writeln!(
                output,
                "  {}:",
                display_workspace_path(path, workspace_root)
            )
            .map_err(|error| Error::io("failed to write output", error))?;
            for line in message.lines() {
                writeln!(output, "    {line}")
                    .map_err(|error| Error::io("failed to write output", error))?;
            }
        }
    }

    finish_operation("import", failures.len())
}

pub fn find_repositories_matching(
    config: &Config,
    workspace_root: &Path,
    pattern: &str,
) -> Result<Vec<PathBuf>> {
    let matcher = Regex::new(pattern)
        .map_err(|error| Error::Usage(format!("invalid regular expression: {error}")))?;
    let base_path = config.base_path(workspace_root);
    if !base_path.is_dir() {
        return Err(Error::Usage(format!(
            "workspace base path does not exist or is not a directory: {}",
            base_path.display()
        )));
    }

    let mut repositories = Vec::new();
    discover_git_repositories(&base_path, &base_path, Some(&matcher), &mut repositories)?;
    repositories.sort();

    if repositories.is_empty() {
        return Err(Error::Usage(format!(
            "no Git repositories matched `{pattern}` below {}",
            base_path.display()
        )));
    }
    Ok(repositories)
}

pub fn remove_repositories(
    config: &mut Config,
    workspace_root: &Path,
    selector: RepositorySelector,
    output: &mut dyn Write,
    options: OutputOptions,
) -> Result<()> {
    let selected = select_repository_indices(config, workspace_root, selector)?;
    let previous = config.repos.clone();
    let mut removed = Vec::with_capacity(selected.len());
    config.repos = previous
        .iter()
        .enumerate()
        .filter_map(|(index, repo)| {
            if selected.contains(&index) {
                removed.push(repo.clone());
                None
            } else {
                Some(repo.clone())
            }
        })
        .collect();
    if let Err(error) = config.save(workspace_root) {
        config.repos = previous;
        return Err(error);
    }

    let rows = removed
        .iter()
        .map(|repo| {
            let destination = config.repo_destination(workspace_root, repo)?;
            Ok([
                TableCell::new("removed", CellStyle::Good),
                TableCell::new(&repo.name, CellStyle::Accent),
                TableCell::plain(display_workspace_path(&destination, workspace_root)),
            ])
        })
        .collect::<Result<Vec<_>>>()?;
    write_table(
        output,
        ["RESULT", "REPOSITORY", "DESTINATION"],
        &rows,
        options.color,
    )?;
    writeln!(output).map_err(|error| Error::io("failed to write output", error))?;
    let count = removed.len();
    let repository = if count == 1 {
        "repository"
    } else {
        "repositories"
    };
    let directory = if count == 1 {
        "working directory was"
    } else {
        "working directories were"
    };
    write_styled(output, "Summary:", CellStyle::Header, options.color)?;
    writeln!(
        output,
        " {count} {repository} removed from configuration; {directory} kept."
    )
    .map_err(|error| Error::io("failed to write output", error))
}

fn select_repository_indices(
    config: &Config,
    workspace_root: &Path,
    selector: RepositorySelector,
) -> Result<HashSet<usize>> {
    let selected = match selector {
        RepositorySelector::Regex(pattern) => {
            let matcher = Regex::new(&pattern)
                .map_err(|error| Error::Usage(format!("invalid regular expression: {error}")))?;
            config
                .repos
                .iter()
                .enumerate()
                .filter_map(|(index, repo)| {
                    matcher
                        .is_match(&repository_base_relative_path(repo))
                        .then_some(index)
                })
                .collect()
        }
        RepositorySelector::Names(names) => {
            if names.is_empty() {
                return Err(Error::Usage(
                    "remove requires at least one repository name or destination".into(),
                ));
            }
            let mut selected = HashSet::new();
            let mut problems = Vec::new();
            for name in names {
                let requested = Path::new(&name);
                let mut matches = Vec::new();
                for (index, repo) in config.repos.iter().enumerate() {
                    let destination = config.repo_destination(workspace_root, repo)?;
                    let workspace_relative = destination
                        .strip_prefix(workspace_root)
                        .unwrap_or(&destination);
                    if requested == repository_base_relative_path_buf(repo)
                        || requested == destination
                        || requested == workspace_relative
                    {
                        matches.push(index);
                    }
                }
                if matches.is_empty() {
                    matches.extend(
                        config
                            .repos
                            .iter()
                            .enumerate()
                            .filter_map(|(index, repo)| (repo.name == name).then_some(index)),
                    );
                }
                match matches.as_slice() {
                    [] => problems.push(format!("unknown repository: {name}")),
                    [index] => {
                        selected.insert(*index);
                    }
                    _ => problems.push(format!(
                        "ambiguous repository name `{name}`; use its destination"
                    )),
                }
            }
            if !problems.is_empty() {
                return Err(Error::Usage(problems.join("; ")));
            }
            selected
        }
    };

    if selected.is_empty() {
        return Err(Error::Usage("no repositories matched for removal".into()));
    }
    Ok(selected)
}

fn repository_base_relative_path(repo: &RepoConfig) -> String {
    repository_base_relative_path_buf(repo)
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn repository_base_relative_path_buf(repo: &RepoConfig) -> PathBuf {
    if repo.path == Path::new(".") {
        PathBuf::from(&repo.name)
    } else {
        repo.path.join(&repo.name)
    }
}

fn discover_git_repositories(
    directory: &Path,
    base_path: &Path,
    matcher: Option<&Regex>,
    repositories: &mut Vec<PathBuf>,
) -> Result<()> {
    let entries = fs::read_dir(directory)
        .map_err(|error| Error::io(format!("failed to read {}", directory.display()), error))?;
    let mut entries = entries
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|error| Error::io(format!("failed to read {}", directory.display()), error))?;
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let file_type = entry.file_type().map_err(|error| {
            Error::io(
                format!("failed to inspect {}", entry.path().display()),
                error,
            )
        })?;
        if !file_type.is_dir() || entry.file_name() == ".git" {
            continue;
        }

        let path = entry.path();
        if path.join(".git").exists() {
            let relative = path.strip_prefix(base_path).map_err(|_| {
                Error::Operation(format!(
                    "discovered repository escaped base path: {}",
                    path.display()
                ))
            })?;
            let candidate = relative
                .components()
                .map(|component| component.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            if matcher.is_none_or(|matcher| matcher.is_match(&candidate)) {
                repositories.push(path);
            }
            continue;
        }

        discover_git_repositories(&path, base_path, matcher, repositories)?;
    }
    Ok(())
}

fn warn_unconfigured_repositories(
    config: &Config,
    workspace_root: &Path,
    output: &mut dyn Write,
    options: OutputOptions,
) -> Result<()> {
    let base_path = config.base_path(workspace_root);
    if !base_path.is_dir() {
        return Ok(());
    }

    let mut discovered = Vec::new();
    if let Err(error) = discover_git_repositories(&base_path, &base_path, None, &mut discovered) {
        write_styled(output, "warning:", CellStyle::Warning, options.color)?;
        writeln!(
            output,
            " could not check for unconfigured repositories: {error}"
        )
        .map_err(|error| Error::io("failed to write output", error))?;
        writeln!(output).map_err(|error| Error::io("failed to write output", error))?;
        return Ok(());
    }
    let configured = config
        .repos
        .iter()
        .map(|repo| config.repo_destination(workspace_root, repo))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .map(|path| fs::canonicalize(&path).unwrap_or(path))
        .collect::<HashSet<_>>();
    let mut unconfigured = discovered
        .into_iter()
        .filter(|path| {
            let normalized = fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            !configured.contains(&normalized)
        })
        .collect::<Vec<_>>();
    unconfigured.sort();
    if unconfigured.is_empty() {
        return Ok(());
    }

    let count = unconfigured.len();
    let repositories = if count == 1 {
        "repository is"
    } else {
        "repositories are"
    };
    write_styled(output, "warning:", CellStyle::Warning, options.color)?;
    writeln!(
        output,
        " {count} Git {repositories} not configured and will be skipped by sync:"
    )
    .map_err(|error| Error::io("failed to write output", error))?;
    for path in unconfigured {
        writeln!(
            output,
            "  {}",
            display_workspace_path(&path, workspace_root)
        )
        .map_err(|error| Error::io("failed to write output", error))?;
    }
    writeln!(output).map_err(|error| Error::io("failed to write output", error))
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
                if let Some(branch) = &repo.branch {
                    writeln!(output, "checkout {name} -> {branch}")
                        .map_err(|error| Error::io("failed to write output", error))?;
                    let result = runner.run(&ProcessSpec {
                        program: "git".into(),
                        args: ["switch", "--", branch].map(OsString::from).to_vec(),
                        cwd: Some(destination),
                        capture: true,
                    })?;
                    write_captured(output, &result)?;
                    if !result.success {
                        failures += 1;
                    }
                } else {
                    writeln!(
                        output,
                        "skip  {name}: already cloned at {}",
                        destination.display()
                    )
                    .map_err(|error| Error::io("failed to write output", error))?;
                }
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
        let mut args = vec![OsString::from("clone")];
        if let Some(branch) = &repo.branch {
            args.extend([OsString::from("--branch"), branch.into()]);
        }
        args.extend([
            "--".into(),
            repo.url.clone().into(),
            destination.into_os_string(),
        ]);
        let result = runner.run(&ProcessSpec {
            program: "git".into(),
            args,
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
    list_repositories_with_options(config, workspace_root, output, OutputOptions::default())
}

pub fn list_repositories_with_options(
    config: &Config,
    workspace_root: &Path,
    output: &mut dyn Write,
    options: OutputOptions,
) -> Result<()> {
    if config.repos.is_empty() {
        return writeln!(output, "No repositories configured.")
            .map_err(|error| Error::io("failed to write output", error));
    }

    let mut rows = Vec::with_capacity(config.repos.len());
    for repo in &config.repos {
        let destination = config.repo_destination(workspace_root, repo)?;
        rows.push([
            TableCell::new(&repo.name, CellStyle::Accent),
            TableCell::plain(display_workspace_path(&destination, workspace_root)),
            TableCell::plain(repo.branch.as_deref().unwrap_or("-")),
            TableCell::new(&repo.url, CellStyle::Muted),
        ]);
    }

    write_table(
        output,
        ["NAME", "DESTINATION", "BRANCH", "REMOTE"],
        &rows,
        options.color,
    )
}

pub fn git_status(
    runner: &impl CommandRunner,
    config: &Config,
    workspace_root: &Path,
    output: &mut dyn Write,
) -> Result<()> {
    git_status_with_options(
        runner,
        config,
        workspace_root,
        output,
        OutputOptions::default(),
    )
}

pub fn git_status_with_options(
    runner: &impl CommandRunner,
    config: &Config,
    workspace_root: &Path,
    output: &mut dyn Write,
    options: OutputOptions,
) -> Result<()> {
    if config.repos.is_empty() {
        return writeln!(output, "No repositories configured.")
            .map_err(|error| Error::io("failed to write output", error));
    }

    let mut rows = Vec::with_capacity(config.repos.len());
    let mut failures = 0usize;
    let mut errors = Vec::new();
    let mut totals = StatusTotals::default();

    for repo in &config.repos {
        let destination = config.repo_destination(workspace_root, repo)?;
        let path = display_workspace_path(&destination, workspace_root);
        if !destination.join(".git").exists() {
            failures += 1;
            totals.not_cloned += 1;
            rows.push(status_row(
                &repo.name,
                "-",
                "-",
                "not cloned",
                "-",
                path,
                CellStyle::Bad,
            ));
            continue;
        }

        let result = runner.run(&ProcessSpec {
            program: "git".into(),
            args: [
                "--no-optional-locks",
                "status",
                "--porcelain=v2",
                "--branch",
                "--untracked-files=normal",
            ]
            .into_iter()
            .map(OsString::from)
            .collect(),
            cwd: Some(destination),
            capture: true,
        })?;

        if !result.success {
            failures += 1;
            totals.errors += 1;
            let code = display_code(result.code);
            let message = String::from_utf8_lossy(&result.stderr).trim().to_owned();
            if !message.is_empty() {
                errors.push((repo.name.clone(), message));
            }
            rows.push(status_row(
                &repo.name,
                "-",
                "-",
                "error",
                format!("exit {code}"),
                path,
                CellStyle::Bad,
            ));
            continue;
        }

        let source = match std::str::from_utf8(&result.stdout) {
            Ok(source) => source,
            Err(_) => {
                failures += 1;
                totals.errors += 1;
                errors.push((repo.name.clone(), "Git output was not valid UTF-8".into()));
                rows.push(status_row(
                    &repo.name,
                    "-",
                    "-",
                    "error",
                    "invalid Git output",
                    path,
                    CellStyle::Bad,
                ));
                continue;
            }
        };
        let summary = parse_git_status(source);
        let (state, state_style) = if summary.conflicts > 0 {
            totals.conflicts += 1;
            ("conflict", CellStyle::Bad)
        } else if summary.has_changes() {
            totals.changed += 1;
            ("changed", CellStyle::Warning)
        } else {
            totals.clean += 1;
            ("clean", CellStyle::Good)
        };
        rows.push(status_row(
            &repo.name,
            summary.display_branch(),
            summary.display_sync(),
            state,
            summary.display_changes(),
            path,
            state_style,
        ));
    }

    write_table(
        output,
        ["NAME", "BRANCH", "SYNC", "STATE", "CHANGES", "DESTINATION"],
        &rows,
        options.color,
    )?;
    writeln!(output).map_err(|error| Error::io("failed to write output", error))?;
    write_status_summary(output, totals, options.color)?;

    if !errors.is_empty() {
        writeln!(output).map_err(|error| Error::io("failed to write output", error))?;
        write_styled(output, "Errors", CellStyle::Header, options.color)?;
        writeln!(output, ":").map_err(|error| Error::io("failed to write output", error))?;
        for (name, message) in errors {
            writeln!(output, "  {name}: {message}")
                .map_err(|error| Error::io("failed to write output", error))?;
        }
    }

    finish_operation("status", failures)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CellStyle {
    Plain,
    Header,
    Accent,
    Muted,
    Good,
    Warning,
    Bad,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TableCell {
    text: String,
    style: CellStyle,
}

impl TableCell {
    fn new(text: impl Into<String>, style: CellStyle) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }

    fn plain(text: impl Into<String>) -> Self {
        Self::new(text, CellStyle::Plain)
    }
}

fn write_table<const COLUMNS: usize>(
    output: &mut dyn Write,
    headers: [&str; COLUMNS],
    rows: &[[TableCell; COLUMNS]],
    color: bool,
) -> Result<()> {
    let mut widths = headers.map(|header| header.chars().count());
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            widths[index] = widths[index].max(cell.text.chars().count());
        }
    }

    for (index, header) in headers.iter().enumerate() {
        write_styled(output, header, CellStyle::Header, color)?;
        write_column_gap(
            output,
            widths[index] - header.chars().count(),
            index,
            COLUMNS,
        )?;
    }
    writeln!(output).map_err(|error| Error::io("failed to write output", error))?;

    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            write_styled(output, &cell.text, cell.style, color)?;
            write_column_gap(
                output,
                widths[index] - cell.text.chars().count(),
                index,
                COLUMNS,
            )?;
        }
        writeln!(output).map_err(|error| Error::io("failed to write output", error))?;
    }
    Ok(())
}

fn write_column_gap(
    output: &mut dyn Write,
    padding: usize,
    index: usize,
    columns: usize,
) -> Result<()> {
    if index + 1 == columns {
        return Ok(());
    }
    write!(output, "{:width$}", "", width = padding + 2)
        .map_err(|error| Error::io("failed to write output", error))
}

fn write_styled(output: &mut dyn Write, text: &str, style: CellStyle, color: bool) -> Result<()> {
    let escape = match style {
        CellStyle::Plain => "",
        CellStyle::Header => "\x1b[1m",
        CellStyle::Accent => "\x1b[1;36m",
        CellStyle::Muted => "\x1b[2m",
        CellStyle::Good => "\x1b[32m",
        CellStyle::Warning => "\x1b[33m",
        CellStyle::Bad => "\x1b[31m",
    };
    if color && !escape.is_empty() {
        write!(output, "{escape}{text}\x1b[0m")
    } else {
        write!(output, "{text}")
    }
    .map_err(|error| Error::io("failed to write output", error))
}

fn display_workspace_path(path: &Path, workspace_root: &Path) -> String {
    match path.strip_prefix(workspace_root) {
        Ok(relative) if relative.as_os_str().is_empty() => ".".into(),
        Ok(relative) => relative.display().to_string(),
        Err(_) => path.display().to_string(),
    }
}

fn status_row(
    name: impl Into<String>,
    branch: impl Into<String>,
    sync: impl Into<String>,
    state: impl Into<String>,
    changes: impl Into<String>,
    destination: impl Into<String>,
    state_style: CellStyle,
) -> [TableCell; 6] {
    let sync = sync.into();
    let sync_style = if sync == "-" {
        CellStyle::Muted
    } else if sync == "up to date" {
        CellStyle::Good
    } else {
        CellStyle::Warning
    };
    let changes = changes.into();
    let changes_style = if changes == "-" {
        CellStyle::Muted
    } else {
        state_style
    };
    [
        TableCell::new(name, CellStyle::Accent),
        TableCell::new(branch, CellStyle::Accent),
        TableCell::new(sync, sync_style),
        TableCell::new(state, state_style),
        TableCell::new(changes, changes_style),
        TableCell::new(destination, CellStyle::Muted),
    ]
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct GitStatusSummary {
    branch: Option<String>,
    oid: Option<String>,
    upstream: Option<String>,
    ahead: usize,
    behind: usize,
    staged: usize,
    modified: usize,
    untracked: usize,
    conflicts: usize,
}

impl GitStatusSummary {
    fn has_changes(&self) -> bool {
        self.staged + self.modified + self.untracked + self.conflicts > 0
    }

    fn display_branch(&self) -> String {
        match self.branch.as_deref() {
            Some("(detached)") => self
                .oid
                .as_deref()
                .filter(|oid| !oid.starts_with('('))
                .map(|oid| format!("detached@{}", &oid[..oid.len().min(7)]))
                .unwrap_or_else(|| "detached".into()),
            Some(branch) => branch.into(),
            None => "-".into(),
        }
    }

    fn display_sync(&self) -> String {
        if self.upstream.is_none() {
            return "-".into();
        }
        match (self.ahead, self.behind) {
            (0, 0) => "up to date".into(),
            (ahead, 0) => format!("↑{ahead}"),
            (0, behind) => format!("↓{behind}"),
            (ahead, behind) => format!("↑{ahead} ↓{behind}"),
        }
    }

    fn display_changes(&self) -> String {
        let mut changes = Vec::new();
        if self.staged > 0 {
            changes.push(format!("{} staged", self.staged));
        }
        if self.modified > 0 {
            changes.push(format!("{} modified", self.modified));
        }
        if self.untracked > 0 {
            changes.push(format!("{} untracked", self.untracked));
        }
        if self.conflicts > 0 {
            changes.push(format!("{} conflicted", self.conflicts));
        }
        if changes.is_empty() {
            "-".into()
        } else {
            changes.join(", ")
        }
    }
}

fn parse_git_status(source: &str) -> GitStatusSummary {
    let mut summary = GitStatusSummary::default();
    for line in source.lines() {
        if let Some(value) = line.strip_prefix("# branch.oid ") {
            summary.oid = Some(value.into());
        } else if let Some(value) = line.strip_prefix("# branch.head ") {
            summary.branch = Some(value.into());
        } else if let Some(value) = line.strip_prefix("# branch.upstream ") {
            summary.upstream = Some(value.into());
        } else if let Some(value) = line.strip_prefix("# branch.ab ") {
            for count in value.split_ascii_whitespace() {
                if let Some(ahead) = count.strip_prefix('+') {
                    summary.ahead = ahead.parse().unwrap_or(0);
                } else if let Some(behind) = count.strip_prefix('-') {
                    summary.behind = behind.parse().unwrap_or(0);
                }
            }
        } else if line.starts_with("1 ") || line.starts_with("2 ") {
            if let Some(status) = line.split_ascii_whitespace().nth(1) {
                let mut status = status.chars();
                if status.next().is_some_and(|value| value != '.') {
                    summary.staged += 1;
                }
                if status.next().is_some_and(|value| value != '.') {
                    summary.modified += 1;
                }
            }
        } else if line.starts_with("u ") {
            summary.conflicts += 1;
        } else if line.starts_with("? ") {
            summary.untracked += 1;
        }
    }
    summary
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct StatusTotals {
    clean: usize,
    changed: usize,
    conflicts: usize,
    not_cloned: usize,
    errors: usize,
}

fn write_import_summary(
    output: &mut dyn Write,
    imported: usize,
    updated: usize,
    failed: usize,
    color: bool,
) -> Result<()> {
    write_styled(output, "Summary:", CellStyle::Header, color)?;
    write!(output, " ").map_err(|error| Error::io("failed to write output", error))?;
    write_styled(
        output,
        &format!("{imported} imported"),
        CellStyle::Good,
        color,
    )?;
    write!(output, ", ").map_err(|error| Error::io("failed to write output", error))?;
    write_styled(
        output,
        &format!("{updated} updated"),
        CellStyle::Accent,
        color,
    )?;
    if failed > 0 {
        write!(output, ", ").map_err(|error| Error::io("failed to write output", error))?;
        write_styled(output, &format!("{failed} failed"), CellStyle::Bad, color)?;
    }
    writeln!(output).map_err(|error| Error::io("failed to write output", error))
}

fn write_status_summary(output: &mut dyn Write, totals: StatusTotals, color: bool) -> Result<()> {
    write_styled(output, "Summary:", CellStyle::Header, color)?;
    let items = [
        (totals.clean, "clean", CellStyle::Good),
        (totals.changed, "changed", CellStyle::Warning),
        (totals.conflicts, "conflict", CellStyle::Bad),
        (totals.not_cloned, "not cloned", CellStyle::Bad),
        (totals.errors, "error", CellStyle::Bad),
    ];
    let mut first = true;
    for (count, label, style) in items {
        if count == 0 {
            continue;
        }
        if first {
            write!(output, " ").map_err(|error| Error::io("failed to write output", error))?;
            first = false;
        } else {
            write!(output, ", ").map_err(|error| Error::io("failed to write output", error))?;
        }
        write_styled(output, &format!("{count} {label}"), style, color)?;
    }
    writeln!(output).map_err(|error| Error::io("failed to write output", error))
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
    sync_with_options(
        runner,
        config,
        workspace_root,
        rebase,
        output,
        OutputOptions::default(),
    )
}

pub fn sync_with_options(
    runner: &impl CommandRunner,
    config: &Config,
    workspace_root: &Path,
    rebase: bool,
    output: &mut dyn Write,
    options: OutputOptions,
) -> Result<()> {
    warn_unconfigured_repositories(config, workspace_root, output, options)?;
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
        let repositories = if failures == 1 {
            "repository"
        } else {
            "repositories"
        };
        Err(Error::Operation(format!(
            "{operation} failed for {failures} {repositories}"
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
            branch: None,
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
            branch: None,
        });
        fs::create_dir_all(root.join("repos/api/.git")).unwrap();
        let runner = RecordingRunner::default();
        git_status(&runner, &config, root, &mut Vec::new()).unwrap();

        let calls = runner.calls.borrow();
        assert_eq!(calls[0].cwd, Some(root.join("repos/api")));
        assert_eq!(calls[0].program, OsString::from("git"));
        assert_eq!(
            calls[0].args,
            [
                "--no-optional-locks",
                "status",
                "--porcelain=v2",
                "--branch",
                "--untracked-files=normal",
            ]
            .map(OsString::from)
        );
    }

    #[test]
    fn parses_branch_sync_and_change_counts_from_porcelain_v2() {
        let summary = parse_git_status(
            "# branch.oid 0123456789abcdef\n\
             # branch.head feature/dashboard\n\
             # branch.upstream origin/feature/dashboard\n\
             # branch.ab +2 -1\n\
             1 M. N... 100644 100644 100644 a b staged.txt\n\
             1 .M N... 100644 100644 100644 a b modified.txt\n\
             2 RM N... 100644 100644 100644 a b R100 renamed.txt\n\
             u UU N... 100644 100644 100644 100644 a b c conflict.txt\n\
             ? notes.txt\n",
        );

        assert_eq!(summary.display_branch(), "feature/dashboard");
        assert_eq!(summary.display_sync(), "↑2 ↓1");
        assert_eq!(summary.staged, 2);
        assert_eq!(summary.modified, 2);
        assert_eq!(summary.untracked, 1);
        assert_eq!(summary.conflicts, 1);
        assert_eq!(
            summary.display_changes(),
            "2 staged, 2 modified, 1 untracked, 1 conflicted"
        );
    }

    #[test]
    fn list_uses_a_header_and_workspace_relative_destinations() {
        let directory = tempdir().unwrap();
        let root = directory.path();
        let mut config = Config::new("demo".into(), PathBuf::from("sources"));
        config.repos.push(RepoConfig {
            name: "api".into(),
            url: "git@github.com:acme/api.git".into(),
            path: PathBuf::from("services"),
            branch: Some("release/next".into()),
        });
        let mut output = Vec::new();

        list_repositories(&config, root, &mut output).unwrap();

        let output = String::from_utf8(output).unwrap();
        assert!(output.starts_with("NAME  DESTINATION"), "{output}");
        assert!(output.contains("api   sources/services/api"), "{output}");
        assert!(!output.contains(&root.display().to_string()), "{output}");
        assert!(!output.contains('\t'), "{output}");
    }

    #[test]
    fn colored_tables_keep_alignment_outside_escape_sequences() {
        let directory = tempdir().unwrap();
        let root = directory.path();
        let mut config = Config::new("demo".into(), PathBuf::from("repos"));
        config.repos.push(RepoConfig {
            name: "api".into(),
            url: "https://github.com/acme/api.git".into(),
            path: PathBuf::from("."),
            branch: None,
        });
        let mut output = Vec::new();

        list_repositories_with_options(&config, root, &mut output, OutputOptions { color: true })
            .unwrap();

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("\x1b[1mNAME\x1b[0m"), "{output:?}");
        assert!(output.contains("\x1b[1;36mapi\x1b[0m"), "{output:?}");
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

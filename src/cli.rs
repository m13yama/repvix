use crate::app;
use crate::config::{CONFIG_FILE, Config, StoredCommand, find_workspace};
use crate::process::SystemRunner;
use crate::{Error, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::ffi::OsString;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Parser)]
#[command(
    name = "repvix",
    version,
    about = "Manage multiple Git repositories as one workspace"
)]
pub struct Cli {
    #[command(subcommand)]
    command: CliCommand,
}

#[derive(Debug, Subcommand)]
enum CliCommand {
    /// Initialize a workspace.
    Init {
        #[arg(long)]
        name: Option<String>,
        /// Clone base path. Relative paths are resolved from repvix.yaml.
        #[arg(long, default_value = "repos")]
        base_path: PathBuf,
    },
    /// Add a repository. PATH is a parent below base_path, not a new repository name.
    Add {
        url: String,
        #[arg(long)]
        path: Option<PathBuf>,
    },
    /// Import one or more existing repositories below workspace.base_path.
    Import {
        #[arg(
            num_args = 1..,
            required_unless_present = "regex",
            conflicts_with = "regex"
        )]
        repositories: Vec<PathBuf>,
        /// Override the repository's origin URL. Only valid with one repository.
        #[arg(long, conflicts_with = "regex")]
        url: Option<String>,
        /// Discover repositories below base_path matching a relative-path regex.
        #[arg(long)]
        regex: Option<String>,
    },
    /// Remove repositories from repvix.yaml without deleting working directories.
    Remove {
        /// Repository names or destinations.
        #[arg(
            num_args = 1..,
            required_unless_present = "regex",
            conflicts_with = "regex"
        )]
        repositories: Vec<String>,
        /// Select repositories by their base-path-relative destination.
        #[arg(long)]
        regex: Option<String>,
    },
    /// Clone all configured repositories.
    Clone,
    /// List repositories and their resolved destinations.
    List,
    /// Show Git status for every repository.
    Status,
    /// Create the same branch in every repository.
    Branch { name: String },
    /// Check out the same branch in every repository.
    Checkout { branch: String },
    /// Pull changes in every repository.
    Sync {
        #[arg(long)]
        rebase: bool,
    },
    /// Execute a program directly in every repository. Use `--` before program flags.
    Exec(ExecArgs),
    /// Manage named commands stored in repvix.yaml.
    Command {
        #[command(subcommand)]
        action: StoredCommandAction,
    },
    /// Run a stored command in every repository.
    Run {
        name: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        extra_args: Vec<OsString>,
    },
    /// Generate a shell completion script.
    Completions {
        #[arg(value_enum)]
        shell: CompletionShell,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum CompletionShell {
    Bash,
}

#[derive(Debug, Args)]
struct ExecArgs {
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    command: Vec<OsString>,
}

#[derive(Debug, Subcommand)]
enum StoredCommandAction {
    /// Store a command. Everything after `--` is saved as program and arguments.
    Add {
        name: String,
        #[arg(long)]
        description: Option<String>,
        #[arg(required = true, last = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    Remove {
        name: String,
    },
    List,
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let current = std::env::current_dir()
        .map_err(|error| Error::io("failed to determine current directory", error))?;
    let stdout = io::stdout();
    let output_options = app::OutputOptions {
        color: stdout.is_terminal()
            && std::env::var_os("NO_COLOR").is_none()
            && std::env::var_os("TERM").is_none_or(|term| term != "dumb"),
    };
    let mut stdout = stdout.lock();
    execute(cli, &current, &SystemRunner, &mut stdout, output_options)
}

fn execute(
    cli: Cli,
    current: &Path,
    runner: &SystemRunner,
    output: &mut dyn Write,
    output_options: app::OutputOptions,
) -> Result<()> {
    if let CliCommand::Completions { shell } = cli.command {
        return generate_completions(shell, output);
    }
    if let CliCommand::Init { name, base_path } = cli.command {
        let inferred_name = current
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("workspace")
            .to_owned();
        let config = app::init_workspace(current, name.unwrap_or(inferred_name), base_path)?;
        writeln!(
            output,
            "initialized {} with base path {}",
            current.join(CONFIG_FILE).display(),
            config.base_path(current).display()
        )
        .map_err(|error| Error::io("failed to write output", error))?;
        return Ok(());
    }

    let workspace_root = find_workspace(current)?;
    let mut config = Config::load(&workspace_root)?;
    match cli.command {
        CliCommand::Init { .. } => unreachable!(),
        CliCommand::Completions { .. } => unreachable!(),
        CliCommand::Add { url, path } => {
            let destination = app::add_repository(&mut config, &workspace_root, url, path)?;
            writeln!(output, "added repository -> {}", destination.display())
                .map_err(|error| Error::io("failed to write output", error))
        }
        CliCommand::Import {
            mut repositories,
            url,
            regex,
        } => {
            if repositories.len() > 1 && url.is_some() {
                return Err(Error::Usage(
                    "--url can only be used when importing one repository".into(),
                ));
            }
            let regex_mode = regex.is_some();
            if let Some(regex) = regex.as_deref() {
                repositories = app::find_repositories_matching(&config, &workspace_root, regex)?;
            } else {
                for repository in &mut repositories {
                    if !repository.is_absolute() {
                        *repository = current.join(&*repository);
                    }
                }
            }

            if repositories.len() == 1 && !regex_mode {
                let imported = app::import_repository(
                    runner,
                    &mut config,
                    &workspace_root,
                    &repositories[0],
                    url,
                )?;
                let action = match imported.action {
                    app::ImportAction::Added => "imported",
                    app::ImportAction::Updated => "updated",
                };
                writeln!(
                    output,
                    "{action} {} -> {}",
                    imported.repo.name,
                    imported.destination.display()
                )
                .map_err(|error| Error::io("failed to write output", error))
            } else {
                app::import_repositories(
                    runner,
                    &mut config,
                    &workspace_root,
                    &repositories,
                    output,
                    output_options,
                )
            }
        }
        CliCommand::Remove {
            repositories,
            regex,
        } => {
            let selector = match regex {
                Some(regex) => app::RepositorySelector::Regex(regex),
                None => app::RepositorySelector::Names(repositories),
            };
            app::remove_repositories(
                &mut config,
                &workspace_root,
                selector,
                output,
                output_options,
            )
        }
        CliCommand::Clone => app::clone_repositories(runner, &config, &workspace_root, output),
        CliCommand::List => {
            app::list_repositories_with_options(&config, &workspace_root, output, output_options)
        }
        CliCommand::Status => {
            app::git_status_with_options(runner, &config, &workspace_root, output, output_options)
        }
        CliCommand::Branch { name } => {
            app::create_branch(runner, &config, &workspace_root, &name, output)
        }
        CliCommand::Checkout { branch } => {
            app::checkout(runner, &config, &workspace_root, &branch, output)
        }
        CliCommand::Sync { rebase } => app::sync_with_options(
            runner,
            &config,
            &workspace_root,
            rebase,
            output,
            output_options,
        ),
        CliCommand::Exec(args) => {
            let mut command = args.command.into_iter();
            let program = command
                .next()
                .ok_or_else(|| Error::Usage("exec requires a program".into()))?;
            app::exec_for_all(
                runner,
                &config,
                &workspace_root,
                program,
                command.collect(),
                output,
            )
        }
        CliCommand::Command { action } => match action {
            StoredCommandAction::Add {
                name,
                description,
                command,
            } => {
                let mut parts = command.into_iter();
                let program = parts.next().ok_or_else(|| {
                    Error::Usage("command add requires a program after `--`".into())
                })?;
                app::add_command(
                    &mut config,
                    &workspace_root,
                    name.clone(),
                    StoredCommand {
                        description,
                        program,
                        args: parts.collect(),
                    },
                )?;
                writeln!(output, "stored command `{name}`")
                    .map_err(|error| Error::io("failed to write output", error))
            }
            StoredCommandAction::Remove { name } => {
                app::remove_command(&mut config, &workspace_root, &name)?;
                writeln!(output, "removed command `{name}`")
                    .map_err(|error| Error::io("failed to write output", error))
            }
            StoredCommandAction::List => {
                for (name, command) in &config.commands {
                    let description = command.description.as_deref().unwrap_or("");
                    writeln!(
                        output,
                        "{name}\t{} {}\t{description}",
                        command.program,
                        command.args.join(" ")
                    )
                    .map_err(|error| Error::io("failed to write output", error))?;
                }
                Ok(())
            }
        },
        CliCommand::Run { name, extra_args } => {
            app::run_stored_command(runner, &config, &workspace_root, &name, extra_args, output)
        }
    }
}

fn generate_completions(shell: CompletionShell, output: &mut dyn Write) -> Result<()> {
    let script = match shell {
        CompletionShell::Bash => BASH_COMPLETION,
    };
    output
        .write_all(script.as_bytes())
        .map_err(|error| Error::io("failed to write completion script", error))
}

const BASH_COMPLETION: &str = r#"# bash completion for repvix
_repvix() {
    local cur prev command candidates action
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD-1]}"
    command="${COMP_WORDS[1]}"

    if (( COMP_CWORD == 1 )); then
        candidates="init add import remove clone list status branch checkout sync exec command run completions help"
        COMPREPLY=( $(compgen -W "$candidates" -- "$cur") )
        return
    fi

    case "$prev" in
        --base-path|--path)
            COMPREPLY=( $(compgen -d -- "$cur") )
            return
            ;;
        --url|--name|--description|--regex)
            return
            ;;
    esac

    case "$command" in
        init)
            candidates="--name --base-path --help"
            ;;
        add)
            candidates="--path --help"
            ;;
        import)
            if [[ "$cur" != -* && " ${COMP_WORDS[*]} " != *" --regex "* ]]; then
                COMPREPLY=( $(compgen -d -- "$cur") )
                return
            fi
            candidates="--url --regex --help"
            ;;
        remove)
            candidates="--regex --help $(repvix list 2>/dev/null | awk 'NR > 1 {print $1}')"
            ;;
        clone|list|status)
            candidates="--help"
            ;;
        branch|checkout)
            candidates="--help"
            ;;
        sync)
            candidates="--rebase --help"
            ;;
        exec)
            candidates="--help"
            ;;
        command)
            if (( COMP_CWORD == 2 )); then
                candidates="add remove list help"
            else
                action="${COMP_WORDS[2]}"
                case "$action" in
                    add)
                        candidates="--description --help --"
                        ;;
                    remove)
                        if (( COMP_CWORD == 3 )); then
                            candidates="$(repvix command list 2>/dev/null | cut -f1)"
                        else
                            candidates="--help"
                        fi
                        ;;
                    *)
                        candidates="--help"
                        ;;
                esac
            fi
            ;;
        run)
            if (( COMP_CWORD == 2 )); then
                candidates="$(repvix command list 2>/dev/null | cut -f1)"
            else
                candidates="--help"
            fi
            ;;
        completions)
            candidates="bash --help"
            ;;
        help)
            candidates="init add import remove clone list status branch checkout sync exec command run completions"
            ;;
        *)
            candidates="--help --version"
            ;;
    esac

    COMPREPLY=( $(compgen -W "$candidates" -- "$cur") )
}

complete -F _repvix repvix
"#;

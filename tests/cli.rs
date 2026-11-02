use std::fs;
use std::process::Command;
use tempfile::tempdir;

fn repvix() -> Command {
    Command::new(env!("CARGO_BIN_EXE_repvix"))
}

#[test]
fn init_add_and_list_preserve_repository_name() {
    let directory = tempdir().unwrap();

    let init = repvix()
        .current_dir(directory.path())
        .args(["init", "--name", "demo", "--base-path", "sources"])
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );

    let add = repvix()
        .current_dir(directory.path())
        .args(["add", "git@github.com:acme/api.git", "--path", "services"])
        .output()
        .unwrap();
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );

    let list = repvix()
        .current_dir(directory.path())
        .arg("list")
        .output()
        .unwrap();
    assert!(
        list.status.success(),
        "{}",
        String::from_utf8_lossy(&list.stderr)
    );
    let stdout = String::from_utf8(list.stdout).unwrap();
    let header = stdout.lines().next().unwrap();
    assert!(header.starts_with("NAME  DESTINATION"), "{stdout}");
    assert!(header.ends_with("REMOTE"), "{stdout}");
    assert!(stdout.contains("sources/services/api"), "{stdout}");
    assert!(!stdout.contains("sources/services\t"), "{stdout}");
    assert!(!stdout.contains("\x1b["), "{stdout:?}");

    let yaml = fs::read_to_string(directory.path().join("repvix.yaml")).unwrap();
    assert!(yaml.contains("version: 2"), "{yaml}");
    assert!(yaml.contains("base_path: sources"), "{yaml}");
    assert!(yaml.contains("name: api"), "{yaml}");
    assert!(yaml.contains("path: services"), "{yaml}");
    assert!(!yaml.contains("tags:"), "{yaml}");
}

#[test]
fn status_summarizes_each_repository_in_a_table() {
    let directory = tempdir().unwrap();
    assert!(
        repvix()
            .current_dir(directory.path())
            .arg("init")
            .status()
            .unwrap()
            .success()
    );
    assert!(
        repvix()
            .current_dir(directory.path())
            .args(["add", "git@github.com:acme/api.git"])
            .status()
            .unwrap()
            .success()
    );
    let repository = directory.path().join("repos/api");
    fs::create_dir_all(&repository).unwrap();
    assert!(
        Command::new("git")
            .current_dir(&repository)
            .args(["init", "--quiet", "--initial-branch", "main"])
            .status()
            .unwrap()
            .success()
    );
    fs::write(repository.join("notes.txt"), "not committed\n").unwrap();

    let status = repvix()
        .current_dir(directory.path())
        .arg("status")
        .output()
        .unwrap();

    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let stdout = String::from_utf8(status.stdout).unwrap();
    assert!(
        stdout.starts_with("NAME  BRANCH  SYNC  STATE    CHANGES      DESTINATION\n"),
        "{stdout}"
    );
    assert!(
        stdout.contains("api   main    -     changed  1 untracked  repos/api"),
        "{stdout}"
    );
    assert!(stdout.contains("Summary: 1 changed"), "{stdout}");
    assert!(!stdout.contains("\x1b["), "{stdout:?}");
}

#[test]
fn import_updates_a_legacy_local_path_entry_from_origin() {
    let directory = tempdir().unwrap();
    let repository = directory.path().join("sources/api");
    fs::create_dir_all(&repository).unwrap();
    assert!(
        Command::new("git")
            .current_dir(&repository)
            .args(["init", "--quiet"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .current_dir(&repository)
            .args(["remote", "add", "origin", "git@github.com:acme/api.git",])
            .status()
            .unwrap()
            .success()
    );
    fs::write(
        directory.path().join("repvix.yaml"),
        r#"version: 1
workspace:
  name: demo
  base_path: ./sources
repos:
- url: ./sources/api
"#,
    )
    .unwrap();

    let imported = repvix()
        .current_dir(directory.path())
        .args(["import", "./sources/api"])
        .output()
        .unwrap();
    assert!(
        imported.status.success(),
        "{}",
        String::from_utf8_lossy(&imported.stderr)
    );
    assert!(
        String::from_utf8_lossy(&imported.stdout).contains("updated api"),
        "{}",
        String::from_utf8_lossy(&imported.stdout)
    );

    let yaml = fs::read_to_string(directory.path().join("repvix.yaml")).unwrap();
    assert!(yaml.contains("version: 2"), "{yaml}");
    assert!(yaml.contains("name: api"), "{yaml}");
    assert!(yaml.contains("url: git@github.com:acme/api.git"), "{yaml}");
    assert!(yaml.contains("path: ."), "{yaml}");
}

#[test]
fn bulk_import_continues_after_a_failure_and_reports_a_summary() {
    let directory = tempdir().unwrap();
    assert!(
        repvix()
            .current_dir(directory.path())
            .arg("init")
            .output()
            .unwrap()
            .status
            .success()
    );

    for name in ["api", "web"] {
        let repository = directory.path().join("repos").join(name);
        fs::create_dir_all(&repository).unwrap();
        assert!(
            Command::new("git")
                .current_dir(&repository)
                .args(["init", "--quiet"])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .current_dir(&repository)
                .args([
                    "remote",
                    "add",
                    "origin",
                    &format!("git@github.com:acme/{name}.git"),
                ])
                .status()
                .unwrap()
                .success()
        );
    }

    let imported = repvix()
        .current_dir(directory.path())
        .args(["import", "./repos/api", "./repos/missing", "./repos/web"])
        .output()
        .unwrap();

    assert!(!imported.status.success());
    let stdout = String::from_utf8(imported.stdout).unwrap();
    let rows: Vec<Vec<&str>> = stdout
        .lines()
        .map(|line| line.split_whitespace().collect())
        .collect();
    assert!(
        rows.iter()
            .any(|row| row.as_slice() == ["imported", "api", "repos/api"]),
        "{stdout}"
    );
    assert!(
        rows.iter()
            .any(|row| row.as_slice() == ["failed", "repos/missing", "-"]),
        "{stdout}"
    );
    assert!(
        rows.iter()
            .any(|row| row.as_slice() == ["imported", "web", "repos/web"]),
        "{stdout}"
    );
    assert!(
        stdout.contains("Summary: 2 imported, 0 updated, 1 failed"),
        "{stdout}"
    );
    assert!(stdout.contains("Errors:\n  repos/missing:"), "{stdout}");
    assert!(!stdout.contains("\x1b["), "{stdout:?}");
    assert!(
        String::from_utf8_lossy(&imported.stderr).contains("import failed for 1 repository"),
        "{}",
        String::from_utf8_lossy(&imported.stderr)
    );

    let yaml = fs::read_to_string(directory.path().join("repvix.yaml")).unwrap();
    assert!(yaml.contains("name: api"), "{yaml}");
    assert!(yaml.contains("name: web"), "{yaml}");
}

#[test]
fn bulk_import_rejects_a_shared_url_override() {
    let directory = tempdir().unwrap();
    assert!(
        repvix()
            .current_dir(directory.path())
            .arg("init")
            .output()
            .unwrap()
            .status
            .success()
    );

    let imported = repvix()
        .current_dir(directory.path())
        .args([
            "import",
            "./repos/api",
            "./repos/web",
            "--url",
            "git@github.com:acme/api.git",
        ])
        .output()
        .unwrap();

    assert!(!imported.status.success());
    assert!(
        String::from_utf8_lossy(&imported.stderr)
            .contains("--url can only be used when importing one repository"),
        "{}",
        String::from_utf8_lossy(&imported.stderr)
    );
}

#[test]
fn regex_import_discovers_repositories_below_the_base_path() {
    let directory = tempdir().unwrap();
    assert!(
        repvix()
            .current_dir(directory.path())
            .arg("init")
            .output()
            .unwrap()
            .status
            .success()
    );

    for (parent, name) in [("apps", "api"), ("apps", "web"), ("services", "worker")] {
        let repository = directory.path().join("repos").join(parent).join(name);
        fs::create_dir_all(&repository).unwrap();
        assert!(
            Command::new("git")
                .current_dir(&repository)
                .args(["init", "--quiet"])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .current_dir(&repository)
                .args([
                    "remote",
                    "add",
                    "origin",
                    &format!("git@github.com:acme/{name}.git"),
                ])
                .status()
                .unwrap()
                .success()
        );
    }

    let apps = repvix()
        .current_dir(directory.path())
        .args(["import", "--regex", "^apps/"])
        .output()
        .unwrap();
    assert!(
        apps.status.success(),
        "{}",
        String::from_utf8_lossy(&apps.stderr)
    );
    let stdout = String::from_utf8(apps.stdout).unwrap();
    assert!(
        stdout.contains("Summary: 2 imported, 0 updated"),
        "{stdout}"
    );
    let yaml = fs::read_to_string(directory.path().join("repvix.yaml")).unwrap();
    assert!(yaml.contains("name: api"), "{yaml}");
    assert!(yaml.contains("name: web"), "{yaml}");
    assert!(!yaml.contains("name: worker"), "{yaml}");

    let all = repvix()
        .current_dir(directory.path())
        .args(["import", "--regex", ".*"])
        .output()
        .unwrap();
    assert!(
        all.status.success(),
        "{}",
        String::from_utf8_lossy(&all.stderr)
    );
    let stdout = String::from_utf8(all.stdout).unwrap();
    assert!(
        stdout.contains("Summary: 1 imported, 2 updated"),
        "{stdout}"
    );
    let yaml = fs::read_to_string(directory.path().join("repvix.yaml")).unwrap();
    assert!(yaml.contains("name: worker"), "{yaml}");

    let invalid = repvix()
        .current_dir(directory.path())
        .args(["import", "--regex", "["])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert!(
        String::from_utf8_lossy(&invalid.stderr).contains("invalid regular expression"),
        "{}",
        String::from_utf8_lossy(&invalid.stderr)
    );
}

#[test]
fn remove_only_updates_configuration_and_sync_warns_about_the_kept_repository() {
    let directory = tempdir().unwrap();
    assert!(
        repvix()
            .current_dir(directory.path())
            .arg("init")
            .output()
            .unwrap()
            .status
            .success()
    );
    for (name, parent) in [("api", "apps"), ("web", "services")] {
        assert!(
            repvix()
                .current_dir(directory.path())
                .args([
                    "add",
                    &format!("git@github.com:acme/{name}.git"),
                    "--path",
                    parent,
                ])
                .output()
                .unwrap()
                .status
                .success()
        );
        let repository = directory.path().join("repos").join(parent).join(name);
        fs::create_dir_all(&repository).unwrap();
        fs::write(repository.join("keep-me"), "still here\n").unwrap();
    }

    let rejected = repvix()
        .current_dir(directory.path())
        .args(["remove", "api", "missing"])
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(
        String::from_utf8_lossy(&rejected.stderr).contains("unknown repository: missing"),
        "{}",
        String::from_utf8_lossy(&rejected.stderr)
    );
    let yaml = fs::read_to_string(directory.path().join("repvix.yaml")).unwrap();
    assert!(yaml.contains("name: api"), "{yaml}");
    assert!(yaml.contains("name: web"), "{yaml}");

    let removed = repvix()
        .current_dir(directory.path())
        .args(["remove", "api"])
        .output()
        .unwrap();
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    let stdout = String::from_utf8(removed.stdout).unwrap();
    assert!(stdout.contains("removed  api"), "{stdout}");
    assert!(stdout.contains("working directory was kept"), "{stdout}");
    assert!(
        directory.path().join("repos/apps/api/keep-me").is_file(),
        "repository contents were deleted"
    );
    let yaml = fs::read_to_string(directory.path().join("repvix.yaml")).unwrap();
    assert!(!yaml.contains("name: api"), "{yaml}");
    assert!(yaml.contains("name: web"), "{yaml}");

    let removed = repvix()
        .current_dir(directory.path())
        .args(["remove", "--regex", "^services/"])
        .output()
        .unwrap();
    assert!(
        removed.status.success(),
        "{}",
        String::from_utf8_lossy(&removed.stderr)
    );
    assert!(
        directory
            .path()
            .join("repos/services/web/keep-me")
            .is_file(),
        "repository contents were deleted"
    );
    let yaml = fs::read_to_string(directory.path().join("repvix.yaml")).unwrap();
    assert!(!yaml.contains("name: web"), "{yaml}");

    let api = directory.path().join("repos/apps/api");
    assert!(
        Command::new("git")
            .current_dir(&api)
            .args(["init", "--quiet"])
            .status()
            .unwrap()
            .success()
    );
    let sync = repvix()
        .current_dir(directory.path())
        .arg("sync")
        .output()
        .unwrap();
    assert!(
        sync.status.success(),
        "{}",
        String::from_utf8_lossy(&sync.stderr)
    );
    let stdout = String::from_utf8(sync.stdout).unwrap();
    assert!(
        stdout.contains("1 Git repository is not configured and will be skipped by sync"),
        "{stdout}"
    );
    assert!(stdout.contains("repos/apps/api"), "{stdout}");
    assert!(api.join("keep-me").is_file());
}

#[test]
fn stores_commands_as_program_and_arguments() {
    let directory = tempdir().unwrap();
    assert!(
        repvix()
            .current_dir(directory.path())
            .arg("init")
            .status()
            .unwrap()
            .success()
    );

    let add = repvix()
        .current_dir(directory.path())
        .args([
            "command",
            "add",
            "test",
            "--description",
            "Run tests",
            "--",
            "cargo",
            "test",
            "--all",
        ])
        .output()
        .unwrap();
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );

    let yaml = fs::read_to_string(directory.path().join("repvix.yaml")).unwrap();
    assert!(yaml.contains("program: cargo"), "{yaml}");
    assert!(yaml.contains("- --all"), "{yaml}");
}

#[test]
fn generates_standalone_bash_completion_outside_a_workspace() {
    let directory = tempdir().unwrap();
    let completion = repvix()
        .current_dir(directory.path())
        .args(["completions", "bash"])
        .output()
        .unwrap();
    assert!(
        completion.status.success(),
        "{}",
        String::from_utf8_lossy(&completion.stderr)
    );
    let script = String::from_utf8(completion.stdout).unwrap();
    assert!(script.starts_with("# bash completion for repvix"));
    assert!(script.contains("init add import remove clone"));
    assert!(script.contains("--url --regex --help"));
    assert!(!script.contains("--from-base-path"));
    assert!(script.contains("complete -F _repvix repvix"));
}

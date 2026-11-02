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
    assert!(stdout.contains("sources/services/api"), "{stdout}");
    assert!(!stdout.contains("sources/services\t"), "{stdout}");

    let yaml = fs::read_to_string(directory.path().join("repvix.yaml")).unwrap();
    assert!(yaml.contains("version: 2"), "{yaml}");
    assert!(yaml.contains("base_path: sources"), "{yaml}");
    assert!(yaml.contains("name: api"), "{yaml}");
    assert!(yaml.contains("path: services"), "{yaml}");
    assert!(!yaml.contains("tags:"), "{yaml}");
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
    assert!(script.contains("init add import clone"));
    assert!(script.contains("complete -F _repvix repvix"));
}

use repvix::config::Config;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::tempdir;

fn successful(output: Output) -> String {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn repvix(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_repvix"))
        .current_dir(root)
        .args(args)
        .output()
        .unwrap()
}

fn git(root: &Path, args: &[&str]) -> String {
    successful(
        Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .unwrap(),
    )
    .trim()
    .to_owned()
}

fn commit(repository: &Path) {
    git(repository, &["add", "tracked.txt"]);
    git(
        repository,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "--quiet",
            "--no-gpg-sign",
            "-m",
            "Update tracked file",
        ],
    );
}

fn create_repository(parent: &Path, name: &str, branch: &str) -> PathBuf {
    let repository = parent.join(name);
    fs::create_dir_all(&repository).unwrap();
    git(
        &repository,
        &["init", "--quiet", "--initial-branch", "main"],
    );
    fs::write(repository.join("tracked.txt"), "main\n").unwrap();
    commit(&repository);
    git(&repository, &["checkout", "-b", branch]);
    fs::write(repository.join("tracked.txt"), format!("{branch}\n")).unwrap();
    commit(&repository);
    git(&repository, &["checkout", "main"]);
    repository
}

#[test]
fn clone_prepares_each_branch_and_keeps_unspecified_repositories_unchanged() {
    let directory = tempdir().unwrap();
    let root = directory.path();
    successful(repvix(root, &["init"]));
    for (name, branch, flag) in [
        ("api", "release/next", Some("--branch")),
        ("web", "develop", Some("-b")),
        ("worker", "feature/local", None),
    ] {
        let remote = create_repository(&root.join("remotes"), name, branch);
        let mut args = vec!["add", remote.to_str().unwrap()];
        if let Some(flag) = flag {
            args.extend([flag, branch]);
        }
        successful(repvix(root, &args));
    }
    let config = Config::load(root).unwrap();
    assert_eq!(config.repos[0].branch.as_deref(), Some("release/next"));
    assert_eq!(config.repos[1].branch.as_deref(), Some("develop"));
    assert_eq!(config.repos[2].branch, None);

    successful(repvix(root, &["clone"]));
    for (name, branch) in [
        ("api", "release/next"),
        ("web", "develop"),
        ("worker", "main"),
    ] {
        let repository = root.join("repos").join(name);
        assert_eq!(git(&repository, &["branch", "--show-current"]), branch);
        assert_eq!(
            fs::read_to_string(repository.join("tracked.txt")).unwrap(),
            format!("{branch}\n")
        );
        assert_eq!(
            git(&repository, &["rev-parse", "--abbrev-ref", "@{upstream}"]),
            format!("origin/{branch}")
        );
    }
    let list = successful(repvix(root, &["list"]));
    assert!(list.lines().next().unwrap().contains("BRANCH"), "{list}");
    assert!(list.contains("release/next"), "{list}");
    assert!(list.contains("develop"), "{list}");

    let worker = root.join("repos/worker");
    git(&worker, &["checkout", "feature/local"]);
    fs::write(worker.join("tracked.txt"), "local edits\n").unwrap();
    successful(repvix(root, &["clone"]));
    assert_eq!(git(&worker, &["branch", "--show-current"]), "feature/local");
    assert_eq!(
        fs::read_to_string(worker.join("tracked.txt")).unwrap(),
        "local edits\n"
    );
}

#[test]
fn clone_switches_existing_branches_without_overwriting_conflicting_changes() {
    let directory = tempdir().unwrap();
    let root = directory.path();
    let api_remote = create_repository(&root.join("remotes"), "api", "release/next");
    successful(repvix(root, &["init"]));
    successful(repvix(root, &["add", api_remote.to_str().unwrap()]));
    successful(repvix(root, &["clone"]));

    let mut config = Config::load(root).unwrap();
    config.repos[0].branch = Some("release/next".into());
    config.save(root).unwrap();
    let api = root.join("repos/api");
    fs::write(api.join("tracked.txt"), "local edits\n").unwrap();

    let web_remote = create_repository(&root.join("remotes"), "web", "develop");
    successful(repvix(
        root,
        &["add", web_remote.to_str().unwrap(), "--branch", "develop"],
    ));
    let failed = repvix(root, &["clone"]);
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("clone failed for 1 repository"));
    assert_eq!(git(&api, &["branch", "--show-current"]), "main");
    assert_eq!(
        fs::read_to_string(api.join("tracked.txt")).unwrap(),
        "local edits\n"
    );
    assert_eq!(
        git(&root.join("repos/web"), &["branch", "--show-current"]),
        "develop"
    );

    fs::write(api.join("tracked.txt"), "main\n").unwrap();
    fs::write(api.join("notes.txt"), "keep this untracked file\n").unwrap();
    successful(repvix(root, &["clone"]));
    assert_eq!(git(&api, &["branch", "--show-current"]), "release/next");
    assert_eq!(
        git(&api, &["rev-parse", "--abbrev-ref", "@{upstream}"]),
        "origin/release/next"
    );
    assert_eq!(
        fs::read_to_string(api.join("notes.txt")).unwrap(),
        "keep this untracked file\n"
    );
}

#[test]
fn clone_reports_missing_branches_and_continues_with_other_repositories() {
    let directory = tempdir().unwrap();
    let root = directory.path();
    successful(repvix(root, &["init"]));
    for (name, branch) in [("api", "missing"), ("web", "develop")] {
        let remote = create_repository(&root.join("remotes"), name, "develop");
        successful(repvix(
            root,
            &["add", remote.to_str().unwrap(), "--branch", branch],
        ));
    }

    let failed = repvix(root, &["clone"]);
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("clone failed for 1 repository"));
    assert!(String::from_utf8_lossy(&failed.stdout).contains("missing"));
    assert!(!root.join("repos/api/.git").exists());
    assert_eq!(
        git(&root.join("repos/web"), &["branch", "--show-current"]),
        "develop"
    );
}

#[test]
fn import_records_current_branches_and_allows_explicit_overrides() {
    let directory = tempdir().unwrap();
    let root = directory.path();
    successful(repvix(root, &["init"]));
    for (name, branch) in [("api", "release/next"), ("web", "develop")] {
        let repository = create_repository(&root.join("repos"), name, branch);
        git(
            &repository,
            &[
                "remote",
                "add",
                "origin",
                &format!("https://example.com/{name}.git"),
            ],
        );
        git(&repository, &["checkout", branch]);
    }
    successful(repvix(root, &["import", "repos/api"]));
    assert_eq!(
        Config::load(root).unwrap().repos[0].branch.as_deref(),
        Some("release/next")
    );

    successful(repvix(root, &["import", "--regex", ".*"]));
    let config = Config::load(root).unwrap();
    assert_eq!(config.repos[0].branch.as_deref(), Some("release/next"));
    assert_eq!(config.repos[1].branch.as_deref(), Some("develop"));

    let api = root.join("repos/api");
    git(&api, &["checkout", "main"]);
    successful(repvix(root, &["import", "repos/api"]));
    assert_eq!(
        Config::load(root).unwrap().repos[0].branch.as_deref(),
        Some("main")
    );

    successful(repvix(
        root,
        &[
            "import",
            "repos/api",
            "repos/web",
            "--branch",
            "feature/shared",
        ],
    ));
    let config = Config::load(root).unwrap();
    assert!(
        config
            .repos
            .iter()
            .all(|repo| repo.branch.as_deref() == Some("feature/shared"))
    );
    assert_eq!(git(&api, &["branch", "--show-current"]), "main");
    assert_eq!(
        git(&root.join("repos/web"), &["branch", "--show-current"]),
        "develop"
    );

    successful(repvix(root, &["import", "--regex", ".*", "-b", "main"]));
    assert!(
        Config::load(root)
            .unwrap()
            .repos
            .iter()
            .all(|repo| repo.branch.as_deref() == Some("main"))
    );
}

#[test]
fn detached_import_requires_a_branch_without_changing_head_or_configuration() {
    let directory = tempdir().unwrap();
    let root = directory.path();
    successful(repvix(root, &["init"]));
    let api = create_repository(&root.join("repos"), "api", "release/next");
    git(
        &api,
        &["remote", "add", "origin", "https://example.com/api.git"],
    );
    git(&api, &["checkout", "--detach"]);
    let before = fs::read(root.join("repvix.yaml")).unwrap();

    let failed = repvix(root, &["import", "repos/api"]);
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("detached HEAD"));
    assert_eq!(fs::read(root.join("repvix.yaml")).unwrap(), before);

    successful(repvix(root, &["import", "repos/api", "-b", "release/next"]));
    assert_eq!(
        Config::load(root).unwrap().repos[0].branch.as_deref(),
        Some("release/next")
    );
    assert_eq!(git(&api, &["branch", "--show-current"]), "");
}

#[test]
fn invalid_branches_do_not_change_configuration() {
    let directory = tempdir().unwrap();
    let root = directory.path();
    successful(repvix(root, &["init"]));
    let before = fs::read(root.join("repvix.yaml")).unwrap();
    for branch in [
        "", " ", "-bad", "HEAD", "bad name", "a..b", "a@{b}", "/main", "main/", "a//b", "a.lock",
        ".hidden", "a.", "a:b",
    ] {
        let failed = repvix(
            root,
            &[
                "add",
                "https://example.com/api.git",
                &format!("--branch={branch}"),
            ],
        );
        assert!(!failed.status.success(), "accepted {branch:?}");
        assert!(String::from_utf8_lossy(&failed.stderr).contains("invalid repository branch"));
        assert_eq!(fs::read(root.join("repvix.yaml")).unwrap(), before);
    }
}

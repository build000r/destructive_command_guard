//! Cheap Cargo invalidation proof: the fixture includes the actual build helper,
//! has no dependencies, and changes only Git history after its source is frozen.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

const FILES: [&str; 6] = [
    "Cargo.toml",
    "Cargo.lock",
    "build.rs",
    "build_git_inputs.rs",
    "src/main.rs",
    ".gitignore",
];

struct Fixture {
    root: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("dcg-git-inputs-{}-{nonce}", std::process::id()));
        let home = root.join("home");
        fs::create_dir_all(&home).unwrap();
        Self { root, home }
    }

    fn command(&self, program: impl AsRef<OsStr>, cwd: &Path) -> Command {
        let mut command = Command::new(program);
        command.current_dir(cwd);
        let original_home = std::env::var_os("HOME").map(PathBuf::from);
        for (name, _) in std::env::vars_os() {
            let name_text = name.to_string_lossy();
            if name_text.starts_with("GIT_")
                || name_text.starts_with("DSR_")
                || name_text.starts_with("DCG_")
            {
                command.env_remove(name);
            }
        }
        for (variable, directory) in [("CARGO_HOME", ".cargo"), ("RUSTUP_HOME", ".rustup")] {
            if std::env::var_os(variable).is_none() {
                if let Some(home) = &original_home {
                    command.env(variable, home.join(directory));
                }
            }
        }
        command
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(
                "GIT_CONFIG_GLOBAL",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
            )
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_AUTHOR_NAME", "DCG fixture")
            .env("GIT_COMMITTER_NAME", "DCG fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
            .env("CARGO_NET_OFFLINE", "true")
            .env("CARGO_INCREMENTAL", "0")
            .env_remove("RUSTC_WRAPPER")
            .env_remove("RUSTC_WORKSPACE_WRAPPER");
        command
    }

    fn checked(command: &mut Command) -> String {
        let output: Output = command.output().expect("fixture process starts");
        assert!(
            output.status.success(),
            "{command:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("UTF-8 fixture output")
            .trim()
            .to_owned()
    }

    fn git(&self, repo: &Path, args: &[&str]) -> String {
        Self::checked(
            self.command("git", repo)
                .args([
                    "-c",
                    "core.hooksPath=",
                    "-c",
                    "commit.gpgSign=false",
                    "-c",
                    "tag.gpgSign=false",
                ])
                .args(args),
        )
    }

    fn write_crate(&self, repo: &Path) {
        fs::create_dir_all(repo.join("src")).unwrap();
        fs::write(repo.join("Cargo.toml"), "[package]\nname = \"provenance-probe\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[workspace]\n").unwrap();
        fs::write(
            repo.join("Cargo.lock"),
            "version = 4\n\n[[package]]\nname = \"provenance-probe\"\nversion = \"0.0.0\"\n",
        )
        .unwrap();
        fs::write(repo.join(".gitignore"), "target/\n").unwrap();
        fs::write(repo.join("src/main.rs"), "fn main() { println!(\"{}|{}|{}\", env!(\"PROBE_SHA\"), env!(\"PROBE_DESCRIBE\"), env!(\"PROBE_NONCE\")); }\n").unwrap();
        let helper = Path::new(env!("CARGO_MANIFEST_DIR")).join("build_git_inputs.rs");
        fs::copy(helper, repo.join("build_git_inputs.rs")).unwrap();
        let build = r#"mod build_git_inputs;

fn main() {
    let root = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let package_has_git = build_git_inputs::emit_git_rerun_inputs(&root).unwrap();
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=DSR_RELEASE_GIT_SHA");
    println!("cargo:rerun-if-env-changed=DSR_RELEASE_GIT_REF");
    let read_git = |args: &[&str]| {
        let output = std::process::Command::new("git").current_dir(&root).args(args).output().unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    let (sha, describe) = if let (Ok(sha), Ok(tag)) = (
        std::env::var("DSR_RELEASE_GIT_SHA"), std::env::var("DSR_RELEASE_GIT_REF"),
    ) {
        // This fixture receives the real source repository's SHA/tag below.
        // DSR's existing validation is unchanged; this test exercises inputs.
        (sha, tag)
    } else if package_has_git {
        (read_git(&["rev-parse", "HEAD"]), read_git(&["describe", "--tags", "--always"]))
    } else {
        ("unknown".to_owned(), "unknown".to_owned())
    };
    println!("cargo:rustc-env=PROBE_SHA={sha}");
    println!("cargo:rustc-env=PROBE_DESCRIBE={describe}");
    println!("cargo:rustc-env=PROBE_NONCE={}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos());
}
"#;
        fs::write(repo.join("build.rs"), build).unwrap();
    }

    fn build(&self, repo: &Path, dsr: Option<(&str, &str)>) -> Vec<String> {
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| env!("CARGO").into());
        let mut command = self.command(cargo, repo);
        command
            .args(["run", "--quiet", "--offline", "--locked", "--jobs", "1"])
            .arg("--target-dir")
            .arg(repo.join("target"));
        if let Some((sha, tag)) = dsr {
            command
                .env("DSR_RELEASE_GIT_SHA", sha)
                .env("DSR_RELEASE_GIT_REF", tag);
        }
        Self::checked(&mut command)
            .split('|')
            .map(str::to_owned)
            .collect()
    }

    fn commit(&self, repo: &Path, label: &str) -> String {
        fs::write(repo.join("history-marker"), label).unwrap();
        self.git(repo, &["add", "history-marker"]);
        self.git(repo, &["commit", "-m", label]);
        self.git(repo, &["rev-parse", "HEAD"])
    }

    fn assert_advanced(&self, repo: &Path, previous: &[String], expected: &str) -> Vec<String> {
        let next = self.build(repo, None);
        assert_eq!(
            next[0], expected,
            "Cargo must embed the actual new Git identity"
        );
        assert_ne!(
            next[2], previous[2],
            "Git-only change must rerun the build script"
        );
        assert_eq!(
            self.build(repo, None),
            next,
            "an unchanged warm build must stay cached"
        );
        next
    }
}

#[test]
fn cargo_tracks_real_git_transitions_without_source_edits() {
    let fixture = Fixture::new();
    let repo = fixture.root.join("repo");
    fixture.write_crate(&repo);
    fixture.git(&repo, &["init", "--initial-branch=main"]);
    fixture.git(&repo, &["add", "."]);
    fixture.git(&repo, &["commit", "-m", "fixture source"]);
    let sources: Vec<_> = FILES
        .iter()
        .map(|name| fs::read(repo.join(name)).unwrap())
        .collect();
    let first = fixture.build(&repo, None);
    assert_eq!(first[0], fixture.git(&repo, &["rev-parse", "HEAD"]));
    assert_eq!(fixture.build(&repo, None), first);
    let sha = fixture.commit(&repo, "normal branch advances");
    let normal = fixture.assert_advanced(&repo, &first, &sha);

    fixture.git(&repo, &["pack-refs", "--all"]);
    let packed = fixture.build(&repo, None);
    assert_eq!(packed[0], normal[0]);
    assert_eq!(fixture.build(&repo, None), packed);
    let sha = fixture.commit(&repo, "packed branch gains loose ref");
    fixture.assert_advanced(&repo, &packed, &sha);

    let linked = fixture.root.join("linked");
    fixture.git(
        &repo,
        &["worktree", "add", "-b", "linked", linked.to_str().unwrap()],
    );
    let first_linked = fixture.build(&linked, None);
    let sha = fixture.commit(&linked, "linked branch advances");
    let linked_next = fixture.assert_advanced(&linked, &first_linked, &sha);
    fixture.git(&repo, &["pack-refs", "--all"]);
    let linked_packed = fixture.build(&linked, None);
    assert_eq!(linked_packed[0], linked_next[0]);
    let sha = fixture.commit(&linked, "packed linked branch advances");
    fixture.assert_advanced(&linked, &linked_packed, &sha);

    fixture.git(&linked, &["checkout", "--detach"]);
    let detached = fixture.build(&linked, None);
    let sha = fixture.commit(&linked, "detached HEAD advances");
    let detached_next = fixture.assert_advanced(&linked, &detached, &sha);
    fixture.git(&linked, &["tag", "v0.0.0"]);
    let tagged = fixture.build(&linked, None);
    assert_eq!(tagged[0], detached_next[0]);
    assert_eq!(tagged[1], "v0.0.0");
    assert_ne!(tagged[2], detached_next[2]);
    for (name, bytes) in FILES.iter().zip(sources) {
        assert_eq!(fs::read(repo.join(name)).unwrap(), bytes);
        assert_eq!(fs::read(linked.join(name)).unwrap(), bytes);
    }

    // An archive inside an unrelated repository must not discover that repo.
    let snapshot = repo.join("snapshot");
    fs::create_dir_all(snapshot.join("src")).unwrap();
    for name in FILES.into_iter().chain(["history-marker"]) {
        fs::copy(linked.join(name), snapshot.join(name)).unwrap();
    }
    let archive = fixture.build(&snapshot, None);
    assert_eq!(archive[0], "unknown");
    assert_eq!(archive[1], "unknown");
    assert_eq!(fixture.build(&snapshot, None), archive);
    // These are the actual committed snapshot's source identity and tag, not
    // invented release metadata. The helper must leave strict DSR inputs alone.
    let dsr = fixture.build(&snapshot, Some((&sha, "v0.0.0")));
    assert_eq!(dsr[0], sha);
    assert_eq!(dsr[1], "v0.0.0");
    assert_eq!(fixture.build(&snapshot, Some((&sha, "v0.0.0"))), dsr);
    eprintln!(
        "retained Git/Cargo provenance fixture: {}",
        fixture.root.display()
    );
}

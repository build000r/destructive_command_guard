//! Git inputs that vergen-gix 10.0.3 misses for linked worktrees and packed refs.
//! Only Cargo invalidation is emitted here; vergen/DSR still own the identity.

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn git(root: &Path, args: &[&str]) -> io::Result<Output> {
    let mut command = Command::new("git");
    command.current_dir(root).args(args).stdin(Stdio::null());
    // Plumbing must describe this package, never an ambient Git context. None
    // of these read-only commands invokes hooks or needs optional index writes.
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_") {
            command.env_remove(name);
        }
    }
    command
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .output()
}

fn text(output: Output) -> io::Result<String> {
    if !output.status.success() {
        return Err(io::Error::other(
            "Git could not resolve the package's provenance inputs",
        ));
    }
    let value = String::from_utf8(output.stdout).map_err(io::Error::other)?;
    let value = value.trim_end_matches(['\r', '\n']);
    if value.is_empty() || value.contains(['\r', '\n']) {
        return Err(io::Error::other("Git returned an invalid Cargo input path"));
    }
    Ok(value.to_owned())
}

fn git_path(root: &Path, name: &str) -> io::Result<PathBuf> {
    text(git(
        root,
        &["rev-parse", "--path-format=absolute", "--git-path", name],
    )?)
    .map(PathBuf::from)
}

fn existing_parent(mut path: PathBuf, refs: &Path) -> io::Result<PathBuf> {
    // Watching a missing file makes Cargo rebuild forever. A packed branch
    // needs its existing parent watched until Git creates its loose ref.
    while !path.try_exists()? {
        if path == refs || !path.starts_with(refs) || !path.pop() {
            return Err(io::Error::other("Git refs directory is unavailable"));
        }
    }
    Ok(path)
}

/// Register provenance inputs and report whether this package owns Git metadata.
/// A false result must also suppress vergen's ancestor-repository discovery;
/// strict DSR provenance remains the caller's independent explicit input.
///
/// # Errors
/// Returns an error when a package has Git metadata but its input paths cannot
/// be resolved safely. Source archives with no package-root `.git` need no Git.
pub fn emit_git_rerun_inputs(root: &Path) -> io::Result<bool> {
    match root.join(".git").symlink_metadata() {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    }
    // No discovery above a source archive or strict DSR snapshot. Git alone
    // resolves linked-worktree common-dir paths and relative gitdir files.
    let mut paths = BTreeSet::from([git_path(root, "HEAD")?]);
    let refs = git_path(root, "refs")?;
    let symbolic = git(root, &["symbolic-ref", "--quiet", "HEAD"])?;
    if symbolic.status.success() {
        let reference = text(symbolic)?;
        paths.insert(existing_parent(git_path(root, &reference)?, &refs)?);
    } else if symbolic.status.code() != Some(1) {
        return Err(io::Error::other("Git could not resolve the package's HEAD"));
    }
    let packed = git_path(root, "packed-refs")?;
    if packed.try_exists()? {
        paths.insert(packed);
    }
    // Describe also changes when tags change without moving HEAD. Packing
    // loose tags changes this directory and makes packed-refs a watched input.
    paths.insert(existing_parent(git_path(root, "refs/tags")?, &refs)?);
    for path in paths {
        let value = path
            .to_str()
            .ok_or_else(|| io::Error::other("non-UTF-8 Git input path"))?;
        println!("cargo:rerun-if-changed={value}");
    }
    Ok(true)
}

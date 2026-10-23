use std::process::Command;

fn dcg_binary() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_dcg"))
}

fn run_hook(command: &str) -> String {
    let input = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": {
            "command": command,
        }
    });

    let mut child = Command::new(dcg_binary())
        // Hook-mode self-heal writes the invoked binary's own path into the
        // caller's real agent settings, so without this a test run registers
        // `target/release/dcg` as a global Claude Code PreToolUse hook on the
        // developer's machine.
        .env("DCG_SELF_HEAL_HOOK", "0")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn dcg");

    {
        let stdin = child.stdin.as_mut().expect("failed to open stdin");
        serde_json::to_writer(stdin, &input).expect("failed to write json");
    }

    let output = child.wait_with_output().expect("failed to wait for dcg");
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn strict_git_hook_decision(command: &str) -> String {
    let temp = tempfile::tempdir().expect("temporary hook home");
    let input = serde_json::json!({
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": {"command": command},
    });
    let mut child = Command::new(dcg_binary())
        .env_clear()
        .env("HOME", temp.path())
        .env("USERPROFILE", temp.path())
        .env("DCG_ALLOWLIST_SYSTEM_PATH", "")
        .env("DCG_NO_SELF_HEAL", "1")
        .env("DCG_HOOK_TIMEOUT_MS", "5000")
        .env("DCG_PACKS", "core.git,strict_git")
        .current_dir(temp.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn isolated strict Git hook");
    serde_json::to_writer(child.stdin.take().expect("hook stdin"), &input)
        .expect("write hook envelope");
    let output = child.wait_with_output().expect("wait for hook");
    assert!(
        output.status.success(),
        "hook failed for {command}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    if output.stdout.is_empty() {
        return "allow".to_string();
    }
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("valid hook response");
    value["hookSpecificOutput"]["permissionDecision"]
        .as_str()
        .expect("hook decision")
        .to_string()
}

#[test]
fn strict_git_log_cherry_pick_option_survives_native_hook() {
    for command in [
        "git log --oneline --left-right --cherry-pick main...11a317bf",
        "git -C /repo --no-pager log --cherry-pick main...topic",
        "git -c user.name=reviewer --git-dir=.git log --cherry-pick main...topic",
        "git diff --stat && git log --cherry-pick main...topic && git show --stat HEAD",
        "git diff --stat; git log --cherry-pick main...topic; git show --stat HEAD",
    ] {
        assert_eq!(strict_git_hook_decision(command), "allow", "{command}");
    }
}

#[test]
fn strict_git_log_refinement_cannot_hide_native_hook_denials() {
    for command in [
        "git cherry-pick abc123",
        "git -C /repo cherry-pick abc123",
        "git log --cherry-pick main...topic; git cherry-pick abc123",
        "git cherry-pick abc123 && git log --cherry-pick main...topic",
        "git log --cherry-pick main...topic && git reset --hard",
        "git -c alias.history=log history --cherry-pick main...topic",
        "git $OPTIONS log --cherry-pick main...topic",
        "git log --cherry-pick \"$(git cherry-pick abc123)\"",
        "sh -c 'git cherry-pick abc123; git log --cherry-pick main...topic'",
        "git log --cherry-pick main...topic; git cherry\\-pick abc123",
        "git log --cherry-pick main...topic; git 'cherry-'pick abc123",
        "git -c core.pager='git cherry-pick abc123' log --cherry-pick main...topic",
    ] {
        assert_eq!(strict_git_hook_decision(command), "deny", "{command}");
    }
}

#[test]
fn python_git_readonly_argv_keeps_dynamic_argument_positions() {
    for source in [
        "subprocess.run(['git','-C',r,'rev-parse','--verify',ref+'^{commit}'], capture_output=True, text=True)",
        "subprocess.run(['git','-C',r,'merge-base','--is-ancestor',oid,'HEAD'])",
        "subprocess.check_output(['git','-C',str(keeper),'ls-files','*.py'])",
        "subprocess.run(['git','-C',str(repo),'worktree','list','--porcelain'])",
        "subprocess.run(['git','-C',r,'rev-parse','HEAD'], shell=False)",
        "subprocess.run(['/usr/bin/git','-C',r,'rev-parse','HEAD'])",
        "subprocess.check_output(['/opt/homebrew/bin/git','-C',str(repo),'ls-files'])",
    ] {
        for receiver in ["python3 -", "ssh build-host 'python3 -'"] {
            let command = format!("{receiver} <<'PY'\nimport subprocess\n{source}\nPY");
            assert_eq!(strict_git_hook_decision(&command), "allow", "{command}");
        }
    }
}

#[test]
fn python_git_argv_positions_do_not_hide_destructive_dispatch() {
    for source in [
        "subprocess.run(['git','-C',r,'branch','-D',branch])",
        "subprocess.run(['git','-C',r,'branch',flag,branch])",
        "subprocess.run(['git','-C',r,'reset','--hard'])",
        "subprocess.run(['git','-C',r,verb,'--hard'])",
        "subprocess.run(['git','-C',r,'unknown-alias'])",
        "subprocess.run(['git','-c',alias,'-C',r,'status'])",
        "subprocess.run(['git','--exec-path='+helper,'-C',r,'status'])",
        "subprocess.run(['git','--config-env',config,'-C',r,'status'])",
        "subprocess.run(['git','-C',r,'rev-parse',ref+'^{commit}'], shell=True)",
        "subprocess.run(['git','-C',r,'rev-parse',ref+'^{commit}'], executable=program)",
        "subprocess.run(['git','-C',r,*args,'reset','--hard'])",
        "subprocess.run([['git','-C',r,'rev-parse'][0],'reset','--hard'])",
        "subprocess.run(['git','-C',r,'rev-parse',ref+'^{commit}'], **options)",
        "run(['git','-C',r,'rev-parse',ref+'^{commit}'])",
        "subprocess.run(['/usr/bin/git','-C',r,'branch',flag,branch])",
        "subprocess.run(['/opt/homebrew/bin/git','-C',r,verb,'--hard'])",
    ] {
        for receiver in ["python3 -", "ssh build-host 'python3 -'"] {
            let command = format!("{receiver} <<'PY'\nimport subprocess\n{source}\nPY");
            assert_eq!(strict_git_hook_decision(&command), "deny", "{command}");
        }
    }
}

#[test]
fn python_git_argv_bounds_do_not_restore_lossy_reconstruction() {
    let oversized = format!(
        "subprocess.run(['git','-C',r,'branch',flag,'{}'])",
        "x".repeat(16_384)
    );
    let too_many_nodes = format!(
        "subprocess.run(['git','-C',r,'branch',flag,{}'last'])",
        "'x',".repeat(600)
    );
    for source in [oversized, too_many_nodes] {
        let command = format!("python3 - <<'PY'\nimport subprocess\n{source}\nPY");
        assert_eq!(strict_git_hook_decision(&command), "deny", "{command}");
    }
}

#[test]
fn git_global_flag_bypass() {
    // This command is destructive but currently bypassed because regex expects "git reset" directly
    let cmd = "git -C /tmp reset --hard";
    let output = run_hook(cmd);

    assert!(
        !output.trim().is_empty(),
        "Bypass confirmed: '{cmd}' was allowed!"
    );

    assert!(output.contains("deny"), "Should be denied");
}

#[test]
fn git_work_tree_bypass() {
    let cmd = "git --work-tree=/tmp clean -fd";
    let output = run_hook(cmd);

    assert!(
        !output.trim().is_empty(),
        "Bypass confirmed: '{cmd}' was allowed!"
    );

    assert!(output.contains("deny"), "Should be denied");
}

/// #429: a `core.git` safe pattern shadows every destructive pattern in the
/// pack, and the executable prefix used to skip any token between `git` and the
/// subcommand. Appending a dry-run-shaped tail therefore turned a destructive
/// command into an allow, while real git accepted the extra tokens as pathspecs
/// and removed every untracked and ignored file under `.` (verified against git
/// 2.55.0). The tail has to reach the safe pattern past a bare `--`, which is
/// exactly where git stops parsing options.
#[test]
fn a_dry_run_shaped_tail_cannot_rescue_git_clean() {
    for cmd in [
        "git clean -fdx -- . clean -n",
        "git clean -xfd -- . clean --dry-run",
        "git clean -fdx -- . lfs prune --dry-run",
        "git clean -fdx -- . restore --staged x",
        "git clean -fdx -- . checkout -b x",
        "git clean -fdx --quiet -- . clean -n",
    ] {
        let output = run_hook(cmd);
        assert!(
            output.contains("deny"),
            "safe-pattern shadowing rescued a destructive clean: {cmd} -> {output}"
        );
    }
}

/// #431: safe matching runs on a view with the quotes removed, so a *quoted*
/// separator arrives as a bare one and satisfies the dashed branch's
/// command-position guard. The four characters below are exactly that guard's
/// set; each one manufactured a command boundary that the shell never had, and
/// git deleted the tree.
#[test]
fn a_quoted_separator_cannot_manufacture_command_position() {
    for separator in ["(", "&", "|", ";"] {
        let cmd = format!("git clean -fdx -- . '{separator}' git-clean -n");
        let output = run_hook(&cmd);
        assert!(
            output.contains("deny"),
            "a quoted {separator} rescued a destructive clean: {cmd} -> {output}"
        );
    }
}

/// The exemptions the two fixes above must not take away. Git's `parse_options`
/// permutes, so a dry-run flag after a pathspec is still a dry run, and a global
/// option before the subcommand is still the same subcommand.
#[test]
fn real_dry_runs_and_staged_restores_stay_allowed() {
    for cmd in [
        "git clean -n",
        "git clean --dry-run",
        "git clean -fdxn",
        "git clean -fdx -n",
        "git clean . -n",
        "git clean -fdx src -n",
        "git -C /tmp/repo clean -n",
        "git -c core.pager=cat clean --dry-run",
        "git --no-pager clean -n",
        "git --git-dir=/tmp/repo/.git clean -n",
        "git --git-dir /tmp/repo/.git clean -n",
        "git restore --staged file.txt",
        "git restore . --staged",
        "git checkout -b feature/x",
        "git checkout --orphan gh-pages",
        "git lfs prune --dry-run",
        "git-clean -n",
        "/usr/libexec/git-core/git-clean -n",
    ] {
        let output = run_hook(cmd);
        assert!(
            !output.contains("deny"),
            "a non-destructive git invocation was denied: {cmd} -> {output}"
        );
    }
}

use super::policy::{NetworkPolicy, SandboxPolicy};

/// Apply macOS sandbox enforcement using Seatbelt SBPL profiles.
///
/// Instead of applying sandbox restrictions in the current process,
/// we modify the exec target to use `sandbox-exec -p <profile> bash -c <command>`.
/// This is called from the child process before exec.
pub fn apply_sandbox(policy: &SandboxPolicy) -> Result<(), String> {
    let profile = generate_sbpl_profile(policy);

    // Store profile in env for the child's exec_bash to pick up.
    // On macOS, the child execs sandbox-exec instead of bash directly.
    // SAFETY: called before spawning the sandboxed child process
    unsafe { std::env::set_var("_LOCALGPT_SBPL_PROFILE", &profile) };

    Ok(())
}

/// Execute a command under sandbox-exec with the given SBPL profile.
/// This replaces the standard exec_bash on macOS when sandbox is active.
pub fn exec_sandboxed(command: &str) -> ! {
    use std::os::unix::process::CommandExt;

    let profile = std::env::var("_LOCALGPT_SBPL_PROFILE").unwrap_or_default();

    if profile.is_empty() {
        let err = std::process::Command::new("/bin/bash")
            .arg("-c")
            .arg(command)
            .exec();
        eprintln!("localgpt-sandbox: failed to exec bash: {}", err);
        std::process::exit(1);
    }

    let err = std::process::Command::new("/usr/bin/sandbox-exec")
        .arg("-p")
        .arg(&profile)
        .arg("/bin/bash")
        .arg("-c")
        .arg(command)
        .exec();

    eprintln!("localgpt-sandbox: failed to exec sandbox-exec: {}", err);
    std::process::exit(1);
}

/// Generate a Seatbelt SBPL profile string from a SandboxPolicy.
///
/// Strategy: allow broad file reads (macOS bash needs access to dyld cache,
/// system frameworks, and paths that are impractical to enumerate), then deny
/// the user's home directory to prevent reading repos, personal files, etc.
/// Re-allow workspace and configured paths within home. Restrict writes to
/// workspace + /tmp. Deny network. SBPL evaluates rules in order — last match
/// wins for deny/allow conflicts.
pub fn generate_sbpl_profile(policy: &SandboxPolicy) -> String {
    let mut rules = vec![
        "(version 1)".to_string(),
        "(deny default)".to_string(),
        // Process operations — bash needs these
        "(allow process*)".to_string(),
        "(allow signal)".to_string(),
        // Mach IPC — required for dyld, libSystem, and most macOS subsystems
        "(allow mach*)".to_string(),
        // IPC — shared memory, semaphores
        "(allow ipc*)".to_string(),
        // Sysctl — process info, hw.ncpu, etc.
        "(allow sysctl*)".to_string(),
        // Pseudo-terminals — needed for interactive commands
        "(allow pseudo-tty)".to_string(),
        // Broad file reads — bash and tools need system libs, dyld cache, etc.
        "(allow file-read*)".to_string(),
        // /dev devices — bash needs /dev/null, /dev/urandom, tty
        "(allow file-write* (subpath \"/dev\"))".to_string(),
    ];

    // Deny reads for the user's home directory — prevents reading repos,
    // personal files, and other data outside the workspace. Later rules
    // re-allow workspace and configured paths.
    let home = home_dir();
    let home_escaped = escape_sbpl_path(&home.to_string_lossy());
    rules.push(format!(
        "(deny file-read* file-write* (subpath \"{}\"))",
        home_escaped
    ));

    // Re-allow read+write for workspace (within home)
    let workspace_escaped = escape_sbpl_path(&policy.workspace_path.to_string_lossy());
    rules.push(format!(
        "(allow file-read* file-write* (subpath \"{}\"))",
        workspace_escaped
    ));

    // Re-allow read+write for extra writable paths (/tmp, user-configured)
    for path in &policy.extra_write_paths {
        let escaped = escape_sbpl_path(&path.to_string_lossy());
        rules.push(format!(
            "(allow file-read* file-write* (subpath \"{}\"))",
            escaped
        ));
    }

    // Deny credential directories explicitly (belt-and-suspenders over home deny)
    for path in &policy.deny_paths {
        let escaped = escape_sbpl_path(&path.to_string_lossy());
        rules.push(format!(
            "(deny file-read* file-write* (subpath \"{}\"))",
            escaped
        ));
    }

    // Network policy — deny all network by default
    match &policy.network {
        NetworkPolicy::Deny => {
            rules.push("(deny network*)".to_string());
        }
        NetworkPolicy::AllowProxy(socket_path) => {
            let escaped = escape_sbpl_path(&socket_path.to_string_lossy());
            rules.push("(deny network*)".to_string());
            rules.push(format!(
                "(allow network-outbound (local file \"{}\"))",
                escaped
            ));
        }
    }

    rules.join("\n")
}

/// Get the user's home directory.
fn home_dir() -> std::path::PathBuf {
    directories::BaseDirs::new()
        .map(|b| b.home_dir().to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("/Users/unknown"))
}

/// Escape a path string for use in SBPL profiles.
fn escape_sbpl_path(path: &str) -> String {
    path.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::policy::{NetworkPolicy, SandboxLevel, SandboxPolicy};
    use std::path::PathBuf;

    fn test_policy() -> SandboxPolicy {
        SandboxPolicy {
            workspace_path: PathBuf::from("/Users/test/project"),
            read_only_paths: vec![
                PathBuf::from("/usr"),
                PathBuf::from("/bin"),
                PathBuf::from("/Library"),
            ],
            extra_write_paths: vec![PathBuf::from("/tmp")],
            deny_paths: vec![
                PathBuf::from("/Users/test/.ssh"),
                PathBuf::from("/Users/test/.aws"),
            ],
            network: NetworkPolicy::Deny,
            timeout_secs: 120,
            max_output_bytes: 1_048_576,
            max_file_size_bytes: 52_428_800,
            max_processes: 64,
            level: SandboxLevel::Standard,
        }
    }

    #[test]
    fn test_generate_sbpl_profile_contains_deny_default() {
        let policy = test_policy();
        let profile = generate_sbpl_profile(&policy);
        assert!(profile.contains("(deny default)"));
    }

    #[test]
    fn test_generate_sbpl_profile_allows_workspace_read_write() {
        let policy = test_policy();
        let profile = generate_sbpl_profile(&policy);
        assert!(
            profile.contains("(allow file-read* file-write* (subpath \"/Users/test/project\"))")
        );
    }

    #[test]
    fn test_generate_sbpl_profile_denies_home_directory() {
        let policy = test_policy();
        let profile = generate_sbpl_profile(&policy);
        let home = home_dir();
        let home_str = home.to_string_lossy();
        // Home directory should be denied
        assert!(
            profile.contains(&format!(
                "(deny file-read* file-write* (subpath \"{}\"))",
                home_str
            )),
            "Profile should deny home directory: {}",
            home_str
        );
    }

    #[test]
    fn test_generate_sbpl_profile_denies_network() {
        let policy = test_policy();
        let profile = generate_sbpl_profile(&policy);
        assert!(profile.contains("(deny network*)"));
    }

    #[test]
    fn test_generate_sbpl_profile_denies_credentials() {
        let policy = test_policy();
        let profile = generate_sbpl_profile(&policy);
        assert!(profile.contains("(deny file-read* file-write* (subpath \"/Users/test/.ssh\"))"));
        assert!(profile.contains("(deny file-read* file-write* (subpath \"/Users/test/.aws\"))"));
    }

    #[test]
    fn test_escape_sbpl_path() {
        assert_eq!(escape_sbpl_path("/simple/path"), "/simple/path");
        assert_eq!(
            escape_sbpl_path("/path/with \"quotes\""),
            "/path/with \\\"quotes\\\""
        );
        assert_eq!(
            escape_sbpl_path("/path\\with\\backslashes"),
            "/path\\\\with\\\\backslashes"
        );
    }

    #[test]
    fn test_generate_sbpl_profile_allows_tmp_read_write() {
        let policy = test_policy();
        let profile = generate_sbpl_profile(&policy);
        assert!(profile.contains("(allow file-read* file-write* (subpath \"/tmp\"))"));
    }

    #[test]
    fn test_generate_sbpl_profile_has_broad_read_then_home_deny() {
        let policy = test_policy();
        let profile = generate_sbpl_profile(&policy);
        // Should have broad file-read* (bash needs it for system libs)
        assert!(profile.contains("(allow file-read*)"));
        // But home directory should be denied AFTER the broad allow
        let home = home_dir();
        let home_deny = format!(
            "(deny file-read* file-write* (subpath \"{}\"))",
            home.to_string_lossy()
        );
        let read_pos = profile.find("(allow file-read*)").unwrap();
        let deny_pos = profile.find(&home_deny).unwrap();
        assert!(
            deny_pos > read_pos,
            "Home deny must come after broad read allow"
        );
    }
}

use super::policy::SandboxLevel;

/// Detected sandbox capabilities of the current platform.
#[derive(Debug, Clone)]
pub struct SandboxCapabilities {
    /// Landlock LSM availability and ABI version (Linux only).
    pub landlock_abi: Option<u32>,

    /// Whether seccomp-bpf is available (Linux only).
    pub seccomp_available: bool,

    /// Whether Seatbelt/sandbox-exec is available (macOS only).
    pub seatbelt_available: bool,

    /// The highest enforcement level available.
    pub level: SandboxLevel,
}

/// Probe the current system for sandbox capabilities.
pub fn detect_capabilities() -> SandboxCapabilities {
    #[cfg(target_os = "linux")]
    {
        detect_linux()
    }

    #[cfg(target_os = "macos")]
    {
        detect_macos()
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        SandboxCapabilities {
            landlock_abi: None,
            seccomp_available: false,
            seatbelt_available: false,
            level: SandboxLevel::None,
        }
    }
}

#[cfg(target_os = "linux")]
fn detect_linux() -> SandboxCapabilities {
    // Probe Landlock ABI version by trying to create a ruleset
    let landlock_abi = probe_landlock_abi();

    // seccomp is available if prctl(PR_SET_NO_NEW_PRIVS) would succeed.
    // On modern kernels (3.5+) this is always available for unprivileged processes.
    let seccomp_available = probe_seccomp();

    let level = match (landlock_abi, seccomp_available) {
        (Some(abi), true) if abi >= 4 => SandboxLevel::Full,
        (Some(_), true) => SandboxLevel::Standard,
        (None, true) => SandboxLevel::Minimal,
        _ => SandboxLevel::None,
    };

    SandboxCapabilities {
        landlock_abi,
        seccomp_available,
        seatbelt_available: false,
        level,
    }
}

#[cfg(target_os = "linux")]
fn probe_landlock_abi() -> Option<u32> {
    // Try to detect the supported Landlock ABI version.
    // We try each ABI from highest to lowest.
    use landlock::{Access, AccessFs, Ruleset, RulesetAttr, ABI};

    for (abi, version) in [
        (ABI::V5, 5u32),
        (ABI::V4, 4),
        (ABI::V3, 3),
        (ABI::V2, 2),
        (ABI::V1, 1),
    ] {
        let result = Ruleset::default().handle_access(AccessFs::from_all(abi));
        if result.is_ok() {
            return Some(version);
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn probe_seccomp() -> bool {
    // Check if /proc/self/status contains Seccomp field
    // (available on kernels 3.8+, which is essentially all modern systems)
    std::fs::read_to_string("/proc/self/status")
        .map(|s| s.contains("Seccomp:"))
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn detect_macos() -> SandboxCapabilities {
    // Check if sandbox-exec binary exists
    let seatbelt_available = std::path::Path::new("/usr/bin/sandbox-exec").exists();

    let level = if seatbelt_available {
        SandboxLevel::Standard
    } else {
        SandboxLevel::None
    };

    SandboxCapabilities {
        landlock_abi: None,
        seccomp_available: false,
        seatbelt_available,
        level,
    }
}

impl SandboxCapabilities {
    /// Resolve the effective level given a user's config setting.
    pub fn effective_level(&self, config_level: &str) -> SandboxLevel {
        match config_level {
            "full" => {
                if self.level >= SandboxLevel::Full {
                    SandboxLevel::Full
                } else {
                    self.level
                }
            }
            "standard" => {
                if self.level >= SandboxLevel::Standard {
                    SandboxLevel::Standard
                } else {
                    self.level
                }
            }
            "minimal" => {
                if self.level >= SandboxLevel::Minimal {
                    SandboxLevel::Minimal
                } else {
                    self.level
                }
            }
            "none" => SandboxLevel::None,
            // "auto" or anything else: use the highest available
            _ => self.level,
        }
    }

    /// Human-readable status lines for `sandbox status` command.
    pub fn status_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();

        #[cfg(target_os = "linux")]
        {
            if let Some(abi) = self.landlock_abi {
                lines.push(format!("  Landlock:  v{:<3}                    ok", abi));
            } else {
                lines.push("  Landlock:  not available           --".to_string());
            }

            if self.seccomp_available {
                lines.push("  Seccomp:   available               ok".to_string());
            } else {
                lines.push("  Seccomp:   not available           --".to_string());
            }
        }

        #[cfg(target_os = "macos")]
        {
            if self.seatbelt_available {
                lines.push("  Seatbelt:  available               ok".to_string());
            } else {
                lines.push("  Seatbelt:  not available           --".to_string());
            }
        }

        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            lines.push("  Platform:  unsupported             --".to_string());
        }

        lines.push(format!("  Level:     {:?}", self.level));

        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_capabilities_runs() {
        let caps = detect_capabilities();
        // Should not panic
        assert!(caps.level <= SandboxLevel::Full);
    }

    #[test]
    fn test_effective_level_auto() {
        let caps = SandboxCapabilities {
            landlock_abi: None,
            seccomp_available: false,
            seatbelt_available: true,
            level: SandboxLevel::Standard,
        };
        assert_eq!(caps.effective_level("auto"), SandboxLevel::Standard);
    }

    #[test]
    fn test_effective_level_none() {
        let caps = SandboxCapabilities {
            landlock_abi: Some(5),
            seccomp_available: true,
            seatbelt_available: false,
            level: SandboxLevel::Full,
        };
        assert_eq!(caps.effective_level("none"), SandboxLevel::None);
    }

    #[test]
    fn test_effective_level_clamps() {
        let caps = SandboxCapabilities {
            landlock_abi: None,
            seccomp_available: false,
            seatbelt_available: false,
            level: SandboxLevel::None,
        };
        // Asking for "full" but platform only supports None
        assert_eq!(caps.effective_level("full"), SandboxLevel::None);
    }

    #[test]
    fn test_status_lines() {
        let caps = detect_capabilities();
        let lines = caps.status_lines();
        assert!(!lines.is_empty());
    }
}

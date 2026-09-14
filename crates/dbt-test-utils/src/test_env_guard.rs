use std::collections::HashMap;
use std::env;

/// Environment variable guard that isolates tests from external environment variables.
///
/// **CAUTION**: Uses unsafe env::remove_var and env::set_var methods,
/// that will NOT work as expected if multiple tests running on different threads
/// collide on the same environment variable they need with different values.
///
/// This will only impact `cargo test` since `cargo nextest` uses 1 process per test.
///
/// This guard clears all environment variables except a pre-defined list of strict
/// matching exclusions (see `TestEnvGuard::EXACT_MATCH_VARS` for non-OS specific ones
/// and `TestEnvGuard::OS_EXACT_MATCH_VARS` for os specific) and those with specified prefixes,
/// ensuring tests run in a clean environment while preserving necessary variables.
///
/// Check `TestEnvGuard::DEFAULT_ALLOWED_PREFIXES` for the up to-date list of allowed prefixes.
///
/// The original environment is automatically restored when the guard is dropped.
#[cfg_attr(windows, allow(dead_code))]
pub struct TestEnvGuard {
    // The state of the environment when the guard was created.
    saved_env: HashMap<String, String>,
    // List of environment variables that were preserved and thus should not
    // be restored when the guard is dropped.
    pass_through_vars: Vec<String>,
}

impl TestEnvGuard {
    const DEFAULT_ALLOWED_PREFIXES: &'static [&'static str] = &["RUST_", "CARGO_", "GITHUB_"];

    const EXACT_MATCH_VARS: &'static [&'static str] = &[
        "_DBT_SKIP_DEV_LICENSE",
        "DBT_SKIP_REMOTE_LICENSE",
        "ADAPTER_AUTH_CREDS",
        "ADAPTER_POSTGRES",
        "ADAPTER_RECORD",
        "CI",
        "CONTINUOUS_INTEGRATION",
        "FUSION_ADAPTERS_CREDS",
        "FUSION_TESTING_KW27752_SNOWFLAKE_PRIVATE_KEY",
        "FUSION_TESTING_KW27752_SNOWFLAKE_PRIVATE_KEY_PASSPHRASE",
        "GOLDIE_UPDATE",
        "HOME",
        "INSIDE_EMACS",
        "IS_DOCKER_ENABLED",
        "NEXTEST",
        "PATH",
        "PGPORT",
        "RUSTFLAGS",
        "SSH_AGENT_PID",
        "SSH_AUTH_SOCK",
        "TMPDIR",
        "USER",
        // These allow locally testing if a driver change breaks any test.
        // DISABLE_CDN_DRIVER_CACHE flips the driver loader into Bundled mode,
        // ADBC_REPOSITORY points at the directory holding the .so to dlopen,
        // and DISABLE_AUTO_DRIVER_REBUILD skips the `make -C <ADBC_REPOSITORY>`
        // step (used when the local repo doesn't ship a Makefile at that path).
        "DISABLE_CDN_DRIVER_CACHE",
        "ADBC_REPOSITORY",
        "DISABLE_AUTO_DRIVER_REBUILD",
        // Used to force vortex client into dev mode for testing
        "VORTEX_DEV_MODE",
        "VORTEX_DEV_MODE_OUTPUT_PATH",
        // Opt-in capture of test OTEL files + rendered summaries for local
        // debugging / comparing runs (see CompareThreadConcurrencySummary).
        "DBT_TEST_OTEL_CAPTURE_DIR",
        "DBT_TEST_OTEL_LABEL",
    ];

    /// Windows-specific environment variables required for system operations and DNS resolution
    #[cfg(windows)]
    const OS_EXACT_MATCH_VARS: &'static [&'static str] = &[
        "ALLUSERSPROFILE",           // All users profile directory
        "APPDATA",                   // User application data directory
        "CD",                        // Current directory
        "CLIENTNAME",                // Client computer name for terminal services
        "CMDCMDLINE",                // Command line that started cmd.exe
        "CMDEXTVERSION",             // Command extension version
        "CommonProgramFiles",        // Common program files directory
        "CommonProgramFiles(x86)",   // Common program files x86 directory
        "CommonProgramW6432",        // 64-bit common program files directory
        "COMPUTERNAME",              // Computer name - critical for network/DNS operations
        "COMSPEC",                   // Command processor path, used by various Windows utilities
        "DATE",                      // Current date
        "ERRORLEVEL",                // Last command error level
        "HOMEDRIVE",                 // User home drive
        "HOMEPATH",                  // User home path
        "LOCALAPPDATA",              // Local application data directory
        "LOGONSERVER",               // Logon server
        "NUMBER_OF_PROCESSORS",      // Number of processors available
        "OS",                        // Operating system name
        "PATHEXT",                   // Executable file extensions
        "PROCESSOR_ARCHITECTURE",    // Processor architecture (x86, AMD64, etc.)
        "PROCESSOR_IDENTIFIER",      // Processor identification string
        "PROCESSOR_LEVEL",           // Processor level
        "PROCESSOR_REVISION",        // Processor revision
        "ProgramData",               // Common application data directory
        "ProgramFiles",              // Program files directory path
        "ProgramFiles(x86)",         // Program files x86 directory
        "ProgramW6432",              // 64-bit program files directory
        "PROMPT",                    // Command prompt format
        "PSModulePath",              // PowerShell module path
        "PUBLIC",                    // Public user directory
        "RANDOM",                    // Random number generator seed
        "SessionName",               // Current logon session name (standard casing)
        "SystemDrive",               // System drive letter (typically C:)
        "SystemRoot",                // System root directory (standard casing)
        "TEMP",                      // Windows temporary directory
        "TIME",                      // Current time
        "TMP",                       // Windows temporary directory (alternative)
        "USERDNSDOMAIN", // DNS domain for current user - critical for network operations
        "USERDOMAIN_ROAMINGPROFILE", // User domain roaming profile path
        "USERDOMAIN",    // User domain
        "USERNAME",      // Current username
        "USERPROFILE",   // User profile directory
        "WINDIR",        // Windows directory path needed for system components
    ];

    /// Unix/POSIX-specific environment variables to preserve (empty for now)
    #[cfg(not(windows))]
    const OS_EXACT_MATCH_VARS: &'static [&'static str] = &[];

    /// Create a new environment guard with custom allowed env vars.
    ///
    /// # Arguments
    /// * `allowed_vars` - List of exact environment variable names to preserve
    /// * `allowed_prefixes` - List of environment variable prefixes to preserve
    ///
    /// # Example
    /// ```
    /// use dbt_test_utils::testing::TestEnvGuard;
    ///
    /// let _guard = TestEnvGuard::new_with_allowed_prefixes(, &["PATH"], &[
    ///     "RUST_",
    ///     "CARGO_",
    ///     "PATH",
    ///     "MY_TEST_VAR_",
    /// ]);
    /// ```
    #[cfg_attr(windows, allow(unused_variables, unreachable_code))]
    pub fn new(allowed_vars: &[&str], allowed_prefixes: &[&str]) -> Self {
        // TODO: Unfortunately, using the guard triggers random memory access failures on Windows.
        // Thus the guard is disabled on Windows for now. Since it is primarily for development
        // and Windows is only used in CI, this is acceptable for now.
        #[cfg(windows)]
        {
            return Self {
                saved_env: Default::default(),
                pass_through_vars: Default::default(),
            };
        }

        // Save the current environment
        let saved_env: HashMap<String, String> = env::vars().collect();
        let mut pass_through_vars = vec![];

        // Remove environment variables that don't match allowed prefixes
        for key in saved_env.keys() {
            if !Self::should_keep_var(key, allowed_vars, allowed_prefixes) {
                unsafe {
                    #[allow(clippy::disallowed_methods)]
                    env::remove_var(key);
                }
            } else {
                // If the variable is allowed, add it to the list of allowed vars
                pass_through_vars.push(key.clone());
            }
        }

        Self {
            saved_env,
            pass_through_vars,
        }
    }

    /// Check if a variable should be kept based on the allowed prefixes
    fn should_keep_var(key: &str, allowed_vars: &[&str], allowed_prefixes: &[&str]) -> bool {
        // CHeck exact matches first
        if allowed_vars.iter().any(|var| {
            // Use OS-appropriate comparison for exact matches
            Self::env_var_name_matches(key, var)
        }) {
            return true;
        }

        // Now check if the key starts with any of the allowed prefixes
        allowed_prefixes.iter().any(|prefix| {
            // For prefixes, use OS-appropriate case sensitivity
            Self::env_var_name_starts_with(key, prefix)
        })
    }

    /// Compare environment variable names using OS-appropriate case sensitivity
    #[cfg(windows)]
    #[allow(dead_code)]
    fn env_var_name_matches(key: &str, expected: &str) -> bool {
        key.eq_ignore_ascii_case(expected)
    }

    /// Compare environment variable names using OS-appropriate case sensitivity
    #[cfg(not(windows))]
    fn env_var_name_matches(key: &str, expected: &str) -> bool {
        key == expected
    }

    /// Check if environment variable name starts with prefix using OS-appropriate case sensitivity
    #[cfg(windows)]
    #[allow(dead_code)]
    fn env_var_name_starts_with(key: &str, prefix: &str) -> bool {
        key.len() >= prefix.len() && key[..prefix.len()].eq_ignore_ascii_case(prefix)
    }

    /// Check if environment variable name starts with prefix using OS-appropriate case sensitivity
    #[cfg(not(windows))]
    fn env_var_name_starts_with(key: &str, prefix: &str) -> bool {
        key.starts_with(prefix)
    }
}

impl Drop for TestEnvGuard {
    fn drop(&mut self) {
        // TODO: Unfortunately, using the guard triggers random memory access failures on Windows.
        // Thus the guard is disabled on Windows for now. Since it is primarily for development
        // and Windows is only used in CI, this is acceptable for now.
        #[cfg(not(windows))]
        {
            // Restore environment
            let cur_vars = env::vars().collect::<HashMap<String, String>>();

            // Remove or restore environment variables created/modified within the guard
            for key in cur_vars.keys() {
                if self.pass_through_vars.contains(key) {
                    // Skip variables that were preserved
                    continue;
                }

                match self.saved_env.get(key) {
                    Some(value) => {
                        // Restore the original value
                        unsafe {
                            #[allow(clippy::disallowed_methods)]
                            env::set_var(key, value);
                        }
                    }
                    None => {
                        // If not in saved_env, remove it
                        unsafe {
                            #[allow(clippy::disallowed_methods)]
                            env::remove_var(key);
                        }
                    }
                }
            }

            // Restore variables removed by the guard
            for (key, value) in &self.saved_env {
                if !cur_vars.contains_key(key) {
                    // If the variable was removed, restore it
                    unsafe {
                        #[allow(clippy::disallowed_methods)]
                        env::set_var(key, value);
                    }
                }
            }
        }
    }
}

impl Default for TestEnvGuard {
    fn default() -> Self {
        // Combine base exact match vars with OS-specific vars
        let mut allowed_vars = Self::EXACT_MATCH_VARS.to_vec();
        allowed_vars.extend(Self::OS_EXACT_MATCH_VARS);

        Self::new(&allowed_vars, Self::DEFAULT_ALLOWED_PREFIXES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(not(windows))]
    fn test_env_guard_basic_functionality() {
        // Set a test variable before creating the guard
        unsafe {
            #[allow(clippy::disallowed_methods)]
            env::set_var("TEST_VAR_BEFORE_GUARD", "value1");
        }

        {
            let _guard = TestEnvGuard::default();

            // TEST_VAR_BEFORE_GUARD should be removed (not in allowlist)
            assert!(env::var("TEST_VAR_BEFORE_GUARD").is_err());

            // PATH should be preserved (it's essential for test execution)
            // Check using OS-appropriate case sensitivity
            let path_exists =
                env::vars().any(|(key, _)| TestEnvGuard::env_var_name_matches(&key, "PATH"));
            assert!(path_exists, "PATH environment variable should be preserved");
        }

        // After guard is dropped, original environment should be restored
        assert_eq!(env::var("TEST_VAR_BEFORE_GUARD").unwrap(), "value1");
        assert!(env::var("TEST_VAR_WITHIN_GUARD").is_err());

        unsafe {
            #[allow(clippy::disallowed_methods)]
            env::set_var("CUSTOM_PREFIX_VAR", "custom_value");
            #[allow(clippy::disallowed_methods)]
            env::set_var("UNWANTED_VAR", "unwanted_value");
        }

        unsafe {
            #[allow(clippy::disallowed_methods)]
            env::set_var("TEST_PASS_THROUGH", "value_before_guard");
        }

        {
            let _guard = TestEnvGuard::new(
                &["TEST_PASS_THROUGH"],
                &["RUST_", "CARGO_", "PATH", "CUSTOM_PREFIX_"],
            );

            // Change the value of the pass-through variable.
            // It should preserve the changed value after the guard is dropped.
            unsafe {
                #[allow(clippy::disallowed_methods)]
                env::set_var("TEST_PASS_THROUGH", "value_within_guard");
            }

            // Custom prefix should be preserved
            assert_eq!(env::var("CUSTOM_PREFIX_VAR").unwrap(), "custom_value");

            // Non-allowlisted variable should be removed
            assert!(env::var("UNWANTED_VAR").is_err());
        }

        // After guard is dropped, original environment should be restored
        assert_eq!(env::var("CUSTOM_PREFIX_VAR").unwrap(), "custom_value");
        assert_eq!(env::var("UNWANTED_VAR").unwrap(), "unwanted_value");

        // But the pass-through variable should retain its value set within the guard
        assert_eq!(env::var("TEST_PASS_THROUGH").unwrap(), "value_within_guard");
    }

    #[test]
    #[cfg(windows)]
    fn test_env_guard_windows_no_op() {
        // On Windows, the guard is a no-op, so just test that it can be created
        let _guard = TestEnvGuard::default();
        // The guard should not modify the environment on Windows
        // This test mainly ensures the code compiles and runs without panicking
    }
}

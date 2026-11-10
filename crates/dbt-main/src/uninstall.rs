use std::env;

#[cfg(target_os = "windows")]
use std::{
    fs::File,
    io::Write,
    process::{Command, Stdio},
};

#[cfg(target_os = "windows")]
use uuid::Uuid;

use dbt_common::FsResult;
#[cfg(target_os = "windows")]
use dbt_common::constants::DBT_CDN_URL;
use dbt_common::{ErrorCode, err};
use dbt_dist::DistInfo;

/// Enforces that dbt may remove its own binary, raising a `NotSupported` error
/// otherwise. Only a self-managed install (the standalone installer, or a
/// native binary no package manager claims) may remove itself; removing a
/// binary owned by Homebrew, pip, or winget would leave that manager pointing
/// at a file that no longer exists, so we refuse and surface that manager's
/// uninstall command instead.
fn ensure_is_not_managed_installation(dist_info: &DistInfo) -> FsResult<()> {
    if dist_info.is_self_managed() {
        return Ok(());
    }
    if let Some(message) = dist_info.unsupported_channel_message("uninstall") {
        return err!(ErrorCode::NotSupported, "{}", message);
    }
    match &dist_info.uninstall_cmd {
        Some(command) => err!(
            ErrorCode::NotSupported,
            "dbt was installed via {}. To uninstall, run:\n\n    {}\n\n\
             (Removing the binary here would leave {} thinking dbt is still installed.)",
            dist_info.install_label(),
            command,
            dist_info.install_label(),
        ),
        None => err!(
            ErrorCode::NotSupported,
            "dbt was installed by another package manager, so it can't uninstall itself. \
             Please uninstall dbt using the package manager you installed it with."
        ),
    }
}

/// The directory a running `dbt` binary lives in, used to locate its
/// `dbt-db-runner` sibling -- that companion binary always keeps its literal
/// name regardless of what the main binary is named (e.g. an alias like
/// `dbtf`), so its location is derived from the directory rather than the
/// main binary's own name.
fn parent_dir_of(exe_path: &str) -> Option<String> {
    std::path::Path::new(exe_path)
        .parent()
        .and_then(|p| p.to_str())
        .map(str::to_string)
}

/// Removes the running `dbt` binary itself, plus its `dbt-db-runner`
/// sibling if present. Deletes `exe_path` directly rather than
/// reconstructing "the directory plus a hardcoded `dbt` filename" -- the
/// binary can be named anything (e.g. `dbtf`, as the VS Code extension does
/// to avoid colliding with a separate OSS `dbt` on PATH), so `exe_path`
/// itself is the only value that's guaranteed to name the file actually
/// being run. A sibling `dbt` in the same directory must survive this call.
///
/// The runner's removal is best-effort: it's an optional companion binary,
/// and a permissions issue or its absence shouldn't fail the primary
/// binary's removal.
#[cfg(not(target_os = "windows"))]
fn remove_self_managed_binary(exe_path: &str) -> FsResult<()> {
    dbt_common::stdfs::remove_file(exe_path)?;
    if let Some(dir) = parent_dir_of(exe_path) {
        let runner_path = std::path::Path::new(&dir).join("dbt-db-runner");
        let _ = dbt_common::stdfs::remove_file(&runner_path);
    }
    Ok(())
}

#[cfg_attr(target_os = "windows", allow(unreachable_code))]
pub async fn exec_uninstall(command_name: &str) -> FsResult<()> {
    ensure_is_not_managed_installation(&DistInfo::current(command_name)?)?;

    println!("Removing dbt from your system");

    let mut curr_path = String::new();
    match env::current_exe() {
        Ok(exe_path) => {
            let _ = &exe_path.to_str().unwrap().clone_into(&mut curr_path);
        }

        Err(_e) => {
            return err!(ErrorCode::IoError, "Failed to get current exe path.");
        }
    };

    let mut pre_string: String = "Current exe at ".to_owned();
    pre_string.push_str(&curr_path);
    //console.println(Prty::progress(ANALYZING, &pre_string, ""));

    // Deleting `curr_path` directly (rather than downloading and running
    // uninstall.sh) removes exactly the binary that was invoked, whatever
    // it's named -- no network access needed either. uninstall.sh remains
    // untouched on the CDN for any other caller that still shells out to it
    // directly (e.g. the standalone install.sh's own upgrade path).
    #[cfg(not(target_os = "windows"))]
    remove_self_managed_binary(&curr_path)?;

    // Windows can't delete its own running executable, so this still has to
    // shell out to a script launched in a detached process after dbt exits.
    #[cfg(target_os = "windows")]
    {
        let script_url = format!("{DBT_CDN_URL}/install/uninstall.ps1");
        let response = reqwest::get(&script_url)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

        let script = response
            .text()
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

        // Create a temporary directory for the script with a unique filename
        let temp_dir = env::temp_dir();
        let unique_id = Uuid::new_v4().to_string();
        let script_path = temp_dir.join(format!("uninstall_{unique_id}.ps1"));

        // Write the PowerShell script to a temporary file
        let mut file = match File::create(&script_path) {
            Ok(file) => file,
            Err(e) => {
                return err!(
                    ErrorCode::IoError,
                    "Failed to create temporary script file: {}",
                    e
                );
            }
        };

        if let Err(e) = file.write_all(script.as_bytes()) {
            return err!(
                ErrorCode::IoError,
                "Failed to write to temporary script file: {}",
                e
            );
        }

        // Important: Close the file handle before executing
        drop(file);

        let path_str = script_path
            .to_string_lossy()
            .to_string()
            .replace("\\", "\\\\");

        // Single-quoted PowerShell string: the only character that needs
        // escaping is a literal single quote, doubled per PowerShell syntax.
        // Passing the full binary path (not a directory + hardcoded
        // `dbt.exe`) means a renamed binary (e.g. `dbtf.exe`, as the VS Code
        // extension uses to avoid colliding with a separate OSS `dbt` on
        // PATH) is the exact file removed, not a same-directory sibling
        // that happens to be named `dbt.exe`.
        let binary_path_arg = curr_path.replace('\'', "''");

        // Determine which PowerShell to use (pwsh vs powershell)
        let ps_exe = if env::var("PSModulePath").is_ok_and(|path| path.contains("PowerShell/7")) {
            "pwsh"
        } else {
            "powershell"
        };

        // Launch PowerShell and exit dbt to release the file lock
        match Command::new(ps_exe)
            .args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                &format!("& '{path_str}' -BinaryPath '{binary_path_arg}'"),
            ])
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
        {
            Ok(_child) => {
                // Wait briefly to ensure PowerShell starts
                std::thread::sleep(std::time::Duration::from_millis(100));
                // Exit dbt to release the file lock
                std::process::exit(0);
            }
            Err(e) => {
                return err!(
                    ErrorCode::IoError,
                    "Failed to start uninstall process: {}",
                    e
                );
            }
        }
    }

    println!("Successfully removed dbt.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_dist::Channel;

    #[test]
    fn parent_dir_of_a_non_standard_location_is_not_the_canonical_default() {
        // Regression test: the uninstall scripts default to `~/.local/bin`
        // when no install location is passed. This must resolve to the
        // binary's *own* directory instead, or an Unclaimed binary living
        // elsewhere would have a completely unrelated install silently
        // targeted for removal.
        let dir = parent_dir_of("/Users/dev/dbt-channel-tests/unclaimed/dbt").unwrap();
        assert_eq!(dir, "/Users/dev/dbt-channel-tests/unclaimed");
    }

    #[test]
    fn parent_dir_of_root_has_no_parent() {
        assert_eq!(parent_dir_of("/"), None);
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn remove_self_managed_binary_only_removes_the_invoked_file() {
        // Regression test: a renamed binary (e.g. `dbtf`, as the VS Code
        // extension names it to avoid colliding with a separate OSS `dbt`
        // on PATH) sitting alongside a real `dbt` in the same directory
        // must not have that sibling removed instead of itself.
        let dir = tempfile::tempdir().unwrap();
        let renamed = dir.path().join("dbtf");
        let sibling = dir.path().join("dbt");
        std::fs::write(&renamed, b"renamed-binary").unwrap();
        std::fs::write(&sibling, b"sibling-binary").unwrap();

        remove_self_managed_binary(renamed.to_str().unwrap()).unwrap();

        assert!(!renamed.exists(), "the invoked binary should be removed");
        assert!(sibling.exists(), "the unrelated sibling must survive");
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn remove_self_managed_binary_also_removes_its_runner_sibling() {
        let dir = tempfile::tempdir().unwrap();
        let dbt_bin = dir.path().join("dbt");
        let runner = dir.path().join("dbt-db-runner");
        std::fs::write(&dbt_bin, b"dbt-binary").unwrap();
        std::fs::write(&runner, b"runner-binary").unwrap();

        remove_self_managed_binary(dbt_bin.to_str().unwrap()).unwrap();

        assert!(!dbt_bin.exists());
        assert!(
            !runner.exists(),
            "the runner sibling should also be removed"
        );
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn remove_self_managed_binary_ignores_a_missing_runner_sibling() {
        let dir = tempfile::tempdir().unwrap();
        let dbt_bin = dir.path().join("dbt");
        std::fs::write(&dbt_bin, b"dbt-binary").unwrap();

        // No dbt-db-runner alongside it -- must not fail just because the
        // optional runner companion doesn't exist.
        remove_self_managed_binary(dbt_bin.to_str().unwrap()).unwrap();

        assert!(!dbt_bin.exists());
    }

    fn dist_info(channel: Option<Channel>, uninstall_cmd: Option<&str>) -> DistInfo {
        DistInfo {
            path: "/usr/local/bin/dbt".to_string(),
            channel,
            distribution: None,
            generation: dbt_dist::Generation::V2,
            py_package_manager: None,
            py_venv_root: None,
            version: None,
            is_prerelease: None,
            upgrade_cmd: None,
            uninstall_cmd: uninstall_cmd.map(str::to_string),
        }
    }

    #[test]
    fn self_managed_install_may_self_uninstall() {
        for channel in [Channel::Standalone, Channel::Unclaimed] {
            let info = dist_info(Some(channel), Some("dbt system uninstall"));
            assert!(ensure_is_not_managed_installation(&info).is_ok());
        }
    }

    #[test]
    fn package_managed_installs_surface_their_uninstall_command() {
        for (channel, command) in [
            (Channel::Brew, "brew uninstall dbt"),
            (Channel::Winget, "winget uninstall --id dbtLabs.dbt --exact"),
            (Channel::Pypi, "pip uninstall dbt"),
        ] {
            let info = dist_info(Some(channel.clone()), Some(command));
            let err = ensure_is_not_managed_installation(&info)
                .expect_err(&format!("{channel:?} should be blocked"));
            assert!(
                err.context.contains(command),
                "{channel:?} message should mention `{command}`, got: {}",
                err.context
            );
        }
    }

    #[test]
    fn unsupported_channel_names_the_manager_and_points_at_install_docs() {
        let info = dist_info(Some(Channel::Unsupported("Scoop".to_string())), None);
        let err = ensure_is_not_managed_installation(&info)
            .expect_err("an unsupported channel should be blocked");
        assert!(
            err.context.contains("Scoop"),
            "message should name the manager, got: {}",
            err.context
        );
        assert!(
            err.context
                .contains("https://docs.getdbt.com/docs/local/install-dbt?version=2"),
            "message should point at install docs, got: {}",
            err.context
        );
    }

    #[test]
    fn unresolved_install_gets_generic_fallback() {
        let info = dist_info(None, None);
        let err = ensure_is_not_managed_installation(&info).expect_err("should be blocked");
        assert!(
            err.context
                .contains("package manager you installed it with")
        );
    }
}

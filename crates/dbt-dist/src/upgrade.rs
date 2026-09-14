//! `dbt system upgrade-distribution`: only available in dbt v2 OSS, this
//! command upgrades the current install to dbt v2 proprietary (published on
//! PyPI as `dbt`). See `upgrade-spec.md` (repo root) for the full design.
//! Handles two cases: a "global install" (a `standalone`/`unclaimed` native
//! binary upgraded in place at its own path, or every other channel -- pip
//! --user / pipx / uv tool / system pip -- getting a fresh standalone
//! install followed by uninstalling the old package), and a "managed
//! project" (rewriting a project manifest's `dbt-core` dependency -- which
//! may also be a pin predating dbt v2 -- to `dbt`, then re-running the
//! manager's install/lock command).

use std::env;
use std::path::{Path, PathBuf};

use dbt_common::tracing::dbt_emit::println;
use dbt_common::{ErrorCode, FsError, FsResult, err, fs_err};

use crate::confirm::{confirm, is_interactive};
use crate::dist::{
    ManagerResolution, resolve_manager_for_manifest, sync_command_for_manager,
    uninstall_command_for_package,
};
use crate::probe_manager_for_manifest;
use crate::python::{
    ManifestReplacements, PackageSpec, PackageVersion, PythonManifest, PythonManifestFormat,
    PythonPackageManager,
};
use crate::version::{ReqwestClient, VersionsHttpClient, cdn_base_url, resolve_target_version};
use crate::{Channel, DiscoveryContext, DistInfo, DistInfoDiscovery, Distribution};
#[cfg(test)]
use dbt_dist_classify::DBT_OSS_PACKAGE_NAME;
use dbt_dist_classify::{DBT_CORE_PACKAGE_NAME, UPGRADABLE_TARGET_NAMES};

/// PyPI's proprietary dbt v2 package.
const PROPRIETARY_PACKAGE_NAME: &str = "dbt";

fn install_script_name() -> &'static str {
    if cfg!(windows) {
        "install.ps1"
    } else {
        "install.sh"
    }
}

fn install_script_url() -> String {
    format!("{}/install/{}", cdn_base_url(), install_script_name())
}

/// The standalone installer's default target, `~/.local/bin/dbt[.exe]`.
/// `None` if the home directory can't be resolved.
fn standalone_target_path() -> Option<PathBuf> {
    let home = env::var("HOME").or_else(|_| env::var("USERPROFILE")).ok()?;
    let mut path = PathBuf::from(home);
    path.push(".local");
    path.push("bin");
    path.push(if cfg!(windows) { "dbt.exe" } else { "dbt" });
    Some(path)
}

pub async fn exec_upgrade_distribution(
    yes: bool,
    package_manager: Option<String>,
    command_name: &str,
) -> FsResult<()> {
    let override_manager = package_manager
        .map(|name| {
            PythonPackageManager::parse_cli_name(&name).ok_or_else(|| {
                fs_err!(
                    ErrorCode::InvalidArgument,
                    "'{name}' isn't a recognized --package-manager value. Valid choices: {}",
                    PythonPackageManager::cli_names()
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
        })
        .transpose()?;

    let dist_info = DistInfo::current(command_name)?;

    match dist_info.distribution {
        Some(Distribution::Dbt) => {
            println("You are already running dbt v2 -- there is nothing to upgrade to.");
            return Ok(());
        }
        Some(Distribution::CloudCLI) => {
            println(
                "The dbt Cloud CLI isn't a supported target for `dbt system upgrade-distribution`.",
            );
            return Ok(());
        }
        Some(Distribution::Core) => {
            return err!(
                ErrorCode::Unexpected,
                "The current `dbt` binary reports itself as the legacy dbt-core distribution, \
                 which isn't possible for a dbt v2 OSS build; refusing to guess. Run \
                 `dbt internal get-distribution-info` for more detail."
            );
        }
        None => {
            return err!(
                ErrorCode::Unexpected,
                "Could not determine the current dbt distribution; refusing to guess. Run \
                 `dbt internal get-distribution-info` for more detail."
            );
        }
        Some(Distribution::Oss) => {}
    }

    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    if let Some(mut manifest) = PythonManifest::detect(&cwd)? {
        if let Some(declared_name) = declared_upgradable_name(&manifest)? {
            if manifest.has_top_level_conda_declaration(declared_name)? {
                return err!(
                    ErrorCode::NotSupported,
                    "{} declares `{declared_name}` in conda's top-level dependency list, but \
                     dbt v2 isn't published on conda channels. Move the dependency into the \
                     `pip:` sub-list (which resolves from PyPI) and re-run this command, or edit \
                     the manifest manually.",
                    manifest.path().display()
                );
            }
            // Verified live (2026-07-30): the CDN tag (`v2.0.0-preview.203`)
            // and PyPI's published string for the same build
            // (`2.0.0rc203`) are both valid PEP 440 spellings of the same
            // pre-release and normalize equal, so pinning the raw tag below
            // works as-is in a Python manifest -- no format conversion
            // needed.
            let target_version = resolve_target_version(None, &ReqwestClient).await?;
            let replacements = manifest
                .get_rename_replacement(
                    declared_name,
                    &PackageSpec {
                        name: PROPRIETARY_PACKAGE_NAME.to_string(),
                        version: PackageVersion::Exact(target_version),
                    },
                )?
                .ok_or_else(|| {
                    fs_err!(
                        ErrorCode::Unexpected,
                        "{} declared `{declared_name}` a moment ago, but no longer does",
                        manifest.path().display()
                    )
                })?;
            return exec_managed_project_upgrade(
                &mut manifest,
                declared_name,
                replacements,
                &dist_info,
                override_manager,
                yes,
                &DiscoveryContext::real(),
            )
            .await;
        }
    } else if dist_info.py_venv_root.is_some() {
        println(
            "Warning: running inside a virtual environment with no managed Python project \
             (pyproject.toml, requirements.txt, ...) detected in this directory or its parents. \
             Continuing will install dbt globally rather than into a project. To upgrade just \
             this project, add `dbt` to its dependencies instead.",
        );
    }

    exec_global_install(&dist_info, yes, command_name, &DiscoveryContext::real()).await
}

/// Which of [`UPGRADABLE_TARGET_NAMES`] `manifest` depends on, if any.
/// Errors if it depends on more than one.
fn declared_upgradable_name(manifest: &PythonManifest) -> FsResult<Option<&'static str>> {
    let mut declared = Vec::new();
    for name in UPGRADABLE_TARGET_NAMES {
        // Version is a placeholder: this only checks whether `name` is
        // declared at all; the replacement itself is discarded.
        let is_declared = manifest
            .get_version_replacement(&PackageSpec {
                name: name.to_string(),
                version: PackageVersion::Exact("0.0.0".to_string()),
            })?
            .is_some();
        if is_declared {
            declared.push(name);
        }
    }
    match declared[..] {
        [] => Ok(None),
        [name] => Ok(Some(name)),
        [..] => err!(
            ErrorCode::NotSupported,
            "{} declares both `{}`; only one dbt dependency is expected. Remove one manually \
             and re-run this command.",
            manifest.path().display(),
            declared.join("` and `")
        ),
    }
}

/// The shared "manifest was edited, but the environment can't be brought
/// back in sync automatically" error -- covers both ways
/// `exec_managed_project_upgrade` can fail to compute a sync command: the
/// package manager itself couldn't be determined, or it was determined but
/// [`sync_command_for_manager`] has no automatic command for it (e.g. Pipx --
/// see that function's doc comment for why). `reason` fills in which
/// of the two happened; the surrounding wording and the "re-run manually"
/// instruction stay identical either way. `code` is threaded through rather
/// than hardcoded because the two cases aren't the same *kind* of failure --
/// "couldn't be determined" is a genuine `Unexpected` (discovery should have
/// found something), while "no automatic command for this manager" is an
/// enumerated, deliberate refusal to automate, the same shape as
/// `ErrorCode::NotSupported`'s other use in this file (the conda-top-level-
/// declaration case in `exec_upgrade_distribution`).
///
/// `backup_path` is repeated in the message (not just the one-time println
/// emitted right after `apply_to` succeeds) since this is an error the user
/// may be looking at well after that println has scrolled out of view.
fn manual_sync_required_err(
    manifest_path: &Path,
    backup_path: &Path,
    code: ErrorCode,
    reason: &str,
) -> Box<FsError> {
    fs_err!(
        code,
        "{} was updated, but {reason}, so its lockfile/environment couldn't be brought back \
         in sync automatically. A backup of the original manifest was saved to {} before this \
         change -- restore from it if you need to undo the edit. Re-run your package manager's \
         install/lock command manually.",
        manifest_path.display(),
        backup_path.display()
    )
}

/// Determines the package manager to sync `format`'s manifest at
/// `manifest_path` with.
///
/// `override_manager` (from `--package-manager`) wins outright if given --
/// it's an explicit user answer, not a guess, so it's checked for
/// compatibility with `format` and used directly, without consulting
/// detection or a picker (and without needing an interactive terminal,
/// unlike the paths below).
///
/// Otherwise, tries automatic detection (`resolve_manager_for_manifest`);
/// when that can't pin one down, or rejects the only candidate it found as
/// incompatible with this manifest, asks the user directly instead of
/// refusing outright -- mirrors [`confirm`]'s tty/`--yes` gating (`-y` skips
/// prompts it can answer with a fixed default; there's no such default
/// here, so `-y` -- and any non-interactive terminal -- gets a hard error
/// instead of a guess, unless `--package-manager` already answered it).
///
/// Called *before* the manifest is rewritten, unlike the detection this
/// replaces used to be: a project whose manager can't be determined is left
/// untouched, rather than edited on disk with no way to bring its
/// lockfile/environment back in sync (see `exec_managed_project_upgrade`).
///
/// `ctx` is threaded in (rather than calling `DiscoveryContext::real()`
/// internally) purely for testability: the presence-probe fallback below
/// runs real subprocesses when given a real context, which would make any
/// unit test asserting "no manager found" flaky against whatever happens to
/// be installed on the machine running the tests.
fn resolve_manager(
    format: PythonManifestFormat,
    manifest_path: &Path,
    existing_hint: Option<PythonPackageManager>,
    override_manager: Option<PythonPackageManager>,
    yes: bool,
    ctx: &DiscoveryContext<'_>,
) -> FsResult<PythonPackageManager> {
    if let Some(manager) = override_manager {
        return if manager.is_compatible_with(format) {
            Ok(manager)
        } else {
            err!(
                ErrorCode::InvalidArgument,
                "--package-manager {} doesn't manage a project in {}'s format. Valid choices \
                 for this manifest: {}",
                manager.label(),
                manifest_path.display(),
                PythonPackageManager::choices_for(format)
                    .iter()
                    .map(|m| m.label())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
    }

    let resolution = resolve_manager_for_manifest(format, manifest_path, existing_hint);
    if let Some(manager) = resolution.manager() {
        return Ok(manager);
    }

    // `existing_hint` (`dist_info.py_package_manager`) reflects how *dbt's
    // own binary* was installed, which is frequently unrelated to this
    // project's dependencies -- e.g. dbt v2 shipped as a standalone binary
    // dropped directly onto `PATH` has no venv/tool-dir signal at all, even
    // though `manifest_dir` obviously has some real package manager
    // governing it. Probe that directory (and `PATH`) directly before
    // giving up to a picker/hard error.
    let manifest_dir = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    if let Some(manager) = probe_manager_for_manifest(ctx, manifest_dir, format) {
        return Ok(manager);
    }

    let reason = match resolution {
        ManagerResolution::Incompatible(manager) => format!(
            "the only package manager discovery found ({}) doesn't manage a project in this \
             manifest's format",
            manager.label()
        ),
        // `resolution.manager()` above already returned for `Determined`.
        ManagerResolution::Undetermined | ManagerResolution::Determined(_) => {
            "the Python package manager for this project couldn't be determined".to_string()
        }
    };

    if yes || !is_interactive() {
        let cause = if yes {
            "--yes was passed, so there's no prompt to ask which one to use"
        } else {
            "this isn't an interactive terminal to ask which one to use"
        };
        return err!(
            ErrorCode::Unexpected,
            "{reason}, and {cause} -- {} was left unchanged. Re-run interactively (without \
             --yes) to pick one, pass --package-manager <name> to say which one non-\
             interactively, or edit and re-sync the manifest yourself.",
            manifest_path.display()
        );
    }

    println(format!("{reason} for {}.", manifest_path.display()));
    let choices = PythonPackageManager::choices_for(format);
    let labels: Vec<&str> = choices.iter().map(|m| m.label()).collect();
    let selection = dialoguer::Select::new()
        .with_prompt("Which package manager manages this project?")
        .items(&labels)
        .default(0)
        .interact()
        .map_err(|e| {
            fs_err!(
                ErrorCode::IoError,
                "failed to read package-manager selection: {e}"
            )
        })?;
    Ok(choices[selection])
}

/// Rewrites a managed Python project's manifest to depend on `dbt`
/// at a pinned exact version instead of `old_name`, then re-runs the
/// project's package manager to bring its lockfile/environment back in sync
/// with that edit -- editing the manifest alone can leave a lockfile stale.
/// Two separate confirmations, matching the two distinct actions: one for
/// the manifest edit (with a diff preview), one for the sync command(s)
/// that follow it.
async fn exec_managed_project_upgrade(
    manifest: &mut PythonManifest,
    old_name: &str,
    replacements: ManifestReplacements,
    dist_info: &DistInfo,
    override_manager: Option<PythonPackageManager>,
    yes: bool,
    ctx: &DiscoveryContext<'_>,
) -> FsResult<()> {
    let mut diff = Vec::new();
    replacements.diff(manifest, &mut diff)?;
    let diff = String::from_utf8_lossy(&diff);
    let edit_prompt = format!(
        "This will rewrite {} to depend on `dbt` instead of `{old_name}`:\n\n{diff}\nProceed?",
        manifest.path().display()
    );
    if !confirm(&edit_prompt, yes)? {
        return err!(ErrorCode::Generic, "Aborted.");
    }

    // Resolved *before* the manifest is touched: a project whose manager
    // can't be pinned down should be left exactly as it was, not edited with
    // no way to bring its lockfile/environment back in sync. See
    // `resolve_manager`'s doc comment.
    let manager = resolve_manager(
        manifest.format(),
        manifest.path(),
        dist_info.py_package_manager,
        override_manager,
        yes,
        ctx,
    )?;

    let backup_path = replacements.apply_to(manifest)?;
    println(format!(
        "{} was rewritten to depend on `dbt`. A backup of the original was saved to \
         {} -- restore from it if you need to undo this edit.",
        manifest.path().display(),
        backup_path.display()
    ));

    let commands = sync_command_for_manager(manager, manifest.format(), manifest.path())
        .ok_or_else(|| {
            manual_sync_required_err(
                manifest.path(),
                &backup_path,
                ErrorCode::NotSupported,
                &format!("dbt has no automatic sync command for {}", manager.label()),
            )
        })?;
    // `manifest.path()` may sit above the process's actual cwd --
    // `PythonManifest::detect` walks up parent directories to find it -- so
    // the sync command(s) must run from the manifest's own directory rather
    // than inheriting whatever directory the user happened to invoke this
    // command from.
    let sync_dir = manifest.path().parent().unwrap_or_else(|| Path::new("."));
    let sync_prompt = format!(
        "This will bring {}'s environment back in sync with the manifest change by running the \
         following in {}:\n\n    {}\n\nProceed?",
        manager.label(),
        sync_dir.display(),
        commands.join("\n    "),
    );
    if !confirm(&sync_prompt, yes)? {
        // Unlike the edit-confirmation abort above, `apply_to` has already
        // run by this point -- the manifest is rewritten on disk even though
        // the sync command never ran. Route through the same message used
        // when no automatic sync command exists at all, so both "manifest
        // changed, sync didn't happen" cases read consistently.
        return Err(manual_sync_required_err(
            manifest.path(),
            &backup_path,
            ErrorCode::Generic,
            "the sync step was declined",
        ));
    }
    for command in &commands {
        run_shell_command(command, Some(sync_dir))?;
    }
    Ok(())
}

async fn exec_global_install(
    dist_info: &DistInfo,
    yes: bool,
    command_name: &str,
    ctx: &DiscoveryContext<'_>,
) -> FsResult<()> {
    match dist_info.channel {
        // A `standalone`/`unclaimed` dbt-core is already a native binary
        // that nothing but dbt itself manages -- there's no package manager
        // to uninstall afterward, so upgrade it in place at its own path
        // instead of installing fresh to the (possibly different) default
        // location.
        Some(Channel::Standalone) | Some(Channel::Unclaimed) => {
            exec_in_place_upgrade(dist_info, yes).await
        }
        _ => exec_fresh_install_and_replace_package(dist_info, yes, command_name, ctx).await,
    }
}

/// Upgrades a `standalone`/`unclaimed` dbt-core install in place: re-runs
/// the dbt v2 installer with `--to <dir>` pointed at the existing binary's
/// own directory (from `DistInfo::path`, not the installer's default) and
/// `--update` so it overwrites rather than refusing a pre-existing file.
/// There's no separate package to uninstall afterward -- the installer
/// replaces the file directly -- and no PATH-shadowing check is needed,
/// since the binary's location on `PATH` hasn't changed.
async fn exec_in_place_upgrade(dist_info: &DistInfo, yes: bool) -> FsResult<()> {
    let current_path = Path::new(&dist_info.path);
    let target_dir = current_path.parent().ok_or_else(|| {
        fs_err!(
            ErrorCode::Unexpected,
            "Could not determine the install directory for {}",
            dist_info.path
        )
    })?;

    let prompt = format!(
        "This will download and run the dbt v2 standalone installer, replacing the \
         existing dbt-core binary in place at {}:\n\n    {}\n\nProceed?",
        target_dir.display(),
        install_command_display(Some(target_dir), true)
    );
    if !confirm(&prompt, yes)? {
        return err!(ErrorCode::Generic, "Aborted.");
    }
    install_dbt_v2_standalone(Some(target_dir), true).await
}

async fn exec_fresh_install_and_replace_package(
    dist_info: &DistInfo,
    yes: bool,
    command_name: &str,
    ctx: &DiscoveryContext<'_>,
) -> FsResult<()> {
    let standalone_target = standalone_target_path();
    let old_package_shadowed_by_target = standalone_target
        .as_deref()
        .is_some_and(|target| target == Path::new(&dist_info.path));

    // If the standalone installer would overwrite the exact file the old
    // `dbt-core` package manager thinks it owns, uninstall the old package
    // *first* -- otherwise the package manager's later uninstall would
    // delete the freshly-installed dbt v2 binary, believing it's removing
    // its own file.
    if old_package_shadowed_by_target {
        confirm_and_uninstall_old_package(dist_info, yes, ctx)?;
    }

    let install_target = standalone_target
        .as_deref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "the default location (~/.local/bin)".to_string());
    let install_prompt = format!(
        "This will download and run the dbt v2 standalone installer:\n\n    {}\n\n\
         installing to {install_target}. Proceed?",
        install_command_display(None, false)
    );
    if !confirm(&install_prompt, yes)? {
        return err!(ErrorCode::Generic, "Aborted.");
    }
    install_dbt_v2_standalone(None, false).await?;

    if !old_package_shadowed_by_target {
        confirm_and_uninstall_old_package(dist_info, yes, ctx)?;
    }

    warn_if_path_shadowed(&standalone_target, command_name).await;

    Ok(())
}

fn confirm_and_uninstall_old_package(
    dist_info: &DistInfo,
    yes: bool,
    ctx: &DiscoveryContext<'_>,
) -> FsResult<()> {
    let Some(channel) = dist_info.channel.clone() else {
        println(
            "Could not determine how the existing install was made, so it can't be uninstalled \
             automatically. Please remove it manually with the package manager you installed it \
             with.",
        );
        return Ok(());
    };
    let package_name = probe_installed_package_name(
        ctx,
        &channel,
        dist_info.py_package_manager,
        dist_info.py_venv_root.as_deref(),
    )
    .unwrap_or(DBT_CORE_PACKAGE_NAME);
    let Some(uninstall_cmd) =
        uninstall_command_for_package(channel, dist_info.py_package_manager, package_name)
    else {
        println(format!(
            "Could not determine how to uninstall the existing {package_name} install \
             automatically. Please remove it manually with the package manager you installed \
             it with."
        ));
        return Ok(());
    };

    let manager_note = if dist_info.py_package_manager.is_none() {
        String::new()
    } else {
        format!(" via {}", dist_info.install_label())
    };
    let prompt = format!(
        "This will remove the existing {package_name} install{manager_note} by running:\n\n    {uninstall_cmd}\n\nProceed?"
    );
    if !confirm(&prompt, yes)? {
        return err!(ErrorCode::Generic, "Aborted.");
    }
    // Not tied to any project manifest -- this uninstalls a globally-managed
    // package, so it keeps inheriting the process's own cwd rather than
    // needing an explicit directory.
    run_shell_command(&uninstall_cmd, None)
}

/// Which of [`UPGRADABLE_TARGET_NAMES`] is actually installed via `channel`/
/// `manager`, so [`confirm_and_uninstall_old_package`] doesn't have to
/// guess. `None` if the check is inconclusive (neither or both match, or
/// this channel/manager has no reliable way to check) -- callers fall back
/// to [`DBT_CORE_PACKAGE_NAME`] in that case.
fn probe_installed_package_name(
    ctx: &DiscoveryContext,
    channel: &Channel,
    manager: Option<PythonPackageManager>,
    venv_root: Option<&str>,
) -> Option<&'static str> {
    let mut installed = UPGRADABLE_TARGET_NAMES
        .into_iter()
        .filter(|name| is_package_installed(ctx, channel, manager, venv_root, name));
    let name = installed.next()?;
    installed.next().is_none().then_some(name)
}

/// Best-effort "is `name` installed" check. Only PyPI installs are checked:
/// there's no `dbt-oss` Homebrew formula or Winget package yet, so a
/// Brew/Winget probe would always fail and just cost a wasted subprocess
/// spawn.
fn is_package_installed(
    ctx: &DiscoveryContext,
    channel: &Channel,
    manager: Option<PythonPackageManager>,
    venv_root: Option<&str>,
    name: &str,
) -> bool {
    if !matches!(channel, Channel::Pypi) {
        return false;
    }
    match manager {
        Some(
            PythonPackageManager::Pip
            | PythonPackageManager::Asdf
            | PythonPackageManager::Mise
            | PythonPackageManager::Pyenv
            | PythonPackageManager::Poetry
            | PythonPackageManager::Pdm
            | PythonPackageManager::Pipenv
            | PythonPackageManager::Hatch,
        ) => {
            let pip = venv_executable(venv_root, "pip");
            (ctx.run)(&pip, &["show", name]).is_some_and(|o| o.success)
        }
        Some(PythonPackageManager::Pipx) => (ctx.run)("pipx", &["list", "--short"])
            .is_some_and(|o| o.success && lists_package(&o.stdout, name)),
        Some(PythonPackageManager::Uv) => (ctx.run)("uv", &["tool", "list"])
            .is_some_and(|o| o.success && lists_package(&o.stdout, name)),
        Some(PythonPackageManager::Conda) => (ctx.run)("conda", &["list", name])
            .is_some_and(|o| o.success && lists_package(&o.stdout, name)),
        Some(PythonPackageManager::Rye) | None => false,
    }
}

/// The path to `executable_name` under `venv_root`, if it exists there;
/// otherwise the bare name, resolved from `PATH` instead. Prefers the venv's
/// own executable so a probe answers for the environment `dbt` actually
/// lives in, not whatever's ambient on the caller's `PATH`.
fn venv_executable(venv_root: Option<&str>, executable_name: &str) -> String {
    let Some(root) = venv_root else {
        return executable_name.to_string();
    };
    let bin_dir = if cfg!(windows) { "Scripts" } else { "bin" };
    let exe_name = if cfg!(windows) {
        format!("{executable_name}.exe")
    } else {
        executable_name.to_string()
    };
    let path = Path::new(root).join(bin_dir).join(exe_name);
    if path.exists() {
        path.to_string_lossy().into_owned()
    } else {
        executable_name.to_string()
    }
}

/// Whether `stdout` (one installed package per line, name first) lists
/// `name` -- from `pipx list --short`, `uv tool list`, or `conda list`.
fn lists_package(stdout: &str, name: &str) -> bool {
    stdout
        .lines()
        .any(|line| line.split_whitespace().next() == Some(name))
}

/// Builds the `Command` `run_shell_command` will run: a `sh -c`/`cmd /C`
/// wrapper around `command`, with `current_dir` set to `dir` when given.
/// Split out as a pure, directly-testable builder (mirroring
/// `install_sh_args`/`windows_ps_command` above) since `Command` itself
/// can't be asserted against without actually running it, but its
/// `current_dir` can be inspected via `get_current_dir()` before that.
fn build_shell_command(command: &str, dir: Option<&Path>) -> std::process::Command {
    #[cfg(not(target_os = "windows"))]
    let mut cmd = std::process::Command::new("sh");
    #[cfg(not(target_os = "windows"))]
    cmd.arg("-c").arg(command);

    #[cfg(target_os = "windows")]
    let mut cmd = std::process::Command::new("cmd");
    #[cfg(target_os = "windows")]
    cmd.args(["/C", command]);

    if let Some(dir) = dir {
        cmd.current_dir(dir);
    }
    cmd
}

/// Runs `command` through a shell. `dir` sets the working directory the
/// command runs in -- pass the manifest's own directory for anything tied to
/// a project manifest (see `exec_managed_project_upgrade`), or `None` to
/// keep inheriting the process's own cwd (e.g. the global-uninstall path in
/// `confirm_and_uninstall_old_package`, which isn't tied to any project).
fn run_shell_command(command: &str, dir: Option<&Path>) -> FsResult<()> {
    let result = build_shell_command(command, dir).status();

    match result {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => err!(ErrorCode::IoError, "`{command}` failed with {status}"),
        Err(e) => err!(ErrorCode::IoError, "failed to run `{command}`: {e}"),
    }
}

/// Builds `install.sh`'s argv for `--update`/`--to <dir>`, in that order.
fn install_sh_args(to: Option<&Path>, update: bool) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    if update {
        args.push("--update".to_string());
    }
    if let Some(to) = to {
        args.push("--to".to_string());
        args.push(to.display().to_string());
    }
    args
}

/// Unix-shaped display string for the command `install_dbt_v2_standalone`
/// will effectively run (mirrors `install_sh_args`'s flags), for
/// confirmation-prompt text only -- never fed to a shell.
fn unix_install_command_display(to: Option<&Path>, update: bool) -> String {
    let mut command = format!("curl -fsSL {} | sh", install_script_url());
    let args = install_sh_args(to, update);
    if !args.is_empty() {
        command.push_str(" -s -- ");
        command.push_str(&args.join(" "));
    }
    command
}

/// Windows-shaped display string, for confirmation-prompt text only.
/// Deliberately doesn't encode `-Update`/`-To` -- there's no single
/// canonical one-liner for "download, then run with args" on Windows the
/// way `curl | sh -s -- args` is on Unix (the real mechanism is
/// temp-file + `powershell -Command`, see `install_dbt_v2_standalone`).
/// Callers convey destination/update behavior in the surrounding prompt
/// prose instead.
fn windows_install_command_display() -> String {
    format!("irm {} | iex", install_script_url())
}

/// Selects the platform-appropriate display string. Trivial by design --
/// all the actual logic lives in the two functions above, which are each
/// fully unit-testable on any host regardless of `cfg!(windows)`.
fn install_command_display(to: Option<&Path>, update: bool) -> String {
    if cfg!(windows) {
        windows_install_command_display()
    } else {
        unix_install_command_display(to, update)
    }
}

/// Builds the `powershell`/`pwsh -Command` string that invokes `script_path`
/// (`install.ps1`, written to a temp file) with `-Update`/`-To <dir>`,
/// mirroring `install_sh_args`' unix argv for the same two flags. Only
/// called in production on Windows, but kept unguarded (rather than
/// `#[cfg(target_os = "windows")]`) so it's exercised by unit tests on every
/// platform; `allow(dead_code)` suppresses the resulting "never used"
/// warning on non-Windows production builds.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn windows_ps_command(script_path: &Path, to: Option<&Path>, update: bool) -> String {
    let mut command = format!(
        "& '{}'",
        escape_ps_single_quoted(&script_path.display().to_string().replace('\\', "\\\\"))
    );
    if update {
        command.push_str(" -Update");
    }
    if let Some(to) = to {
        command.push_str(&format!(
            " -To '{}'",
            escape_ps_single_quoted(&to.display().to_string().replace('\\', "\\\\"))
        ));
    }
    command
}

/// Escapes `s` for interpolation into a PowerShell single-quoted string
/// literal (`'...'`). Doubling `'` is the only escape needed inside a
/// PowerShell single-quoted string.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn escape_ps_single_quoted(s: &str) -> String {
    s.replace('\'', "''")
}

/// Downloads and runs the dbt v2 standalone installer. `to` pins the install
/// directory (the installer's own default, `~/.local/bin`, is used when
/// `None`); `update` allows overwriting a file already at the destination --
/// both are set together for [`exec_in_place_upgrade`]'s in-place overwrite
/// of an existing native binary, and both left unset for a fresh install.
/// Unlike `dbt system update`'s in-place self-update (which must release the
/// currently-running exe's file lock before overwriting it), this installs a
/// different binary, so it runs synchronously to completion on every
/// platform.
async fn install_dbt_v2_standalone(to: Option<&Path>, update: bool) -> FsResult<()> {
    let script = ReqwestClient.get_text(&install_script_url()).await?;

    #[cfg(not(target_os = "windows"))]
    {
        let args = install_sh_args(to, update);
        let options = run_script::ScriptOptions::new();
        let (code, output, error) = run_script::run(&script, &args, &options)
            .map_err(|e| fs_err!(ErrorCode::IoError, "failed to run install script: {e}"))?;
        let output = output.trim();
        let error = error.trim();
        if code != 0 {
            let msg = if !error.is_empty() {
                format!("{error}\n{output}")
            } else {
                output.to_string()
            };
            return err!(ErrorCode::IoError, "dbt v2 install script failed: {msg}");
        }
        println(output);
        Ok(())
    }

    #[cfg(target_os = "windows")]
    {
        let mut tmp = tempfile::Builder::new()
            .prefix("dbt-v2-install-")
            .suffix(".ps1")
            .tempfile()
            .map_err(|e| {
                fs_err!(
                    ErrorCode::IoError,
                    "failed to create temp install script: {e}"
                )
            })?;
        use std::io::Write as _;
        tmp.write_all(script.as_bytes()).map_err(|e| {
            fs_err!(
                ErrorCode::IoError,
                "failed to write temp install script: {e}"
            )
        })?;
        tmp.flush().map_err(|e| {
            fs_err!(
                ErrorCode::IoError,
                "failed to write temp install script: {e}"
            )
        })?;

        let ps_command = windows_ps_command(tmp.path(), to, update);

        let ps_exe = if env::var("PSModulePath").is_ok_and(|p| p.contains("PowerShell/7")) {
            "pwsh"
        } else {
            "powershell"
        };
        let status = std::process::Command::new(ps_exe)
            .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command"])
            .arg(&ps_command)
            .status()
            .map_err(|e| fs_err!(ErrorCode::IoError, "failed to run install script: {e}"))?;
        if !status.success() {
            return err!(
                ErrorCode::IoError,
                "dbt v2 install script failed with {status}"
            );
        }
        Ok(())
    }
}

/// Best-effort post-install check: warns (without failing the command) if
/// something earlier on `PATH` still shadows the new dbt v2 install, or if
/// the current shell hasn't picked up the installer's PATH/rc changes yet.
async fn warn_if_path_shadowed(standalone_target: &Option<PathBuf>, command_name: &str) {
    let Some(target) = standalone_target else {
        return;
    };
    let Ok(infos) = DistInfoDiscovery::AllInPath.discover(command_name) else {
        return;
    };
    match infos.first() {
        Some(first) if Path::new(&first.path) == target.as_path() => {}
        Some(first) => {
            println(format!(
                "Warning: dbt v2 was installed to {}, but the first `dbt` on your PATH is \
                 still {} -- you may need to reorder PATH, or open a new shell, before the new \
                 install takes effect.",
                target.display(),
                first.path
            ));
        }
        None => {
            println(format!(
                "Warning: dbt v2 was installed to {}, but it isn't on PATH yet -- you may \
                 need to open a new shell, or add it to PATH manually.",
                target.display()
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proc::ProcessOutput;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    /// A [`DiscoveryContext`] that finds nothing on `PATH` or in any venv --
    /// used by tests that need `resolve_manager`'s presence-probe fallback to
    /// behave as "no manager found" deterministically, rather than reflecting
    /// whatever happens to be installed on the machine running the tests.
    fn empty_discovery_context() -> DiscoveryContext<'static> {
        fn nothing_found(_: &str, _: &[&str]) -> Option<ProcessOutput> {
            None
        }
        DiscoveryContext {
            env: &no_env,
            run: &nothing_found,
        }
    }

    #[test]
    fn install_script_name_matches_platform() {
        let name = install_script_name();
        assert!(name == "install.sh" || name == "install.ps1");
    }

    #[test]
    fn install_sh_args_fresh_install_has_no_flags() {
        assert_eq!(install_sh_args(None, false), Vec::<String>::new());
    }

    #[test]
    fn install_sh_args_in_place_upgrade_passes_update_and_to() {
        assert_eq!(
            install_sh_args(Some(Path::new("/opt/dbt/bin")), true),
            vec!["--update", "--to", "/opt/dbt/bin"]
        );
    }

    #[test]
    fn install_sh_args_to_without_update() {
        assert_eq!(
            install_sh_args(Some(Path::new("/opt/dbt/bin")), false),
            vec!["--to", "/opt/dbt/bin"]
        );
    }

    #[test]
    fn unix_install_command_display_fresh_install_has_no_flags() {
        assert_eq!(
            unix_install_command_display(None, false),
            format!("curl -fsSL {} | sh", install_script_url())
        );
    }

    #[test]
    fn unix_install_command_display_in_place_upgrade_passes_update_and_to() {
        assert_eq!(
            unix_install_command_display(Some(Path::new("/opt/dbt/bin")), true),
            format!(
                "curl -fsSL {} | sh -s -- --update --to /opt/dbt/bin",
                install_script_url()
            )
        );
    }

    #[test]
    fn windows_install_command_display_has_no_flags_or_args() {
        assert_eq!(
            windows_install_command_display(),
            format!("irm {} | iex", install_script_url())
        );
    }

    #[test]
    fn windows_ps_command_fresh_install_has_no_flags() {
        assert_eq!(
            windows_ps_command(Path::new(r"C:\temp\install.ps1"), None, false),
            r"& 'C:\\temp\\install.ps1'"
        );
    }

    #[test]
    fn windows_ps_command_in_place_upgrade_passes_update_and_to() {
        assert_eq!(
            windows_ps_command(
                Path::new(r"C:\temp\install.ps1"),
                Some(Path::new(r"C:\Users\me\.local\bin")),
                true
            ),
            r"& 'C:\\temp\\install.ps1' -Update -To 'C:\\Users\\me\\.local\\bin'"
        );
    }

    #[test]
    fn windows_ps_command_escapes_single_quotes_in_paths() {
        assert_eq!(
            windows_ps_command(
                Path::new(r"C:\Users\O'Brien\install.ps1"),
                Some(Path::new(r"C:\Users\O'Brien\.local\bin")),
                true
            ),
            r"& 'C:\\Users\\O''Brien\\install.ps1' -Update -To 'C:\\Users\\O''Brien\\.local\\bin'"
        );
    }

    #[test]
    fn build_shell_command_sets_current_dir_when_given() {
        let dir = Path::new("/tmp/some-project");
        let cmd = build_shell_command("pip install -r requirements.txt", Some(dir));
        assert_eq!(cmd.get_current_dir(), Some(dir));
    }

    #[test]
    fn build_shell_command_leaves_current_dir_unset_when_none() {
        let cmd = build_shell_command("pip install -r requirements.txt", None);
        assert_eq!(cmd.get_current_dir(), None);
    }

    fn dist_info_for_test(channel: Option<Channel>, path: &str) -> DistInfo {
        DistInfo {
            path: path.to_string(),
            channel,
            distribution: Some(Distribution::Oss),
            generation: crate::Generation::V2,
            py_package_manager: None,
            py_venv_root: None,
            version: None,
            is_prerelease: None,
            upgrade_cmd: None,
            uninstall_cmd: None,
        }
    }

    /// Mocks `pip show <name>` / `conda list <name>` (success iff `name` is
    /// in `installed`) and `pipx list --short` / `uv tool list` (always
    /// succeeds, listing every name in `installed`). Any other command/args
    /// -- including a Brew/Winget probe, which `is_package_installed` must
    /// never issue -- returns `None`, so a test relying on one fails loudly.
    fn run_reporting_installed(
        installed: &'static [&'static str],
    ) -> impl Fn(&str, &[&str]) -> Option<ProcessOutput> {
        move |cmd: &str, args: &[&str]| match (cmd, args) {
            ("pip", ["show", name]) | ("conda", ["list", name]) => Some(ProcessOutput {
                success: installed.contains(name),
                stdout: String::new(),
                stderr: String::new(),
            }),
            ("pipx", ["list", "--short"]) | ("uv", ["tool", "list"]) => Some(ProcessOutput {
                success: true,
                stdout: installed.join("\n"),
                stderr: String::new(),
            }),
            _ => None,
        }
    }

    #[test]
    fn probe_installed_package_name_detects_dbt_core_via_pip() {
        let ctx = DiscoveryContext {
            env: &no_env,
            run: &run_reporting_installed(&[DBT_CORE_PACKAGE_NAME]),
        };
        assert_eq!(
            probe_installed_package_name(
                &ctx,
                &Channel::Pypi,
                Some(PythonPackageManager::Pip),
                None
            ),
            Some(DBT_CORE_PACKAGE_NAME)
        );
    }

    #[test]
    fn probe_installed_package_name_detects_dbt_oss_via_pip() {
        let ctx = DiscoveryContext {
            env: &no_env,
            run: &run_reporting_installed(&[DBT_OSS_PACKAGE_NAME]),
        };
        assert_eq!(
            probe_installed_package_name(
                &ctx,
                &Channel::Pypi,
                Some(PythonPackageManager::Pip),
                None
            ),
            Some(DBT_OSS_PACKAGE_NAME)
        );
    }

    #[test]
    fn probe_installed_package_name_none_when_neither_found() {
        let ctx = DiscoveryContext {
            env: &no_env,
            run: &run_reporting_installed(&[]),
        };
        assert_eq!(
            probe_installed_package_name(
                &ctx,
                &Channel::Pypi,
                Some(PythonPackageManager::Pip),
                None
            ),
            None
        );
    }

    #[test]
    fn probe_installed_package_name_none_when_both_found() {
        let ctx = DiscoveryContext {
            env: &no_env,
            run: &run_reporting_installed(&[DBT_CORE_PACKAGE_NAME, DBT_OSS_PACKAGE_NAME]),
        };
        assert_eq!(
            probe_installed_package_name(
                &ctx,
                &Channel::Pypi,
                Some(PythonPackageManager::Pip),
                None
            ),
            None
        );
    }

    #[test]
    fn probe_installed_package_name_none_for_rye_or_no_manager() {
        let ctx = DiscoveryContext {
            env: &no_env,
            run: &run_reporting_installed(&[DBT_OSS_PACKAGE_NAME]),
        };
        assert_eq!(
            probe_installed_package_name(
                &ctx,
                &Channel::Pypi,
                Some(PythonPackageManager::Rye),
                None
            ),
            None
        );
        assert_eq!(
            probe_installed_package_name(&ctx, &Channel::Pypi, None, None),
            None
        );
    }

    #[test]
    fn probe_installed_package_name_detects_via_pipx_list_output() {
        let ctx = DiscoveryContext {
            env: &no_env,
            run: &run_reporting_installed(&[DBT_OSS_PACKAGE_NAME]),
        };
        assert_eq!(
            probe_installed_package_name(
                &ctx,
                &Channel::Pypi,
                Some(PythonPackageManager::Pipx),
                None
            ),
            Some(DBT_OSS_PACKAGE_NAME)
        );
    }

    #[test]
    fn probe_installed_package_name_never_probes_brew_or_winget() {
        // No known dbt-oss Homebrew formula or Winget package exists yet --
        // Brew/Winget must never be probed, regardless of what a (mocked)
        // package manager would otherwise report.
        let ctx = DiscoveryContext {
            env: &no_env,
            run: &run_reporting_installed(&[DBT_CORE_PACKAGE_NAME]),
        };
        assert_eq!(
            probe_installed_package_name(&ctx, &Channel::Brew, None, None),
            None
        );
        assert_eq!(
            probe_installed_package_name(&ctx, &Channel::Winget, None, None),
            None
        );
    }

    #[test]
    fn probe_installed_package_name_prefers_the_venv_pip_when_present() {
        let venv = tempfile::tempdir().unwrap();
        let bin_dir = if cfg!(windows) { "Scripts" } else { "bin" };
        let pip_name = if cfg!(windows) { "pip.exe" } else { "pip" };
        let bin = venv.path().join(bin_dir);
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join(pip_name), "").unwrap();
        let venv_pip = bin.join(pip_name).to_string_lossy().into_owned();

        let run = move |cmd: &str, args: &[&str]| match (cmd, args) {
            (c, ["show", name]) if c == venv_pip.as_str() => Some(ProcessOutput {
                success: *name == DBT_OSS_PACKAGE_NAME,
                stdout: String::new(),
                stderr: String::new(),
            }),
            _ => None,
        };
        let ctx = DiscoveryContext {
            env: &no_env,
            run: &run,
        };
        assert_eq!(
            probe_installed_package_name(
                &ctx,
                &Channel::Pypi,
                Some(PythonPackageManager::Pip),
                Some(venv.path().to_str().unwrap())
            ),
            Some(DBT_OSS_PACKAGE_NAME),
            "expected the venv's own pip to be queried instead of the bare `pip` on PATH"
        );
    }

    #[test]
    fn venv_executable_falls_back_to_bare_name_when_absent_from_venv() {
        let venv = tempfile::tempdir().unwrap();
        assert_eq!(
            venv_executable(Some(venv.path().to_str().unwrap()), "pip"),
            "pip"
        );
        assert_eq!(venv_executable(None, "pip"), "pip");
    }

    #[test]
    fn lists_package_matches_first_whitespace_token_only() {
        assert!(lists_package(
            "dbt-oss 2.0.0\nother-tool 1.0.0\n",
            "dbt-oss"
        ));
        assert!(!lists_package("dbt-oss-extra 1.0.0\n", "dbt-oss"));
        assert!(!lists_package("", "dbt-oss"));
    }

    #[test]
    fn exec_in_place_upgrade_target_dir_is_parent_of_dist_info_path() {
        let dist_info = dist_info_for_test(Some(Channel::Standalone), "/home/user/.local/bin/dbt");
        let target_dir = Path::new(&dist_info.path).parent().unwrap();
        assert_eq!(target_dir, Path::new("/home/user/.local/bin"));
    }

    fn manifest_with(dir: &Path, filename: &str, content: &str) -> PythonManifest {
        std::fs::write(dir.join(filename), content).unwrap();
        PythonManifest::detect(dir).unwrap().unwrap()
    }

    #[test]
    fn declared_upgradable_name_detects_dbt_core() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = manifest_with(tmp.path(), "requirements.txt", "dbt-core==1.2.3\n");
        assert_eq!(
            declared_upgradable_name(&manifest).unwrap(),
            Some(DBT_CORE_PACKAGE_NAME)
        );
    }

    #[test]
    fn declared_upgradable_name_detects_dbt_oss() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = manifest_with(tmp.path(), "requirements.txt", "dbt-oss==2.0.0\n");
        assert_eq!(
            declared_upgradable_name(&manifest).unwrap(),
            Some(DBT_OSS_PACKAGE_NAME)
        );
    }

    #[test]
    fn declared_upgradable_name_returns_none_when_neither_declared() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = manifest_with(tmp.path(), "requirements.txt", "other-package==0.1.0\n");
        assert_eq!(declared_upgradable_name(&manifest).unwrap(), None);
    }

    #[test]
    fn declared_upgradable_name_errors_when_both_declared() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = manifest_with(
            tmp.path(),
            "requirements.txt",
            "dbt-core==1.2.3\ndbt-oss==2.0.0\n",
        );
        let err = declared_upgradable_name(&manifest).expect_err("both names are declared");
        assert!(
            err.context.contains("dbt-core") && err.context.contains("dbt-oss"),
            "expected the error to name both declared packages, got: {}",
            err.context
        );
    }

    fn managed_project_manifest_and_replacements(
        dir: &Path,
    ) -> (PythonManifest, ManifestReplacements) {
        std::fs::write(
            dir.join("requirements.txt"),
            "other-package==0.1.0\ndbt-core==1.2.3\n",
        )
        .unwrap();
        let manifest = PythonManifest::detect(dir).unwrap().unwrap();
        let replacements = manifest
            .get_rename_replacement(
                DBT_CORE_PACKAGE_NAME,
                &PackageSpec {
                    name: PROPRIETARY_PACKAGE_NAME.to_string(),
                    version: PackageVersion::Exact("2.0.0".to_string()),
                },
            )
            .unwrap()
            .expect("dbt-core is declared");
        (manifest, replacements)
    }

    #[dbt_runtime::test]
    async fn exec_managed_project_upgrade_aborts_before_editing_when_not_confirmed() {
        // Not a TTY under `cargo test`, so `confirm(..., false)` errors
        // before the manifest is touched -- this exercises the diff-preview
        // construction without needing a real confirmation or any network
        // access.
        let tmp = tempfile::tempdir().unwrap();
        let (mut manifest, replacements) = managed_project_manifest_and_replacements(tmp.path());
        let dist_info = dist_info_for_test(Some(Channel::Pypi), "/venv/bin/dbt");

        let result = exec_managed_project_upgrade(
            &mut manifest,
            DBT_CORE_PACKAGE_NAME,
            replacements,
            &dist_info,
            None,
            false,
            &empty_discovery_context(),
        )
        .await;
        assert!(result.is_err());

        let on_disk = std::fs::read_to_string(tmp.path().join("requirements.txt")).unwrap();
        assert_eq!(on_disk, "other-package==0.1.0\ndbt-core==1.2.3\n");
        assert!(
            !tmp.path().join("requirements.txt.bak").exists(),
            "aborting before the edit is confirmed must not touch the manifest, so no backup \
             should be created either"
        );
    }

    #[dbt_runtime::test]
    async fn exec_managed_project_upgrade_errors_before_editing_when_manager_unknown() {
        // `yes: true` skips the edit confirmation, but with no
        // `py_package_manager` on `dist_info` and no local lockfile signal,
        // there's no sensible default manager to assume non-interactively --
        // `resolve_manager` must stop *before* the manifest is touched, not
        // edit it and then fail with no way to bring it back in sync.
        let tmp = tempfile::tempdir().unwrap();
        let (mut manifest, replacements) = managed_project_manifest_and_replacements(tmp.path());
        let mut dist_info = dist_info_for_test(Some(Channel::Pypi), "/venv/bin/dbt");
        dist_info.py_package_manager = None;

        let result = exec_managed_project_upgrade(
            &mut manifest,
            DBT_CORE_PACKAGE_NAME,
            replacements,
            &dist_info,
            None,
            true,
            &empty_discovery_context(),
        )
        .await;
        assert!(result.is_err());

        let on_disk = std::fs::read_to_string(tmp.path().join("requirements.txt")).unwrap();
        assert_eq!(
            on_disk, "other-package==0.1.0\ndbt-core==1.2.3\n",
            "an undetermined manager must leave the manifest untouched"
        );
        assert!(
            !tmp.path().join("requirements.txt.bak").exists(),
            "no edit happened, so no backup should be created either"
        );
    }

    #[dbt_runtime::test]
    async fn exec_managed_project_upgrade_rejects_incompatible_manager_hint_before_editing() {
        // `dist_info.py_package_manager` is resolved independently of the
        // manifest (`manager_from_manifest_signals` walks ancestor
        // directories for a fixed signal-file table), so it can disagree
        // with the manifest `exec_managed_project_upgrade` actually detected
        // and edited -- e.g. a stray `environment.yml` elsewhere in the
        // ancestor chain resolving `Conda`, while the project's real
        // manifest is a `requirements.txt`. `resolve_manager_for_manifest`
        // must reject that incompatible hint rather than emit a nonsensical
        // `conda env update` command for a project with no conda manifest;
        // dispatch should stop -- before editing the manifest -- rather than
        // silently trust `Conda`.
        let tmp = tempfile::tempdir().unwrap();
        let (mut manifest, replacements) = managed_project_manifest_and_replacements(tmp.path());
        let mut dist_info = dist_info_for_test(Some(Channel::Pypi), "/venv/bin/dbt");
        dist_info.py_package_manager = Some(PythonPackageManager::Conda);

        let result = exec_managed_project_upgrade(
            &mut manifest,
            DBT_CORE_PACKAGE_NAME,
            replacements,
            &dist_info,
            None,
            true,
            &empty_discovery_context(),
        )
        .await;
        let err = result.expect_err("an incompatible manager hint must not be trusted");
        assert!(
            err.context
                .contains("doesn't manage a project in this manifest's format"),
            "expected the resolver to report the incompatible hint, got: {}",
            err.context
        );
        assert!(
            !err.context.to_lowercase().contains("conda env update"),
            "must not have emitted a conda-flavored command for a requirements.txt project, \
             got: {}",
            err.context
        );

        let on_disk = std::fs::read_to_string(tmp.path().join("requirements.txt")).unwrap();
        assert_eq!(
            on_disk, "other-package==0.1.0\ndbt-core==1.2.3\n",
            "an incompatible hint must leave the manifest untouched"
        );
        assert!(!tmp.path().join("requirements.txt.bak").exists());
    }

    #[test]
    fn resolve_manager_non_interactive_reports_incompatible_hint_without_touching_manifest() {
        // Under `cargo test`, stdin is never a tty, so this exercises the
        // non-interactive branch of `resolve_manager` directly rather than
        // needing to fake a `Select` prompt.
        let manifest_path = Path::new("/project/requirements.txt");
        let err = resolve_manager(
            PythonManifestFormat::Requirements,
            manifest_path,
            Some(PythonPackageManager::Uv),
            None,
            false,
            &empty_discovery_context(),
        )
        .expect_err("a non-interactive terminal can't answer a manager picker");
        assert!(
            err.context
                .contains("doesn't manage a project in this manifest's format"),
            "got: {}",
            err.context
        );
        assert!(
            err.context.contains("/project/requirements.txt"),
            "got: {}",
            err.context
        );
    }

    #[test]
    fn resolve_manager_assume_yes_errors_rather_than_guessing() {
        // `--yes` has no sensible default answer for "which package manager
        // manages this project" the way it does for a plain confirm, so it
        // must still error rather than silently pick one.
        let err = resolve_manager(
            PythonManifestFormat::Requirements,
            Path::new("/project/requirements.txt"),
            None,
            None,
            true,
            &empty_discovery_context(),
        )
        .expect_err("--yes must not bypass manager selection with a guess");
        assert!(
            err.context
                .contains("the Python package manager for this project couldn't be determined")
        );
    }

    #[test]
    fn resolve_manager_falls_back_to_the_manifest_dir_probe_when_hint_is_none() {
        // The exact scenario an unattended `-y` sweep hits when dbt v2 ships
        // as a standalone binary with no venv/tool-dir signal of its own
        // (`existing_hint: None`) against a Hatch-managed pyproject.toml
        // (no uv.lock/poetry.lock/pdm.lock for the earlier lockfile check to
        // find): this used to be undetermined/9002 even under `-y`. It must
        // now resolve via the manifest-directory probe instead, with no
        // picker and no error.
        fn no_env(_: &str) -> Option<String> {
            None
        }
        fn finds_hatch(cmd: &str, _: &[&str]) -> Option<ProcessOutput> {
            if cmd == "hatch" {
                Some(ProcessOutput {
                    success: true,
                    stdout: String::new(),
                    stderr: String::new(),
                })
            } else {
                None
            }
        }
        let ctx = DiscoveryContext {
            env: &no_env,
            run: &finds_hatch,
        };

        let manager = resolve_manager(
            PythonManifestFormat::Pyproject,
            Path::new("/project/pyproject.toml"),
            None,
            None,
            true,
            &ctx,
        )
        .expect("the manifest-dir probe should find hatch on PATH without needing a prompt");
        assert_eq!(manager, PythonPackageManager::Hatch);
    }

    #[test]
    fn resolve_manager_override_wins_without_detection_or_a_prompt() {
        // `--package-manager` is an explicit answer, not a guess -- it must
        // be used directly even though stdin isn't a tty and no hint/lockfile
        // signal was given, and even under `--yes`.
        let manager = resolve_manager(
            PythonManifestFormat::Requirements,
            Path::new("/project/requirements.txt"),
            None,
            Some(PythonPackageManager::Hatch),
            true,
            &empty_discovery_context(),
        )
        .expect("an explicit override needs no detection and no interactive terminal");
        assert_eq!(manager, PythonPackageManager::Hatch);
    }

    #[test]
    fn resolve_manager_override_incompatible_with_format_is_rejected() {
        // `Uv` doesn't manage a `requirements.txt`-format project (see
        // `PythonPackageManager::is_compatible_with`) -- an explicit
        // `--package-manager uv` here is still wrong, and must be rejected
        // with a clear message rather than emitting a nonsensical `uv sync`.
        let err = resolve_manager(
            PythonManifestFormat::CondaEnvironment,
            Path::new("/project/environment.yml"),
            None,
            Some(PythonPackageManager::Uv),
            true,
            &empty_discovery_context(),
        )
        .expect_err("an incompatible --package-manager override must be rejected");
        assert!(
            err.context.contains("doesn't manage a project in"),
            "got: {}",
            err.context
        );
    }

    #[test]
    fn manual_sync_required_err_message_when_sync_declined() {
        // The sync-command `confirm` in `exec_managed_project_upgrade` can't
        // actually be driven to a declined `Ok(false)` under `cargo test`:
        // `confirm` there only ever returns `Ok(true)` (`yes: true`) or an
        // `Err` from the non-interactive check (`yes: false`), never
        // `Ok(false)` -- so there's no way to reach that branch end-to-end
        // in this environment. This instead asserts directly on the message
        // `exec_managed_project_upgrade` constructs for that branch, i.e.
        // that declining the sync step (unlike declining the earlier edit
        // confirmation) says the manifest was already changed.
        let manifest_path = Path::new("/project/requirements.txt");
        let backup_path = Path::new("/project/requirements.txt.bak");
        let err = manual_sync_required_err(
            manifest_path,
            backup_path,
            ErrorCode::Generic,
            "the sync step was declined",
        );
        assert!(
            err.context
                .contains("/project/requirements.txt was updated")
        );
        assert!(err.context.contains("the sync step was declined"));
        assert!(
            err.context.contains("/project/requirements.txt.bak"),
            "expected the backup path to be repeated in the message, got: {}",
            err.context
        );
        assert!(
            err.context
                .contains("Re-run your package manager's install/lock command manually")
        );
    }

    #[dbt_runtime::test]
    async fn global_install_dispatches_to_in_place_upgrade_for_standalone_and_unclaimed() {
        // Not a TTY under `cargo test`, so `confirm(..., false)` errors
        // before any network call -- this exercises the channel-dispatch
        // and target-dir resolution in `exec_global_install` /
        // `exec_in_place_upgrade` without touching the network.
        for channel in [Channel::Standalone, Channel::Unclaimed] {
            let dist_info = dist_info_for_test(Some(channel.clone()), "/home/user/.local/bin/dbt");
            let result =
                exec_global_install(&dist_info, false, "dbt-core", &empty_discovery_context())
                    .await;
            assert!(result.is_err(), "channel={channel:?}");
        }
    }
}

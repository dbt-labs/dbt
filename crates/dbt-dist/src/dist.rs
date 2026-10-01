use std::path::Path;

use crate::python::{PythonManifestFormat, PythonPackageManager};
use dbt_common::{ErrorCode, FsResult, fs_err};
pub use dbt_dist_classify::{Distribution, Generation};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Channel {
    Standalone,
    /// A native binary that no known package manager (Homebrew, Winget) or
    /// the standalone installer's canonical `~/.local/bin` claims — a dev
    /// build, or one placed somewhere non-standard. Treated the same as
    /// `Standalone` for self-update/uninstall purposes: nothing else owns
    /// it, so dbt may manage it directly.
    Unclaimed,
    Pypi,
    Brew,
    Winget,
    /// A native binary under a package manager we recognize by path but
    /// don't publish to (e.g. Scoop, Chocolatey) — unlike Homebrew/Winget/
    /// PyPI, we have no official package there, so there's no command we
    /// can vouch for. The `String` is the manager's display name, used only
    /// in messages (never matched on) — see `DistInfo::unsupported_channel_message`.
    Unsupported(String),
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DistInfo {
    pub path: String,
    pub channel: Option<Channel>,
    pub distribution: Option<Distribution>,
    pub generation: Generation,
    pub py_package_manager: Option<PythonPackageManager>,
    pub py_venv_root: Option<String>,
    /// The version found at `path`, when it could be determined.
    ///
    /// `#[serde(default)]`: a `dbt` probed via `internal
    /// get-distribution-info` (see "Resolving the release channel") may be a
    /// different, older build than the one parsing its output, and older
    /// builds may predate this field.
    #[serde(default)]
    pub version: Option<String>,
    /// Whether `version` is a pre-release. `None` when `version` itself
    /// couldn't be determined — never collapsed to `false`, since command
    /// generation treats "unknown" as "not a pre-release" (the safe
    /// default) but the field itself should stay honest about that.
    #[serde(default)]
    pub is_prerelease: Option<bool>,
    pub upgrade_cmd: Option<String>,
    pub uninstall_cmd: Option<String>,
}

impl DistInfo {
    /// Discovers [`DistInfo`] for the currently running process. Thin
    /// wrapper around [`crate::DistInfoDiscovery::Current`] for callers that
    /// just want a single value rather than a one-element `Vec`.
    pub fn current(command_name: &str) -> FsResult<Self> {
        crate::DistInfoDiscovery::Current
            .discover(command_name)?
            .pop()
            .ok_or_else(|| {
                fs_err!(
                    ErrorCode::Unexpected,
                    "distribution discovery returned no results"
                )
            })
    }

    /// Whether this install is one dbt manages itself (`dbt system update` /
    /// `dbt system uninstall`) rather than one owned by an external package
    /// manager.
    pub fn is_self_managed(&self) -> bool {
        matches!(
            self.channel,
            Some(Channel::Standalone) | Some(Channel::Unclaimed)
        )
    }

    /// The upgrade command to surface, pinned to `target_version` when the
    /// channel needs it. Winget's plain `upgrade` is unreliable for dbt's
    /// pre-release versions, so a known target version is pinned explicitly;
    /// every other channel's `upgrade_cmd` is already version-agnostic.
    pub fn upgrade_command_for_version(&self, target_version: Option<&str>) -> Option<String> {
        match (&self.channel, target_version) {
            (Some(Channel::Winget), Some(v)) => Some(format!(
                "winget install --id dbtLabs.dbt --exact --version {v}"
            )),
            _ => self.upgrade_cmd.clone(),
        }
    }

    /// Human-readable name of the tool/channel that owns this install, for
    /// error messages (e.g. "dbt was installed via {label}. To uninstall,
    /// run ...").
    pub fn install_label(&self) -> &'static str {
        match self.channel {
            Some(Channel::Standalone) | Some(Channel::Unclaimed) => "the standalone installer",
            Some(Channel::Brew) => "Homebrew",
            Some(Channel::Winget) => "winget",
            Some(Channel::Pypi) => self
                .py_package_manager
                .map_or("a Python package manager", PythonPackageManager::label),
            // Not expected to be reached in practice — callers check
            // `unsupported_channel_message` first, which has the manager's
            // actual name; this is only here so the match stays exhaustive.
            Some(Channel::Unsupported(_)) => "an unsupported package manager",
            None => "another package manager",
        }
    }

    /// For a recognized-but-unsupported manager (Scoop, Chocolatey, ...),
    /// the message telling the user to go through that manager instead —
    /// `action` is the verb ("update" or "uninstall"). `None` for every
    /// other channel.
    pub fn unsupported_channel_message(&self, action: &str) -> Option<String> {
        let Some(Channel::Unsupported(manager)) = &self.channel else {
            return None;
        };
        Some(format!(
            "dbt was not installed using an officially-supported channel. \
             Please {action} with {manager} and see \
             https://docs.getdbt.com/docs/local/install-dbt?version=2 \
             for installation instructions."
        ))
    }
}

/// Generates the uninstall command for a package that isn't necessarily the
/// currently-running distribution's own package — e.g. removing `dbt-core`
/// once a cross-distribution upgrade has installed `dbt` (v2) alongside
/// or in place of it. Mirrors the uninstall half of
/// [`crate::PathDiscovery::command_strings`]'s `Pypi` arm, but parameterized
/// by an explicit package name instead of the hardcoded `"dbt"`, and passes
/// `-y`/`--yes` wherever the underlying tool defaults to an interactive
/// confirmation prompt (`pip`, `hatch run pip`, `conda`) -- unlike
/// `command_strings`, this function's output is executed unattended via
/// `run_shell_command`, with no stdin for the prompt to read.
///
/// Deliberately separate from `command_strings`: that function backs
/// `dbt system update`/`uninstall`'s self-management of the *current*
/// package and must not change behavior for this unrelated use case.
pub fn uninstall_command_for_package(
    channel: Channel,
    manager: Option<PythonPackageManager>,
    package_name: &str,
) -> Option<String> {
    match channel {
        Channel::Standalone | Channel::Unclaimed => Some("dbt system uninstall".to_string()),
        Channel::Brew => Some(format!("brew uninstall {package_name}")),
        Channel::Winget => Some(format!(
            "winget uninstall --id dbtLabs.{package_name} --exact"
        )),
        Channel::Unsupported(_) => None,
        Channel::Pypi => {
            let manager = manager?;
            let command = match manager {
                PythonPackageManager::Pip
                | PythonPackageManager::Asdf
                | PythonPackageManager::Mise
                | PythonPackageManager::Pyenv => format!("pip uninstall -y {package_name}"),
                PythonPackageManager::Pipx => format!("pipx uninstall {package_name}"),
                PythonPackageManager::Uv => format!("uv tool uninstall {package_name}"),
                PythonPackageManager::Poetry => format!("poetry remove {package_name}"),
                PythonPackageManager::Pdm => format!("pdm remove {package_name}"),
                PythonPackageManager::Pipenv => format!("pipenv uninstall {package_name}"),
                PythonPackageManager::Hatch => {
                    format!("hatch run pip uninstall -y {package_name}")
                }
                PythonPackageManager::Conda => format!("conda remove -y {package_name}"),
                PythonPackageManager::Rye => format!("rye uninstall {package_name}"),
            };
            Some(command)
        }
    }
}

/// The outcome of [`resolve_manager_for_manifest`] -- distinct from a plain
/// `Option` because "nothing was discovered" and "something was discovered
/// but doesn't apply to this manifest" call for different error handling
/// upstream (see `exec_managed_project_upgrade` in `upgrade.rs`): the latter
/// has an offending manager to name in a message (and to exclude from an
/// interactive picker's premise that just showing a list will help), the
/// former doesn't.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagerResolution {
    /// A manager was determined and is compatible with this manifest's
    /// format.
    Determined(PythonPackageManager),
    /// A manager was discovered (via `existing_hint`), but it isn't one that
    /// [`PythonPackageManager::is_compatible_with`] this manifest's format --
    /// it was resolved for some other install/project and doesn't describe
    /// this one.
    Incompatible(PythonPackageManager),
    /// No package manager could be discovered at all.
    Undetermined,
}

impl ManagerResolution {
    /// The resolved manager, if any.
    pub fn manager(self) -> Option<PythonPackageManager> {
        match self {
            Self::Determined(m) => Some(m),
            Self::Incompatible(_) | Self::Undetermined => None,
        }
    }
}

/// Resolves the package manager for `manifest`, using the manifest itself
/// as the primary signal rather than trusting a manager resolved
/// independently of it (e.g. `DistInfo::py_package_manager`, which comes
/// from `manager_from_manifest_signals` walking `cwd`'s ancestors for a
/// fixed signal-file table that has no `pyproject.toml` entry and no
/// awareness of which manifest `PythonManifest::detect` actually picked --
/// see this module's `sync_command_for_manager` and the investigation
/// behind this function for the mismatch that motivated it).
///
/// - `CondaEnvironment`/`Pipfile` manifests deterministically imply their
///   own manager (`Conda`/`Pipenv`) -- no other manager can be
///   legitimately paired with them, so those are returned outright.
/// - `Pyproject` is ambiguous across several managers; disambiguate via a
///   lockfile *in the manifest's own directory* (not any other ancestor),
///   which is more precise than scanning arbitrary ancestors.
/// - Otherwise (no manifest-local lockfile, or a `Requirements`/`SetupCfg`
///   manifest, which have no lockfile signal of their own), fall back to
///   `existing_hint` -- but only if it's actually compatible with this
///   manifest's format; an incompatible hint was resolved for a different
///   file and doesn't describe this project.
pub fn resolve_manager_for_manifest(
    format: PythonManifestFormat,
    manifest_path: &Path,
    existing_hint: Option<PythonPackageManager>,
) -> ManagerResolution {
    match format {
        PythonManifestFormat::CondaEnvironment => {
            return ManagerResolution::Determined(PythonPackageManager::Conda);
        }
        PythonManifestFormat::Pipfile => {
            return ManagerResolution::Determined(PythonPackageManager::Pipenv);
        }
        _ => {}
    }
    let dir = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    if format == PythonManifestFormat::Pyproject {
        const LOCKFILES: [(&str, PythonPackageManager); 3] = [
            ("uv.lock", PythonPackageManager::Uv),
            ("poetry.lock", PythonPackageManager::Poetry),
            ("pdm.lock", PythonPackageManager::Pdm),
        ];
        if let Some(manager) = LOCKFILES
            .iter()
            .find_map(|(file, manager)| dir.join(file).is_file().then_some(*manager))
        {
            return ManagerResolution::Determined(manager);
        }
    }
    match existing_hint {
        Some(m) if m.is_compatible_with(format) => ManagerResolution::Determined(m),
        Some(m) => ManagerResolution::Incompatible(m),
        None => ManagerResolution::Undetermined,
    }
}

/// The command(s), run in order, that bring a managed Python project's
/// lockfile/environment back in sync after its manifest has been rewritten
/// to depend on `dbt` (v2) instead of `dbt-core` -- editing the manifest
/// alone can leave a lockfile stale relative to it. Managers with their own
/// lock/sync step re-lock as part of the listed command; `pip`-family
/// managers have no lock step and no rename semantics, so installing the
/// new spec doesn't remove the old package -- hence two commands for that
/// branch, reusing [`uninstall_command_for_package`] for the second half.
///
/// Returns `None` when there's no automatic command this function can
/// vouch for -- currently only Pipx (installs applications into isolated
/// per-app venvs, not project dependencies from a manifest; there's no
/// `pipx`-native concept of "resync this project's manifest"). Callers
/// should fall back to a manual "re-run your package manager's
/// install/lock command yourself" message rather than run a fabricated
/// command -- see `exec_managed_project_upgrade` in `upgrade.rs`.
pub fn sync_command_for_manager(
    manager: PythonPackageManager,
    format: PythonManifestFormat,
    manifest_path: &Path,
) -> Option<Vec<String>> {
    match manager {
        PythonPackageManager::Uv => Some(vec!["uv sync".to_string()]),
        // `poetry install` errors on a lockfile that's stale relative to
        // pyproject.toml, so the lock step must run first.
        PythonPackageManager::Poetry => Some(vec![
            "poetry lock".to_string(),
            "poetry install".to_string(),
        ]),
        PythonPackageManager::Pdm => Some(vec!["pdm install".to_string()]),
        PythonPackageManager::Pipenv => Some(vec!["pipenv install".to_string()]),
        // `--prune` is required or the old `dbt-core` package lingers
        // alongside the new `dbt` entry. The manifest's full path (not just
        // its filename) is spliced in -- `PythonManifest::detect` walks up
        // through parent directories, so the manifest can legitimately live
        // outside the cwd `conda env update` will run from, and it may be
        // named `environment.yml` *or* `environment.yaml`. Quoted since the
        // path is spliced into a shell command string (`run_shell_command`
        // runs it via `sh -c`/`cmd /C`) and isn't guaranteed to be free of
        // spaces or other shell-meaningful characters.
        PythonPackageManager::Conda => {
            // Callers going through `resolve_manager_for_manifest`
            // (`upgrade.rs`'s `exec_managed_project_upgrade`) can no longer
            // reach this arm with a mismatched `format` -- that function now
            // derives `manager` from the manifest itself, and a
            // `CondaEnvironment` manifest deterministically implies `Conda`
            // and nothing else. This guard is kept anyway as defense-in-depth
            // for the small number of other callers: `sync_command_for_manager`
            // is a `pub fn`, and nothing stops a caller (present or future,
            // including tests) from passing an inconsistent
            // `(manager, format, manifest_path)` triple directly. In that
            // case, `manifest_path` may point at some other manifest (e.g.
            // `pyproject.toml`) whose edit no conda command can pick up --
            // `find_conda_matches` in `python.rs` only recognizes direct
            // package scalars in an `environment.yml`'s top-level
            // `dependencies:`/nested `pip:` list, never a `pip:`-style
            // `-r <file>` reference into another manifest -- so guessing a
            // filename and running `conda env update` on it would either
            // silently no-op or update an unrelated environment. Decline to
            // guess and fall through to the same "no automatic sync command"
            // path used by Pipx -- see `manual_sync_required_err` in
            // `upgrade.rs`.
            if format != PythonManifestFormat::CondaEnvironment {
                return None;
            }
            Some(vec![format!(
                "conda env update -f {} --prune",
                quote_shell_arg(&manifest_path.display().to_string())
            )])
        }
        // Hatch runs everything through its own managed environment rather
        // than the ambient interpreter -- mirrors the existing
        // `hatch run pip uninstall` shape in `uninstall_command_for_package`
        // below (and `PathDiscovery::command_strings`'s `hatch run pip
        // install --upgrade dbt` in `lib.rs`) by wrapping the same
        // format-aware pip install used by the plain pip-family arm in
        // `hatch run`.
        PythonPackageManager::Hatch => {
            let install = match format {
                PythonManifestFormat::Requirements => format!(
                    "hatch run pip install -r {}",
                    quote_shell_arg(&manifest_path.display().to_string())
                ),
                // No filename to splice in here -- `pip install -e .`
                // installs from whatever project directory it's run in, not
                // a specific manifest file, so relies on the caller having
                // set `current_dir` to the manifest's own directory (as
                // `exec_managed_project_upgrade`'s `sync_dir` does).
                _ => "hatch run pip install -e .".to_string(),
            };
            let mut commands = vec![install];
            commands.extend(uninstall_command_for_package(
                Channel::Pypi,
                Some(manager),
                "dbt-core",
            ));
            Some(commands)
        }
        // Asdf/Mise/Pyenv are Python *version* managers, not package
        // managers -- they have no install/lock command of their own, so
        // falling through to plain `pip` (same as bare Pip) is correct here,
        // not a gap. `uninstall_command_for_package` already treats these
        // three identically to Pip for the same reason.
        PythonPackageManager::Pip
        | PythonPackageManager::Asdf
        | PythonPackageManager::Mise
        | PythonPackageManager::Pyenv => {
            let install = match format {
                PythonManifestFormat::Requirements => format!(
                    "pip install -r {}",
                    quote_shell_arg(&manifest_path.display().to_string())
                ),
                // See the `Hatch` arm above for why `-e .` stays relative --
                // there's no manifest filename to splice into it.
                _ => "pip install -e .".to_string(),
            };
            let mut commands = vec![install];
            commands.extend(uninstall_command_for_package(
                Channel::Pypi,
                Some(manager),
                "dbt-core",
            ));
            Some(commands)
        }
        // Pipx installs applications into isolated per-app venvs, not
        // project dependencies from a manifest -- there's no pipx-native
        // "resync this project" operation, and a bare `pip install ...`
        // would target whatever environment happens to be active, not the
        // pipx-managed one. No command in this codebase to vouch for; see
        // this function's doc comment.
        PythonPackageManager::Pipx => None,
        // `rye sync` relocks and resyncs the venv from `pyproject.toml` in
        // one atomic step (https://rye.astral.sh/guide/sync/), so no
        // separate uninstall command is needed here (unlike the
        // pip-family/hatch arms above).
        PythonPackageManager::Rye => Some(vec!["rye sync".to_string()]),
    }
}

/// Quotes `s` for safe interpolation into the shell command line
/// `run_shell_command` (in `upgrade.rs`) actually runs it with -- `sh -c` on
/// non-Windows, `cmd /C` on Windows. Those two shells disagree on which
/// character is the quote character, so the selection has to be
/// platform-aware; this mirrors the shape `unix_install_command_display`/
/// `windows_install_command_display`/`install_command_display` already use
/// in `upgrade.rs`: two always-compiled, `cfg!`-free pure builders (each
/// independently unit-testable on any host) plus this trivial `cfg!(windows)`
/// selector, which itself needs no dedicated test.
fn quote_shell_arg(s: &str) -> String {
    if cfg!(windows) {
        quote_cmd_arg(s)
    } else {
        quote_posix_shell_arg(s)
    }
}

/// POSIX single-quoted-string quoting, for the `sh -c` invocation
/// `run_shell_command` uses on non-Windows. Wraps `s` in single quotes,
/// escaping any embedded single quote as `'\''` (close the quote, emit an
/// escaped literal quote via a separate single-quoted-by-backslash segment,
/// then reopen) -- the one escape a POSIX single-quoted string needs.
fn quote_posix_shell_arg(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// `cmd.exe`-style double-quoted-string quoting, for the `cmd /C` invocation
/// `run_shell_command` uses on Windows. `cmd`/the Windows CRT argv parser
/// treats `"` (not `'`) as the quote character, so a bare double-quote wrap
/// is sufficient here: `'` needs no escaping under `cmd`'s rules, and `"`
/// itself never needs escaping in practice because it's one of the
/// characters Windows forbids in a path -- a real manifest path can't
/// contain one to begin with.
fn quote_cmd_arg(s: &str) -> String {
    format!("\"{s}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_serializes_to_spec_contract() {
        assert_eq!(
            serde_json::to_string(&Channel::Standalone).unwrap(),
            "\"standalone\""
        );
        assert_eq!(serde_json::to_string(&Channel::Pypi).unwrap(), "\"pypi\"");
        assert_eq!(serde_json::to_string(&Channel::Brew).unwrap(), "\"brew\"");
        assert_eq!(
            serde_json::to_string(&Channel::Winget).unwrap(),
            "\"winget\""
        );
        assert_eq!(
            serde_json::to_string(&Channel::Unclaimed).unwrap(),
            "\"unclaimed\""
        );
    }

    fn sample_dist_info() -> DistInfo {
        DistInfo {
            path: "/home/user/.venv/bin/dbt".to_string(),
            channel: Some(Channel::Pypi),
            distribution: Some(Distribution::Dbt),
            generation: Generation::V2,
            py_package_manager: Some(PythonPackageManager::Uv),
            py_venv_root: Some("/home/user/.venv".to_string()),
            version: Some("2.0.0-preview.203".to_string()),
            is_prerelease: Some(true),
            upgrade_cmd: Some("uv tool upgrade dbt".to_string()),
            uninstall_cmd: Some("uv tool uninstall dbt".to_string()),
        }
    }

    #[test]
    fn dist_info_serializes_full_contract_shape() {
        let info = sample_dist_info();
        let value: serde_json::Value = serde_json::to_value(&info).unwrap();
        assert_eq!(value["path"], "/home/user/.venv/bin/dbt");
        assert_eq!(value["channel"], "pypi");
        assert_eq!(value["distribution"], "dbt");
        assert_eq!(value["generation"], "v2");
        assert_eq!(value["py_package_manager"], "uv");
        assert_eq!(value["py_venv_root"], "/home/user/.venv");
        assert_eq!(value["version"], "2.0.0-preview.203");
        assert_eq!(value["is_prerelease"], true);
        assert_eq!(value["upgrade_cmd"], "uv tool upgrade dbt");
        assert_eq!(value["uninstall_cmd"], "uv tool uninstall dbt");
    }

    #[test]
    fn is_self_managed_true_for_standalone_and_unclaimed() {
        for channel in [Channel::Standalone, Channel::Unclaimed] {
            let mut info = sample_dist_info();
            info.channel = Some(channel.clone());
            assert!(info.is_self_managed(), "{channel:?} should be self-managed");
        }
    }

    #[test]
    fn is_self_managed_false_for_managed_channels() {
        for channel in [Channel::Brew, Channel::Winget, Channel::Pypi] {
            let mut info = sample_dist_info();
            info.channel = Some(channel.clone());
            assert!(
                !info.is_self_managed(),
                "{channel:?} should not be self-managed"
            );
        }
        let mut info = sample_dist_info();
        info.channel = None;
        assert!(!info.is_self_managed());
    }

    #[test]
    fn upgrade_command_for_version_pins_winget() {
        let mut info = sample_dist_info();
        info.channel = Some(Channel::Winget);
        info.upgrade_cmd = Some("winget upgrade --id dbtLabs.dbt --exact".to_string());
        assert_eq!(
            info.upgrade_command_for_version(Some("2.0.0-preview.180")),
            Some("winget install --id dbtLabs.dbt --exact --version 2.0.0-preview.180".to_string())
        );
        assert_eq!(
            info.upgrade_command_for_version(None),
            Some("winget upgrade --id dbtLabs.dbt --exact".to_string())
        );
    }

    #[test]
    fn upgrade_command_for_version_passes_through_other_channels() {
        let info = sample_dist_info();
        assert_eq!(
            info.upgrade_command_for_version(Some("2.0.0-preview.180")),
            info.upgrade_cmd
        );
    }

    #[test]
    fn install_label_covers_every_channel() {
        let cases = [
            (Some(Channel::Standalone), None, "the standalone installer"),
            (Some(Channel::Unclaimed), None, "the standalone installer"),
            (Some(Channel::Brew), None, "Homebrew"),
            (Some(Channel::Winget), None, "winget"),
            (Some(Channel::Pypi), Some(PythonPackageManager::Pip), "pip"),
            (Some(Channel::Pypi), None, "a Python package manager"),
            (None, None, "another package manager"),
        ];
        for (channel, manager, expected) in cases {
            let mut info = sample_dist_info();
            info.channel = channel.clone();
            info.py_package_manager = manager;
            assert_eq!(info.install_label(), expected, "channel={channel:?}");
        }
    }

    #[test]
    fn uninstall_command_for_package_covers_every_channel_and_manager() {
        let cases: &[(Channel, Option<PythonPackageManager>, &str)] = &[
            (Channel::Standalone, None, "dbt system uninstall"),
            (Channel::Unclaimed, None, "dbt system uninstall"),
            (Channel::Brew, None, "brew uninstall dbt-core"),
            (
                Channel::Winget,
                None,
                "winget uninstall --id dbtLabs.dbt-core --exact",
            ),
            (
                Channel::Pypi,
                Some(PythonPackageManager::Pip),
                "pip uninstall -y dbt-core",
            ),
            (
                Channel::Pypi,
                Some(PythonPackageManager::Asdf),
                "pip uninstall -y dbt-core",
            ),
            (
                Channel::Pypi,
                Some(PythonPackageManager::Mise),
                "pip uninstall -y dbt-core",
            ),
            (
                Channel::Pypi,
                Some(PythonPackageManager::Pyenv),
                "pip uninstall -y dbt-core",
            ),
            (
                Channel::Pypi,
                Some(PythonPackageManager::Pipx),
                "pipx uninstall dbt-core",
            ),
            (
                Channel::Pypi,
                Some(PythonPackageManager::Uv),
                "uv tool uninstall dbt-core",
            ),
            (
                Channel::Pypi,
                Some(PythonPackageManager::Poetry),
                "poetry remove dbt-core",
            ),
            (
                Channel::Pypi,
                Some(PythonPackageManager::Pdm),
                "pdm remove dbt-core",
            ),
            (
                Channel::Pypi,
                Some(PythonPackageManager::Pipenv),
                "pipenv uninstall dbt-core",
            ),
            (
                Channel::Pypi,
                Some(PythonPackageManager::Hatch),
                "hatch run pip uninstall -y dbt-core",
            ),
            (
                Channel::Pypi,
                Some(PythonPackageManager::Conda),
                "conda remove -y dbt-core",
            ),
            (
                Channel::Pypi,
                Some(PythonPackageManager::Rye),
                "rye uninstall dbt-core",
            ),
        ];
        for (channel, manager, expected) in cases {
            assert_eq!(
                uninstall_command_for_package(channel.clone(), *manager, "dbt-core"),
                Some(expected.to_string()),
                "channel={channel:?} manager={manager:?}"
            );
        }
    }

    #[test]
    fn uninstall_command_for_package_pypi_with_no_manager_is_none() {
        assert_eq!(
            uninstall_command_for_package(Channel::Pypi, None, "dbt-core"),
            None
        );
    }

    #[test]
    fn sync_command_for_manager_covers_every_lock_aware_manager_except_conda() {
        // `manifest_path` is only consulted by the `Conda` arm -- see the
        // dedicated `sync_command_for_manager_conda_*` tests below for that
        // arm, including its platform-dependent quoting (kept out of this
        // table so those tests can build their expected strings via
        // `quote_shell_arg` and stay host-independent, rather than this
        // table hardcoding a POSIX-only quote character).
        let cases: &[(PythonPackageManager, PythonManifestFormat, &str, &[&str])] = &[
            (
                PythonPackageManager::Uv,
                PythonManifestFormat::Pyproject,
                "pyproject.toml",
                &["uv sync"],
            ),
            (
                PythonPackageManager::Rye,
                PythonManifestFormat::Pyproject,
                "pyproject.toml",
                &["rye sync"],
            ),
            (
                PythonPackageManager::Poetry,
                PythonManifestFormat::Pyproject,
                "pyproject.toml",
                &["poetry lock", "poetry install"],
            ),
            (
                PythonPackageManager::Pdm,
                PythonManifestFormat::Pyproject,
                "pyproject.toml",
                &["pdm install"],
            ),
            (
                PythonPackageManager::Pipenv,
                PythonManifestFormat::Pipfile,
                "Pipfile",
                &["pipenv install"],
            ),
        ];
        for (manager, format, manifest_path, expected) in cases {
            assert_eq!(
                sync_command_for_manager(*manager, *format, Path::new(manifest_path)),
                Some(expected.iter().map(|s| s.to_string()).collect::<Vec<_>>()),
                "manager={manager:?} format={format:?}"
            );
        }
    }

    #[test]
    fn resolve_manager_for_manifest_conda_environment_always_returns_conda() {
        // `CondaEnvironment` deterministically implies `Conda` -- no hint,
        // whether absent, agreeing, or actively conflicting, changes that.
        for hint in [
            None,
            Some(PythonPackageManager::Conda),
            Some(PythonPackageManager::Pipenv),
        ] {
            assert_eq!(
                resolve_manager_for_manifest(
                    PythonManifestFormat::CondaEnvironment,
                    Path::new("environment.yml"),
                    hint,
                ),
                ManagerResolution::Determined(PythonPackageManager::Conda),
                "hint={hint:?}"
            );
        }
    }

    #[test]
    fn resolve_manager_for_manifest_pipfile_always_returns_pipenv() {
        // Same deterministic shape as `CondaEnvironment`, for `Pipfile`.
        for hint in [
            None,
            Some(PythonPackageManager::Pipenv),
            Some(PythonPackageManager::Conda),
        ] {
            assert_eq!(
                resolve_manager_for_manifest(
                    PythonManifestFormat::Pipfile,
                    Path::new("Pipfile"),
                    hint,
                ),
                ManagerResolution::Determined(PythonPackageManager::Pipenv),
                "hint={hint:?}"
            );
        }
    }

    #[test]
    fn resolve_manager_for_manifest_pyproject_prefers_lockfile_in_manifest_dir_over_hint() {
        // A lockfile alongside the manifest is a stronger, more precise
        // signal than a hint resolved independently (e.g. by scanning
        // arbitrary ancestor directories) -- it must win even when the hint
        // actively disagrees.
        let cases: &[(&str, PythonPackageManager)] = &[
            ("uv.lock", PythonPackageManager::Uv),
            ("poetry.lock", PythonPackageManager::Poetry),
            ("pdm.lock", PythonPackageManager::Pdm),
        ];
        for (lockfile, expected_manager) in cases {
            let tmp = tempfile::tempdir().unwrap();
            std::fs::write(tmp.path().join(lockfile), "").unwrap();
            let manifest_path = tmp.path().join("pyproject.toml");
            assert_eq!(
                resolve_manager_for_manifest(
                    PythonManifestFormat::Pyproject,
                    &manifest_path,
                    Some(PythonPackageManager::Conda),
                ),
                ManagerResolution::Determined(*expected_manager),
                "lockfile={lockfile}"
            );
        }
    }

    #[test]
    fn resolve_manager_for_manifest_pyproject_with_no_lockfile_falls_back_to_compatible_hint() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest_path = tmp.path().join("pyproject.toml");
        assert_eq!(
            resolve_manager_for_manifest(
                PythonManifestFormat::Pyproject,
                &manifest_path,
                Some(PythonPackageManager::Hatch),
            ),
            ManagerResolution::Determined(PythonPackageManager::Hatch),
        );
    }

    #[test]
    fn resolve_manager_for_manifest_pyproject_with_no_lockfile_rejects_incompatible_hint() {
        // `Conda`/`Pipenv` are never compatible with a `Pyproject` manifest
        // -- an incompatible hint was resolved for a different file (e.g.
        // a stray `environment.yml` elsewhere in the ancestor chain) and
        // must not be trusted here.
        let tmp = tempfile::tempdir().unwrap();
        let manifest_path = tmp.path().join("pyproject.toml");
        assert_eq!(
            resolve_manager_for_manifest(
                PythonManifestFormat::Pyproject,
                &manifest_path,
                Some(PythonPackageManager::Conda),
            ),
            ManagerResolution::Incompatible(PythonPackageManager::Conda),
        );
    }

    #[test]
    fn resolve_manager_for_manifest_requirements_and_setup_cfg_use_hint_compatibility() {
        for format in [
            PythonManifestFormat::Requirements,
            PythonManifestFormat::SetupCfg,
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let manifest_path = tmp.path().join("requirements.txt");
            assert_eq!(
                resolve_manager_for_manifest(
                    format,
                    &manifest_path,
                    Some(PythonPackageManager::Pip)
                ),
                ManagerResolution::Determined(PythonPackageManager::Pip),
                "format={format:?} compatible hint"
            );
            // `Conda`/`Pipenv` are excluded because they're not
            // `Requirements`/`SetupCfg`'s 1:1 file format at all; `Uv`/`Rye`
            // are excluded even though they're otherwise normal pip-family-
            // adjacent managers, because their `sync_command_for_manager`
            // commands (`uv sync`/`rye sync`) read `pyproject.toml`/their
            // own lockfile, not a `requirements.txt`/`setup.cfg` -- trusting
            // either here would silently sync from the wrong file, the same
            // failure mode `is_compatible_with` exists to prevent.
            for incompatible in [
                PythonPackageManager::Conda,
                PythonPackageManager::Uv,
                PythonPackageManager::Rye,
            ] {
                assert_eq!(
                    resolve_manager_for_manifest(format, &manifest_path, Some(incompatible)),
                    ManagerResolution::Incompatible(incompatible),
                    "format={format:?} incompatible hint={incompatible:?}"
                );
            }
        }
    }

    #[test]
    fn sync_command_for_manager_conda_uses_environment_yaml_spelling() {
        // The manifest-detection table recognizes both `environment.yml` and
        // `environment.yaml` as conda manifests -- the generated sync
        // command must reference whichever spelling the manifest actually
        // uses, not hardcode `.yml`. Expected string is built via
        // `quote_shell_arg` (not a hardcoded quote character) so this test
        // is host-independent -- quoting-character correctness itself is
        // covered separately by `quote_posix_shell_arg_escapes_embedded_single_quotes`
        // and `quote_cmd_arg_wraps_in_double_quotes`.
        assert_eq!(
            sync_command_for_manager(
                PythonPackageManager::Conda,
                PythonManifestFormat::CondaEnvironment,
                Path::new("environment.yaml"),
            ),
            Some(vec![format!(
                "conda env update -f {} --prune",
                quote_shell_arg("environment.yaml")
            )]),
        );
    }

    #[test]
    fn sync_command_for_manager_conda_uses_full_manifest_path() {
        // `PythonManifest::detect` walks up through parent directories, so
        // the manifest can legitimately live outside the cwd `conda env
        // update` runs from -- the generated command must reference the
        // full path, not just the bare filename. Host-independent for the
        // same reason as the test above.
        assert_eq!(
            sync_command_for_manager(
                PythonPackageManager::Conda,
                PythonManifestFormat::CondaEnvironment,
                Path::new("project/environment.yml"),
            ),
            Some(vec![format!(
                "conda env update -f {} --prune",
                quote_shell_arg("project/environment.yml")
            )]),
        );
    }

    #[test]
    fn sync_command_for_manager_conda_quotes_path_with_spaces_and_single_quotes() {
        // Integration-level check that `sync_command_for_manager` actually
        // routes the `Conda` arm's path through `quote_shell_arg` (not just
        // that the helper itself works in isolation, per
        // `quote_posix_shell_arg_escapes_embedded_single_quotes` /
        // `quote_cmd_arg_wraps_in_double_quotes` below). Built via
        // `quote_shell_arg` rather than a hardcoded quote character so this
        // stays host-independent.
        let path = "my project's dir/environment.yml";
        assert_eq!(
            sync_command_for_manager(
                PythonPackageManager::Conda,
                PythonManifestFormat::CondaEnvironment,
                Path::new(path),
            ),
            Some(vec![format!(
                "conda env update -f {} --prune",
                quote_shell_arg(path)
            )]),
        );
    }

    #[test]
    fn sync_command_for_manager_conda_falls_back_when_format_disagrees_with_manager() {
        // `sync_command_for_manager` is `pub fn`, so a direct caller can
        // still pass a `manager`/`format`/`manifest_path` triple that
        // disagrees with itself (e.g. `Conda` with a `pyproject.toml`) even
        // though `resolve_manager_for_manifest` (see its own tests below)
        // now prevents this mismatch from arising via the normal
        // `exec_managed_project_upgrade` flow. This test is therefore a
        // defense-in-depth check on the guard itself, not a reproduction of
        // a reachable end-to-end path: in the mismatched case, `manifest_path`
        // points at `pyproject.toml`, not a conda environment file -- the
        // edit that was just applied lives there, and no conda command can
        // pick it up (there's no `pip:`-style `-r <file>` indirection from
        // an `environment.yml` into another manifest for `find_conda_matches`
        // to have followed). Guessing a filename would just produce a
        // command that silently fails to sync the real edit, so this must
        // return `None` and fall through to the same "no automatic sync
        // command" path as Pipx.
        assert_eq!(
            sync_command_for_manager(
                PythonPackageManager::Conda,
                PythonManifestFormat::Pyproject,
                Path::new("/some/project/pyproject.toml"),
            ),
            None,
        );
    }

    #[test]
    fn quote_posix_shell_arg_escapes_embedded_single_quotes() {
        assert_eq!(
            quote_posix_shell_arg("environment.yml"),
            "'environment.yml'"
        );
        assert_eq!(
            quote_posix_shell_arg("my project's dir/environment.yaml"),
            "'my project'\\''s dir/environment.yaml'"
        );
    }

    #[test]
    fn quote_cmd_arg_wraps_in_double_quotes() {
        assert_eq!(quote_cmd_arg("environment.yml"), "\"environment.yml\"");
        // `'` needs no escaping under `cmd`'s quoting rules (only `"` is
        // special), so a path containing one still just gets wrapped.
        assert_eq!(
            quote_cmd_arg("my project's dir/environment.yaml"),
            "\"my project's dir/environment.yaml\""
        );
    }

    #[test]
    fn sync_command_for_manager_pip_family_installs_then_uninstalls_old_package() {
        // Pip, Asdf, Mise, and Pyenv all fall through to the same bare-`pip`
        // behavior -- Asdf/Mise/Pyenv are Python *version* managers with no
        // install/lock command of their own, and `uninstall_command_for_package`
        // already treats all four identically (`pip uninstall`), so this group
        // staying on generic `pip` is intentional, not an unhandled case.
        //
        // `manifest_path` varies per case for `Requirements` -- the `-r`
        // command now embeds the actual manifest path passed in (quoted via
        // `quote_shell_arg`, kept dynamic here rather than a hardcoded quote
        // character so this test stays host-independent -- see the
        // `Conda`/`quote_shell_arg` tests above for the same pattern), not a
        // hardcoded `requirements.txt` literal; the non-`Requirements` cases
        // use `-e .` regardless of `manifest_path`, so an arbitrary path is
        // fine there.
        let cases: &[(
            PythonPackageManager,
            PythonManifestFormat,
            &str,
            Option<&str>,
        )] = &[
            (
                PythonPackageManager::Pip,
                PythonManifestFormat::Requirements,
                "requirements.txt",
                Some("requirements.txt"),
            ),
            (
                PythonPackageManager::Pip,
                PythonManifestFormat::Requirements,
                "/some/project/requirements.txt",
                Some("/some/project/requirements.txt"),
            ),
            (
                PythonPackageManager::Pip,
                PythonManifestFormat::SetupCfg,
                "pyproject.toml",
                None,
            ),
            (
                PythonPackageManager::Pip,
                PythonManifestFormat::Pyproject,
                "pyproject.toml",
                None,
            ),
            (
                PythonPackageManager::Asdf,
                PythonManifestFormat::Pyproject,
                "pyproject.toml",
                None,
            ),
            (
                PythonPackageManager::Mise,
                PythonManifestFormat::Pyproject,
                "pyproject.toml",
                None,
            ),
            (
                PythonPackageManager::Pyenv,
                PythonManifestFormat::Requirements,
                "requirements.txt",
                Some("requirements.txt"),
            ),
        ];
        for (manager, format, manifest_path, requirements_path) in cases {
            let install = match requirements_path {
                Some(path) => format!("pip install -r {}", quote_shell_arg(path)),
                None => "pip install -e .".to_string(),
            };
            assert_eq!(
                sync_command_for_manager(*manager, *format, Path::new(manifest_path)),
                Some(vec![install, "pip uninstall -y dbt-core".to_string()]),
                "manager={manager:?} format={format:?}"
            );
        }
    }

    #[test]
    fn sync_command_for_manager_hatch_runs_install_through_hatch_run() {
        // Hatch gets its own arm (not the plain pip-family catch-all):
        // everything runs through `hatch run`, mirroring the existing
        // `hatch run pip uninstall` shape in `uninstall_command_for_package`.
        // Same path-awareness as the pip-family test above: the
        // `Requirements` command now embeds the actual manifest path, quoted
        // via `quote_shell_arg` (dynamic, not a hardcoded quote character)
        // to stay host-independent.
        let cases: &[(PythonManifestFormat, &str, Option<&str>)] = &[
            (
                PythonManifestFormat::Requirements,
                "requirements.txt",
                Some("requirements.txt"),
            ),
            (
                PythonManifestFormat::Requirements,
                "/some/project/requirements.txt",
                Some("/some/project/requirements.txt"),
            ),
            (PythonManifestFormat::SetupCfg, "pyproject.toml", None),
            (PythonManifestFormat::Pyproject, "pyproject.toml", None),
        ];
        for (format, manifest_path, requirements_path) in cases {
            let install = match requirements_path {
                Some(path) => format!("hatch run pip install -r {}", quote_shell_arg(path)),
                None => "hatch run pip install -e .".to_string(),
            };
            assert_eq!(
                sync_command_for_manager(
                    PythonPackageManager::Hatch,
                    *format,
                    Path::new(manifest_path)
                ),
                Some(vec![
                    install,
                    "hatch run pip uninstall -y dbt-core".to_string()
                ]),
                "format={format:?}"
            );
        }
    }

    #[test]
    fn sync_command_for_manager_pipx_has_no_automatic_command() {
        // Pipx installs applications into isolated per-app venvs, not
        // project dependencies from a manifest -- there's no pipx-native
        // "resync this project" operation and no repo evidence to justify
        // fabricating one, so this must signal "no automatic command"
        // rather than emit a wrong `pip install`.
        assert_eq!(
            sync_command_for_manager(
                PythonPackageManager::Pipx,
                PythonManifestFormat::Requirements,
                Path::new("requirements.txt"),
            ),
            None,
        );
    }
}

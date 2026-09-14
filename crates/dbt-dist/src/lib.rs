//! Distribution, install channel, and self-update/uninstall detection for dbt.

pub mod command;
pub mod confirm;
pub mod dist;
mod proc;
pub mod python;
pub mod upgrade;
pub mod version;
use std::{
    collections::HashSet,
    env,
    io::Read,
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

use dbt_common::{ErrorCode, FsResult, err, error::WrappedError, fs_err};
pub use dbt_dist_classify::classify_version_output;
pub use dist::{Channel, DistInfo, Distribution, Generation, uninstall_command_for_package};

use crate::proc::{GRACE_WAIT, NORMAL_WAIT, ProcessOutput, real_run};
use crate::python::PythonManifestFormat;
pub use crate::python::PythonPackageManager;

/// Entry point for discovering [`DistInfo`] about one or more `dbt`
/// installations.
pub enum DistInfoDiscovery<'a> {
    /// The currently running binary.
    Current,
    /// A specific `dbt` executable found elsewhere on disk.
    AtLocation(&'a Path),
    /// Every `dbt`/`dbt.exe` found on `PATH`.
    AllInPath,
}

impl<'a> DistInfoDiscovery<'a> {
    /// `command_name` is the CLI-brand name of the currently running binary
    /// (e.g. `"dbt-core"` for OSS, or the proprietary dbt v2 build's name) —
    /// used only to resolve the *current* process's own distribution, since a
    /// process can't `--version`-probe itself the way it does other `dbt`s
    /// found on `PATH`.
    pub fn discover(self, command_name: &str) -> FsResult<Vec<DistInfo>> {
        match self {
            Self::Current => get_current(command_name).map(|d| vec![d]),
            Self::AtLocation(file_path) => get_at_path(file_path, command_name).map(|d| vec![d]),
            Self::AllInPath => get_all_in_path(command_name),
        }
    }
}

/// Environment and subprocess access, threaded through as closures (rather
/// than called directly) so the channel-detection rules that depend on them
/// can be exercised with canned inputs instead of real env mutation or real
/// tools on `PATH`.
///
/// `+ Send + Sync` on both trait objects: `upgrade::resolve_manager` holds a
/// `&DiscoveryContext` across an `.await` point in its caller
/// (`exec_managed_project_upgrade`), so the whole thing has to stay `Send`
/// for that `async fn`'s generated future to be `Send` -- required since the
/// CLI dispatch layer boxes it as `Pin<Box<dyn Future<Output = _> + Send>>`.
pub(crate) struct DiscoveryContext<'a> {
    env: &'a (dyn Fn(&str) -> Option<String> + Send + Sync),
    run: &'a (dyn Fn(&str, &[&str]) -> Option<ProcessOutput> + Send + Sync),
}

fn real_env(name: &str) -> Option<String> {
    env::var(name).ok()
}

impl DiscoveryContext<'static> {
    pub(crate) fn real() -> Self {
        DiscoveryContext {
            env: &real_env,
            run: &real_run,
        }
    }
}

/// Whether a resolved file is a native executable, a script (shebang or
/// Windows launcher shim), or undeterminable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileKind {
    NativeBinary,
    Script,
    Unknown,
}

/// Reads the first few bytes of `path` and classifies it as a native
/// executable (ELF/Mach-O/PE magic bytes) or a script (leading `#!`).
/// This is the sole discriminator needed for the one genuinely ambiguous
/// case in channel detection: a real file at `~/.local/bin/dbt` could be
/// either the standalone native binary or a `pip --user` console script.
fn sniff_file_kind(path: &Path) -> FileKind {
    const MACHO_MAGICS: [[u8; 4]; 5] = [
        [0xFE, 0xED, 0xFA, 0xCE], // Mach-O 32-bit BE
        [0xCE, 0xFA, 0xED, 0xFE], // Mach-O 32-bit LE
        [0xFE, 0xED, 0xFA, 0xCF], // Mach-O 64-bit BE
        [0xCF, 0xFA, 0xED, 0xFE], // Mach-O 64-bit LE
        [0xCA, 0xFE, 0xBA, 0xBE], // universal/fat binary
    ];

    let Ok(mut file) = std::fs::File::open(path) else {
        return FileKind::Unknown;
    };
    let mut buf = [0u8; 4];
    let Ok(n) = file.read(&mut buf) else {
        return FileKind::Unknown;
    };

    if n >= 4 && buf == *b"\x7fELF" {
        return FileKind::NativeBinary;
    }
    if n >= 4 && MACHO_MAGICS.contains(&buf) {
        return FileKind::NativeBinary;
    }
    if n >= 2 && &buf[0..2] == b"MZ" {
        return FileKind::NativeBinary;
    }
    if n >= 2 && &buf[0..2] == b"#!" {
        return FileKind::Script;
    }
    FileKind::Unknown
}

/// A file exists and has its executable bit set (unix) or simply exists
/// (other platforms, where "executable" isn't a permission-bit concept for
/// our purposes). Follows symlinks.
fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// A path's symlinks/shims resolved to their real target and classified:
/// `given` is the original path, `resolved` is its real target, and `kind`
/// is what that target actually is.
struct ClassifiedPath {
    given: PathBuf,
    resolved: PathBuf,
    kind: FileKind,
}

/// Resolves symlinks/shims to their real target, then classifies the target.
fn classify_path(path: &Path) -> ClassifiedPath {
    let resolved = dbt_common::stdfs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let kind = sniff_file_kind(&resolved);
    ClassifiedPath {
        given: path.to_path_buf(),
        resolved,
        kind,
    }
}

fn home_dir(ctx: &DiscoveryContext) -> Option<PathBuf> {
    (ctx.env)("HOME")
        .or_else(|| (ctx.env)("USERPROFILE"))
        .map(PathBuf::from)
}

/// Result of applying the path-based release-channel rules to a resolved
/// `dbt` location: the detected channel (if any), a distribution the path
/// alone was enough to prove (e.g. a Homebrew-tap conflict identifying the
/// dbt Cloud CLI), and a package-manager hint for `pypi` installs.
#[derive(Debug, Clone, Default)]
struct PathDiscovery {
    channel: Option<Channel>,
    distribution_override: Option<Distribution>,
    py_package_manager_hint: Option<PythonPackageManager>,
}

impl PathDiscovery {
    /// Derives the upgrade/uninstall commands to surface to the user.
    ///
    /// Checked ahead of `channel`: a Homebrew-tap conflict identifies the dbt
    /// Cloud CLI (see `detect_homebrew`) without resolving a release channel
    /// at all, but we still owe the user an uninstall command for the
    /// conflicting install.
    ///
    /// `is_prerelease` is whether the *currently installed* version is
    /// itself a pre-release. dbt ships pre-release versions today
    /// (`2.0.0-preview.N`), which most package managers skip by default once
    /// a stable release exists — a pre-release install should still be able
    /// to move to another pre-release, but a stable install must never be
    /// silently upgraded into one, so the pre-release-allowing flag is only
    /// added when `is_prerelease` is true.
    fn command_strings(
        &self,
        manager: Option<PythonPackageManager>,
        is_prerelease: bool,
    ) -> (Option<String>, Option<String>) {
        if self.distribution_override == Some(Distribution::CloudCLI) {
            return (
                None,
                Some("brew uninstall dbt-labs/dbt-cli/dbt".to_string()),
            );
        }
        match self.channel {
            Some(Channel::Standalone) | Some(Channel::Unclaimed) => (
                Some("dbt system update".to_string()),
                Some("dbt system uninstall".to_string()),
            ),
            Some(Channel::Brew) => (
                Some("brew upgrade dbt".to_string()),
                Some("brew uninstall dbt".to_string()),
            ),
            Some(Channel::Winget) => (
                Some("winget upgrade --id dbtLabs.dbt --exact".to_string()),
                Some("winget uninstall --id dbtLabs.dbt --exact".to_string()),
            ),
            // We don't publish to this manager, so there's no command to
            // vouch for — `DistInfo::unsupported_channel_message` covers
            // the user-facing message instead.
            Some(Channel::Unsupported(_)) => (None, None),
            Some(Channel::Pypi) => {
                let Some(manager) = manager else {
                    return (None, None);
                };
                let commands = match manager {
                    PythonPackageManager::Pip
                    | PythonPackageManager::Asdf
                    | PythonPackageManager::Mise
                    | PythonPackageManager::Pyenv => {
                        if is_prerelease {
                            ("pip install --pre --upgrade dbt", "pip uninstall dbt")
                        } else {
                            ("pip install --upgrade dbt", "pip uninstall dbt")
                        }
                    }
                    PythonPackageManager::Pipx => {
                        if is_prerelease {
                            (
                                "pipx upgrade --pip-args=\"--pre\" dbt",
                                "pipx uninstall dbt",
                            )
                        } else {
                            ("pipx upgrade dbt", "pipx uninstall dbt")
                        }
                    }
                    PythonPackageManager::Uv => {
                        if is_prerelease {
                            (
                                "uv tool upgrade --prerelease allow dbt",
                                "uv tool uninstall dbt",
                            )
                        } else {
                            ("uv tool upgrade dbt", "uv tool uninstall dbt")
                        }
                    }
                    // Poetry has no per-invocation flag for `update`; the
                    // documented mechanism for a one-off pre-release pull is
                    // `add --allow-prereleases` instead.
                    PythonPackageManager::Poetry => {
                        if is_prerelease {
                            ("poetry add --allow-prereleases dbt", "poetry remove dbt")
                        } else {
                            ("poetry update dbt", "poetry remove dbt")
                        }
                    }
                    PythonPackageManager::Pdm => {
                        if is_prerelease {
                            ("pdm update --prerelease dbt", "pdm remove dbt")
                        } else {
                            ("pdm update dbt", "pdm remove dbt")
                        }
                    }
                    PythonPackageManager::Pipenv => {
                        if is_prerelease {
                            ("pipenv update --pre dbt", "pipenv uninstall dbt")
                        } else {
                            ("pipenv update dbt", "pipenv uninstall dbt")
                        }
                    }
                    PythonPackageManager::Hatch => {
                        if is_prerelease {
                            (
                                "hatch run pip install --pre --upgrade dbt",
                                "hatch run pip uninstall dbt",
                            )
                        } else {
                            (
                                "hatch run pip install --upgrade dbt",
                                "hatch run pip uninstall dbt",
                            )
                        }
                    }
                    // conda's pre-release handling is channel-label-based
                    // (e.g. `-c conda-forge/label/prerelease`), not a
                    // per-invocation flag, and there's no such channel for
                    // dbt -- left as-is rather than guessed.
                    PythonPackageManager::Conda => ("conda update dbt", "conda remove dbt"),
                    // rye has no native "upgrade" verb (see the comment on
                    // its command below) or documented pre-release flag --
                    // left as-is rather than guessed.
                    PythonPackageManager::Rye => ("rye install dbt", "rye uninstall dbt"),
                };
                (Some(commands.0.to_string()), Some(commands.1.to_string()))
            }
            None => (None, None),
        }
    }
}

fn is_home_local_bin_dbt(resolved: &Path, home: &Path) -> bool {
    resolved == home.join(".local/bin/dbt") || resolved == home.join(".local/bin/dbt.exe")
}

/// Determines whether `resolved` sits under the Homebrew prefix and, if so,
/// which tap installed it. Returns `None` when not under Homebrew at all.
fn detect_homebrew(
    ctx: &DiscoveryContext,
    resolved: &Path,
) -> Option<(Option<Channel>, Option<Distribution>)> {
    // Canonical Homebrew install prefixes. Each formula is installed into a
    // keg under `<prefix>/Cellar/<formula>/<version>/...` and then symlinked
    // into `<prefix>/bin`; these three prefixes are the only ones the
    // official installer uses. See https://docs.brew.sh/Installation.
    const HOMEBREW_CELLAR_PREFIXES: &[&str] = &[
        "/opt/homebrew/Cellar",              // Apple Silicon macOS
        "/usr/local/Cellar",                 // Intel macOS
        "/home/linuxbrew/.linuxbrew/Cellar", // Linuxbrew
    ];

    let Some(prefix_output) = (ctx.run)("brew", &["--prefix"]) else {
        // `brew` isn't callable at all (e.g. not on PATH in a non-interactive
        // shell or minimal environment). We can't resolve the tap without the
        // subprocess, but we can still recognize a Cellar-rooted binary by
        // path alone so it isn't misclassified as `Unclaimed` (and therefore
        // treated as self-updatable) just because the probe couldn't run.
        return HOMEBREW_CELLAR_PREFIXES
            .iter()
            .any(|prefix| resolved.starts_with(prefix))
            .then_some((Some(Channel::Brew), None));
    };
    if !prefix_output.success {
        return None;
    }
    let prefix = prefix_output.stdout.trim();
    if prefix.is_empty() || !resolved.starts_with(prefix) {
        return None;
    }

    const UNTRUSTED_TAP_ERROR: &str =
        "Refusing to load formula dbt-labs/dbt-cli/dbt from untrusted tap dbt-labs/dbt-cli";

    match (ctx.run)("brew", &["info", "dbt", "--json=v2"]) {
        Some(info) if info.success => {
            let tap = serde_json::from_str::<serde_json::Value>(&info.stdout)
                .ok()
                .and_then(|v| {
                    v.get("formulae")?
                        .as_array()?
                        .first()?
                        .get("tap")?
                        .as_str()
                        .map(str::to_string)
                });
            match tap.as_deref() {
                Some("dbt-labs/dbt-cli") => Some((None, Some(Distribution::CloudCLI))),
                _ => Some((Some(Channel::Brew), None)),
            }
        }
        Some(info) if info.stderr.contains(UNTRUSTED_TAP_ERROR) => {
            Some((None, Some(Distribution::CloudCLI)))
        }
        _ => Some((Some(Channel::Brew), None)),
    }
}

fn detect_winget(path: &Path) -> bool {
    let normalized = path.to_string_lossy().replace('/', "\\").to_lowercase();
    normalized.contains("\\winget\\links\\") || normalized.contains("\\winget\\packages\\")
}

/// Recognizes a native binary installed via a Windows package manager we
/// don't publish to. Unlike Homebrew/Winget/PyPI, we have no official
/// package on Scoop or Chocolatey, so there's no channel-specific
/// upgrade/uninstall command to offer — see `Channel::Unsupported`. Matches
/// both the user-scoped default (`%USERPROFILE%\scoop\...`) and the
/// all-users default (`C:\ProgramData\chocolatey\...`), since a plain
/// substring check on the normalized path covers either root.
fn detect_unsupported_manager(path: &Path) -> Option<&'static str> {
    let normalized = path.to_string_lossy().replace('/', "\\").to_lowercase();
    if normalized.contains("\\scoop\\apps\\") || normalized.contains("\\scoop\\shims\\") {
        Some("Scoop")
    } else if normalized.contains("\\chocolatey\\") {
        Some("Chocolatey")
    } else {
        None
    }
}

/// Checks whether `given` or `resolved` falls under a tool-isolated /
/// version-manager directory, honoring an env-var override when the tool
/// supports relocating it (e.g. `PIPX_HOME`), falling back to the default
/// `$HOME`-relative location.
fn matches_tool_dir(
    given: &Path,
    resolved: &Path,
    home: Option<&Path>,
    ctx: &DiscoveryContext,
    env_override: Option<&str>,
    default_rel: &[&str],
) -> bool {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(env_var) = env_override {
        if let Some(custom) = (ctx.env)(env_var) {
            if !custom.is_empty() {
                candidates.push(PathBuf::from(custom));
            }
        }
    }
    if let Some(home) = home {
        let mut dir = home.to_path_buf();
        for part in default_rel {
            dir.push(part);
        }
        candidates.push(dir);
    }
    candidates
        .iter()
        .any(|dir| given.starts_with(dir) || resolved.starts_with(dir))
}

/// Applies the path-based release-channel rules in the order documented in
/// "Resolving the release channel". Native-binary-only channels
/// (standalone/brew/winget/unsupported-manager) are gated on
/// `kind == NativeBinary`; every other rule matches on location alone, since
/// a Windows pip/pipx launcher shim is
/// itself a PE binary and would otherwise defeat a kind-based check there.
/// `Unclaimed` (an unrecognized native binary) is checked last, after every
/// location-based rule has had a chance to match, for the same reason.
fn discover_channel(
    ctx: &DiscoveryContext,
    given: &Path,
    resolved: &Path,
    kind: FileKind,
) -> PathDiscovery {
    let home = home_dir(ctx);

    if kind == FileKind::NativeBinary {
        if let Some(home) = &home {
            if is_home_local_bin_dbt(resolved, home) {
                return PathDiscovery {
                    channel: Some(Channel::Standalone),
                    ..Default::default()
                };
            }
        }
        if let Some((channel, distribution_override)) = detect_homebrew(ctx, resolved) {
            return PathDiscovery {
                channel,
                distribution_override,
                ..Default::default()
            };
        }
        if detect_winget(given) || detect_winget(resolved) {
            return PathDiscovery {
                channel: Some(Channel::Winget),
                ..Default::default()
            };
        }
        if let Some(manager) =
            detect_unsupported_manager(given).or_else(|| detect_unsupported_manager(resolved))
        {
            return PathDiscovery {
                channel: Some(Channel::Unsupported(manager.to_string())),
                ..Default::default()
            };
        }
    }

    let home_ref = home.as_deref();

    if matches_tool_dir(
        given,
        resolved,
        home_ref,
        ctx,
        Some("PIPX_HOME"),
        &[".local", "pipx"],
    ) {
        return PathDiscovery {
            channel: Some(Channel::Pypi),
            py_package_manager_hint: Some(PythonPackageManager::Pipx),
            ..Default::default()
        };
    }
    if matches_tool_dir(
        given,
        resolved,
        home_ref,
        ctx,
        Some("UV_TOOL_DIR"),
        &[".local", "share", "uv", "tools"],
    ) {
        return PathDiscovery {
            channel: Some(Channel::Pypi),
            py_package_manager_hint: Some(PythonPackageManager::Uv),
            ..Default::default()
        };
    }
    if matches_tool_dir(given, resolved, home_ref, ctx, None, &[".rye", "tools"]) {
        return PathDiscovery {
            channel: Some(Channel::Pypi),
            py_package_manager_hint: Some(PythonPackageManager::Rye),
            ..Default::default()
        };
    }
    if matches_tool_dir(given, resolved, home_ref, ctx, None, &[".asdf"]) {
        return PathDiscovery {
            channel: Some(Channel::Pypi),
            py_package_manager_hint: Some(PythonPackageManager::Asdf),
            ..Default::default()
        };
    }
    if matches_tool_dir(given, resolved, home_ref, ctx, None, &[".pyenv", "shims"])
        || matches_tool_dir(
            given,
            resolved,
            home_ref,
            ctx,
            None,
            &[".pyenv", "versions"],
        )
    {
        return PathDiscovery {
            channel: Some(Channel::Pypi),
            py_package_manager_hint: Some(PythonPackageManager::Pyenv),
            ..Default::default()
        };
    }
    if matches_tool_dir(
        given,
        resolved,
        home_ref,
        ctx,
        None,
        &[".local", "share", "mise", "shims"],
    ) {
        return PathDiscovery {
            channel: Some(Channel::Pypi),
            py_package_manager_hint: Some(PythonPackageManager::Mise),
            ..Default::default()
        };
    }

    if let Some(virtual_env) = (ctx.env)("VIRTUAL_ENV") {
        let venv_bin =
            PathBuf::from(&virtual_env).join(if cfg!(windows) { "Scripts" } else { "bin" });
        if resolved.starts_with(&venv_bin) {
            return PathDiscovery {
                channel: Some(Channel::Pypi),
                ..Default::default()
            };
        }
    }

    if kind == FileKind::Script {
        return PathDiscovery {
            channel: Some(Channel::Pypi),
            ..Default::default()
        };
    }

    // A native binary, but no known package manager or the standalone
    // installer's canonical location claims it (a dev build, or one placed
    // somewhere non-standard). Nothing else owns it, so treat it as
    // self-managed the same as `Standalone`. Checked last, after every
    // location-based rule above (a Windows pip/pipx/asdf/pyenv/mise entry
    // point is itself a PE launcher shim, i.e. `NativeBinary`, so those
    // rules must get a chance to match first).
    if kind == FileKind::NativeBinary {
        return PathDiscovery {
            channel: Some(Channel::Unclaimed),
            ..Default::default()
        };
    }

    PathDiscovery::default()
}

/// Location of the virtual/Conda environment `resolved` was launched from,
/// independent of which channel rule (if any) matched.
fn venv_root(ctx: &DiscoveryContext, resolved: &Path) -> Option<String> {
    if let Some(virtual_env) = (ctx.env)("VIRTUAL_ENV") {
        let venv_bin =
            PathBuf::from(&virtual_env).join(if cfg!(windows) { "Scripts" } else { "bin" });
        if resolved.starts_with(&venv_bin) {
            return Some(virtual_env);
        }
    }
    if let Some(conda_prefix) = (ctx.env)("CONDA_PREFIX") {
        let conda_bin =
            PathBuf::from(&conda_prefix).join(if cfg!(windows) { "Scripts" } else { "bin" });
        if resolved.starts_with(&conda_bin) {
            return Some(conda_prefix);
        }
    }
    None
}

fn manager_from_manifest_signals(cwd: &Path) -> Option<PythonPackageManager> {
    const SIGNALS: [(&str, PythonPackageManager); 8] = [
        ("uv.lock", PythonPackageManager::Uv),
        ("poetry.lock", PythonPackageManager::Poetry),
        ("pdm.lock", PythonPackageManager::Pdm),
        ("Pipfile.lock", PythonPackageManager::Pipenv),
        ("environment.yml", PythonPackageManager::Conda),
        ("environment.yaml", PythonPackageManager::Conda),
        ("requirements.txt", PythonPackageManager::Pip),
        ("setup.cfg", PythonPackageManager::Pip),
    ];
    for dir in cwd.ancestors() {
        for (file_name, manager) in SIGNALS {
            if dir.join(file_name).is_file() {
                return Some(manager);
            }
        }
    }
    None
}

/// Package managers checked by the presence probes below, in priority order:
/// whichever's executable is found and runs first wins.
const PACKAGE_MANAGER_CANDIDATES: [(&str, PythonPackageManager); 9] = [
    ("uv", PythonPackageManager::Uv),
    ("pipx", PythonPackageManager::Pipx),
    ("poetry", PythonPackageManager::Poetry),
    ("pdm", PythonPackageManager::Pdm),
    ("pipenv", PythonPackageManager::Pipenv),
    ("conda", PythonPackageManager::Conda),
    ("hatch", PythonPackageManager::Hatch),
    ("rye", PythonPackageManager::Rye),
    ("pip", PythonPackageManager::Pip),
];

/// Shared loop behind [`probe_package_manager`] and
/// [`probe_manager_for_manifest`]: whichever `candidates` entry's command
/// runs successfully first, via a global `PATH` search, wins.
fn probe_candidates(
    ctx: &DiscoveryContext,
    candidates: &[(&str, PythonPackageManager)],
) -> Option<PythonPackageManager> {
    for (command, manager) in candidates {
        if let Some(output) = (ctx.run)(command, &["--version"]) {
            if output.success {
                return Some(*manager);
            }
        }
    }
    None
}

/// Shared loop behind [`probe_package_manager_in_venv`] and
/// [`probe_manager_for_manifest`]: like [`probe_candidates`], but only for
/// an executable living inside `venv_bin` specifically, never a `PATH`
/// search.
fn probe_candidates_in_venv(
    ctx: &DiscoveryContext,
    venv_bin: &Path,
    candidates: &[(&str, PythonPackageManager)],
) -> Option<PythonPackageManager> {
    for (command, manager) in candidates {
        let exe_name = if cfg!(windows) {
            format!("{command}.exe")
        } else {
            command.to_string()
        };
        let exe = venv_bin.join(exe_name);
        if let Some(output) = (ctx.run)(&exe.to_string_lossy(), &["--version"]) {
            if output.success {
                return Some(*manager);
            }
        }
    }
    None
}

/// Presence probe of installed package managers, searched via `PATH` (i.e.
/// `run` resolves each bare command name itself). Used only when there's no
/// venv/conda env to scope the search to -- see `probe_package_manager_in_venv`
/// for the scoped equivalent.
fn probe_package_manager(ctx: &DiscoveryContext) -> Option<PythonPackageManager> {
    probe_candidates(ctx, &PACKAGE_MANAGER_CANDIDATES)
}

/// Presence probe scoped to `venv_bin` (a venv/conda-env's `bin`/`Scripts`
/// dir): checks the same candidates, in the same order, but only for an
/// executable living inside that specific environment -- never a `PATH`
/// search. A global search would credit whichever unrelated package manager
/// (e.g. `uv`) happens to be installed and runnable elsewhere on the
/// machine, even though it never touched this venv, and hand back an
/// uninstall command for a tool that never installed dbt.
fn probe_package_manager_in_venv(
    ctx: &DiscoveryContext,
    venv_bin: &Path,
) -> Option<PythonPackageManager> {
    probe_candidates_in_venv(ctx, venv_bin, &PACKAGE_MANAGER_CANDIDATES)
}

/// Presence probe for a *project's* package manager, scoped to
/// `manifest_dir` and narrowed to managers compatible with `format` --
/// independent of how the running `dbt` binary itself was installed.
///
/// [`resolve_package_manager`] (and the `dist_info.py_package_manager` hint
/// derived from it, in turn fed into `dist::resolve_manager_for_manifest`)
/// answers a different question: which manager installed *dbt's own
/// binary*. That's frequently `None`, or simply irrelevant to a project's
/// dependencies, when dbt ships as a standalone binary placed directly onto
/// `PATH` (the common case for the proprietary dbt v2 build) -- there's no venv/tool-dir signal to
/// derive a hint from at all, even though `manifest_dir` obviously has
/// *some* real package manager governing it. This probes that directory
/// directly instead, as a fallback once every other signal
/// (`resolve_manager_for_manifest`'s lockfile check and the `dist_info`
/// hint) has come up empty.
///
/// Checks the same candidates as [`probe_package_manager`], in the same
/// priority order, but narrowed with
/// [`PythonPackageManager::is_compatible_with`] first -- so a machine with
/// several managers installed can't return one that doesn't even manage
/// projects in this manifest's format (e.g. `uv` for a `requirements.txt`
/// project). Scoped first to a project-local `.venv`/`venv` directory if one
/// exists, then falls back to a global `PATH` search.
pub(crate) fn probe_manager_for_manifest(
    ctx: &DiscoveryContext,
    manifest_dir: &Path,
    format: PythonManifestFormat,
) -> Option<PythonPackageManager> {
    let candidates: Vec<(&str, PythonPackageManager)> = PACKAGE_MANAGER_CANDIDATES
        .into_iter()
        .filter(|(_, manager)| manager.is_compatible_with(format))
        .collect();

    for venv_name in [".venv", "venv"] {
        let venv_bin =
            manifest_dir
                .join(venv_name)
                .join(if cfg!(windows) { "Scripts" } else { "bin" });
        if let Some(manager) = probe_candidates_in_venv(ctx, &venv_bin, &candidates) {
            return Some(manager);
        }
    }

    probe_candidates(ctx, &candidates)
}

/// Resolves the Python package manager for a `pypi`-channel install, in
/// order: a hint already implied by the install location (e.g. a `uv tool`
/// dir), then managed-project manifest/lockfile signals, then a presence
/// probe.
///
/// The probe is scoped to the venv/conda env the install lives in, when
/// there is one, and deliberately does *not* fall back to a global `PATH`
/// search if nothing turns up there: a plain `python -m venv` + `pip install
/// dbt` env has no path-based hint and no manifest, and once its own `pip`
/// is found there's no reason to keep looking elsewhere. Conversely, an
/// env with no package-manager executable of its own (e.g. a bare `uv venv`
/// that dbt was `uv pip install`-ed into) has no name to report -- printing
/// a wrong-but-confident uninstall command for whatever unrelated manager
/// happens to be on `PATH` is worse than admitting we don't know, so callers
/// see `None` and fall back to a generic "uninstall it with whatever
/// installed it" message instead.
fn resolve_package_manager(
    ctx: &DiscoveryContext,
    cwd: &Path,
    venv_bin: Option<&Path>,
    hint: Option<PythonPackageManager>,
) -> Option<PythonPackageManager> {
    if let Some(manager) = hint.or_else(|| manager_from_manifest_signals(cwd)) {
        return Some(manager);
    }
    match venv_bin {
        Some(bin) => probe_package_manager_in_venv(ctx, bin),
        None => probe_package_manager(ctx),
    }
}

/// Fields discoverable from a `dbt` executable's path alone plus its
/// prerelease status, independent of `channel`/`distribution_override`
/// (which the caller must already have from `discover_channel` to decide
/// whether a version probe is even needed).
struct DiscoveredDistFields {
    py_package_manager: Option<PythonPackageManager>,
    py_venv_root: Option<String>,
    upgrade_cmd: Option<String>,
    uninstall_cmd: Option<String>,
}

/// The shared core behind both `get_current` and the legacy-dbt fallback, so
/// the two can never drift: venv root, package-manager resolution, and
/// upgrade/uninstall command generation, given an already-resolved
/// `path_discovery` (so callers that need to probe a version in between
/// `discover_channel` and this call don't pay for a second `discover_channel`
/// invocation -- notably a second round of Homebrew subprocess calls).
fn resolve_dist_fields(
    ctx: &DiscoveryContext,
    cwd: &Path,
    resolved: &Path,
    path_discovery: &PathDiscovery,
    is_prerelease: bool,
) -> DiscoveredDistFields {
    let py_venv_root = venv_root(ctx, resolved);
    let venv_bin = py_venv_root
        .as_ref()
        .map(|root| PathBuf::from(root).join(if cfg!(windows) { "Scripts" } else { "bin" }));
    let py_package_manager = if path_discovery.channel == Some(Channel::Pypi) {
        resolve_package_manager(
            ctx,
            cwd,
            venv_bin.as_deref(),
            path_discovery.py_package_manager_hint,
        )
    } else {
        None
    };
    let (upgrade_cmd, uninstall_cmd) =
        path_discovery.command_strings(py_package_manager, is_prerelease);
    DiscoveredDistFields {
        py_package_manager,
        py_venv_root,
        upgrade_cmd,
        uninstall_cmd,
    }
}

fn current_cwd() -> PathBuf {
    env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Whether `version` is a pre-release: anything containing characters other
/// than digits and `.` (dbt's own `X.Y.Z-preview.N` scheme, or a PEP 440
/// `rc`/`a`/`b`/`.dev`/`+build` suffix on a legacy `dbt-core` release) is
/// treated as one. A version made of digits and dots only (`2.0.0`) is
/// stable.
fn is_prerelease_version(version: &str) -> bool {
    !version.chars().all(|c| c.is_ascii_digit() || c == '.')
}

fn get_current(command_name: &str) -> FsResult<DistInfo> {
    const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

    let exe_path = env::current_exe().map_err(|e| {
        fs_err!(ErrorCode::IoError, "Failed to get current executable")
            .with_cause(WrappedError::Io(e))
    })?;

    let ctx = DiscoveryContext::real();
    let cwd = current_cwd();
    let ClassifiedPath {
        given,
        resolved,
        kind,
    } = classify_path(&exe_path);
    let path_discovery = discover_channel(&ctx, &given, &resolved, kind);
    let is_prerelease = is_prerelease_version(CURRENT_VERSION);
    let fields = resolve_dist_fields(&ctx, &cwd, &resolved, &path_discovery, is_prerelease);

    Ok(DistInfo {
        path: resolved.to_string_lossy().into_owned(),
        channel: path_discovery.channel,
        distribution: path_discovery
            .distribution_override
            .or_else(|| Some(dbt_dist_classify::distribution_from_name(command_name))),
        generation: Generation::V2,
        py_package_manager: fields.py_package_manager,
        py_venv_root: fields.py_venv_root,
        version: Some(CURRENT_VERSION.to_string()),
        is_prerelease: Some(is_prerelease),
        upgrade_cmd: fields.upgrade_cmd,
        uninstall_cmd: fields.uninstall_cmd,
    })
}

/// Upper bound on how long a single on-PATH `dbt` is given to answer
/// discovery probes before its result is given up on. `get_at_path` can make
/// up to two sequential subprocess calls per install (the native
/// `get-distribution-info` introspection, then a `--version` fallback), each
/// individually bounded by `NORMAL_WAIT + GRACE_WAIT` in `real_run` — this
/// needs enough headroom to cover both worst cases plus a little slack for
/// filesystem probing, rather than cutting a well-behaved second probe off
/// mid-flight because the first one had to be killed.
const DISCOVERY_TIMEOUT: Duration =
    Duration::from_secs(2 * (NORMAL_WAIT.as_secs() + GRACE_WAIT.as_secs()) + 5);

fn get_all_in_path(command_name: &str) -> FsResult<Vec<DistInfo>> {
    discover_all_in_path_value(env::var_os("PATH").as_deref(), command_name)
}

fn discover_all_in_path_value(
    path_var: Option<&std::ffi::OsStr>,
    command_name: &str,
) -> FsResult<Vec<DistInfo>> {
    let Some(path_var) = path_var else {
        return Ok(vec![]);
    };

    let exe_names: &[&str] = if cfg!(windows) {
        &["dbt.exe", "dbt"]
    } else {
        &["dbt"]
    };

    let mut seen = HashSet::new();
    let mut resolved_paths: Vec<PathBuf> = Vec::new();
    for dir in env::split_paths(path_var) {
        for name in exe_names {
            let candidate = dir.join(name);
            if is_executable(&candidate) {
                let key = dbt_common::stdfs::canonicalize(&candidate)
                    .unwrap_or_else(|_| candidate.clone());
                if seen.insert(key) {
                    resolved_paths.push(candidate);
                }
                break;
            }
        }
    }

    // Each resolved path is discovered independently (its own subprocess
    // spawns, its own filesystem probes), so run them concurrently rather
    // than paying for the sum of every install's discovery time instead of
    // just the slowest one.
    //
    // These are detached threads rather than scoped ones: a `dbt` found on
    // PATH is an arbitrary, possibly badly-behaved program, and a scoped
    // thread must be joined before the scope can exit, so a single hung
    // subprocess would block discovery forever. Detached threads let us give
    // up on a slow probe after DISCOVERY_TIMEOUT and move on; the abandoned
    // thread (and its subprocess, if still running) is left to finish on its
    // own rather than taking the whole process down with it.
    let receivers: Vec<mpsc::Receiver<FsResult<DistInfo>>> = resolved_paths
        .into_iter()
        .map(|path| {
            let command_name = command_name.to_string();
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(get_at_path(&path, &command_name));
            });
            rx
        })
        .collect();

    receivers
        .into_iter()
        .filter_map(|rx| rx.recv_timeout(DISCOVERY_TIMEOUT).ok())
        .collect()
}

/// Whether `file_path` resolves to the currently running executable.
///
/// A `get-distribution-info` invocation whose target is itself must be
/// short-circuited to [`get_current`] rather than spawned as a subprocess:
/// spawning would run the exact same command again, which would in turn
/// detect that its own target is itself and spawn again, forever.
fn is_current_executable(file_path: &Path) -> bool {
    let Ok(current) = env::current_exe() else {
        return false;
    };
    let current = dbt_common::stdfs::canonicalize(&current).unwrap_or(current);
    let target =
        dbt_common::stdfs::canonicalize(file_path).unwrap_or_else(|_| file_path.to_path_buf());
    current == target
}

fn get_at_path(file_path: &Path, command_name: &str) -> FsResult<DistInfo> {
    if !is_executable(file_path) {
        return err!(
            ErrorCode::FileNotFound,
            "File not found or is not executable: {}",
            file_path.to_str().unwrap_or("<invalid path>")
        );
    }

    if is_current_executable(file_path) {
        return get_current(command_name);
    }

    let ctx = DiscoveryContext::real();
    let classified = classify_path(file_path);

    // Only a native v2 binary can possibly understand `internal
    // get-distribution-info`; a script (pip/pipx/asdf/pyenv/... install) is
    // guaranteed to reject it, so skip straight to the legacy `--version`
    // fallback rather than paying for a doomed subprocess spawn — for a
    // Python-based `dbt-core` this roughly halves the interpreter-startup
    // cost paid per PATH entry.
    if classified.kind == FileKind::NativeBinary {
        if let Some(dist_info) = introspect_for_dist_info(&ctx, file_path) {
            return Ok(dist_info);
        }
    }

    discover_dist_info_from_legacy_dbt(&ctx, classified)
}

// Given a path to a dbt executable, try running `dbt internal
// get-distribution-info`, and if it succeeds, parse the result. Any
// failures (not found, non-zero exit, malformed JSON) are treated as None.
fn introspect_for_dist_info(ctx: &DiscoveryContext, file_path: &Path) -> Option<DistInfo> {
    let stdout = run_dbt_internal(ctx, file_path)?;
    parse_dist_info_json(&stdout)
}

fn run_dbt_internal(ctx: &DiscoveryContext, file_path: &Path) -> Option<String> {
    let path_str = file_path.to_str()?;
    // Passing the target path explicitly (rather than relying on the
    // child's own arg0) is what lets a freshly downloaded `dbt` classify a
    // different on-path install.
    let output = (ctx.run)(path_str, &["internal", "get-distribution-info", path_str])?;
    output.success.then_some(output.stdout)
}

fn parse_dist_info_json(stdout: &str) -> Option<DistInfo> {
    serde_json::from_str(stdout).ok()
}

fn discover_dist_info_from_legacy_dbt(
    ctx: &DiscoveryContext,
    classified: ClassifiedPath,
) -> FsResult<DistInfo> {
    let cwd = current_cwd();
    let ClassifiedPath {
        given,
        resolved,
        kind,
    } = classified;
    // Resolve the channel first: the version probe below is skipped once a
    // Homebrew-tap conflict already names a distribution, and `is_prerelease`
    // (needed by `command_strings`, called inside `resolve_dist_fields`) can
    // only be computed once that probe's version -- or lack of one -- is
    // known. Calling `discover_channel` a second time inside
    // `resolve_dist_fields` would double any Homebrew subprocess calls it
    // makes, so it's called here once and threaded through instead.
    let path_discovery = discover_channel(ctx, &given, &resolved, kind);

    // A Homebrew-tap conflict (detected via the path alone, above) already
    // tells us the distribution — e.g. a non-brew-installed dbt Cloud CLI
    // could otherwise print a `--version` line that spuriously parses as a
    // v2 banner. Skip the probe entirely once the path resolver has an
    // answer, rather than letting it clobber `generation` with a bogus
    // reading for a program that was never a v1/v2 `dbt` in the first place.
    let version_probe = path_discovery
        .distribution_override
        .is_none()
        .then(|| probe_generation_and_distribution(ctx, &given))
        .flatten();
    let generation = version_probe
        .as_ref()
        .map_or(Generation::NotApplicable, |(generation, _, _)| *generation);
    let distribution = path_discovery.distribution_override.or_else(|| {
        version_probe
            .as_ref()
            .map(|(_, distribution, _)| *distribution)
    });
    let version = version_probe.and_then(|(_, _, version)| version);
    let is_prerelease = version.as_deref().map(is_prerelease_version);

    let fields = resolve_dist_fields(
        ctx,
        &cwd,
        &resolved,
        &path_discovery,
        is_prerelease.unwrap_or(false),
    );

    Ok(DistInfo {
        path: resolved.to_string_lossy().into_owned(),
        channel: path_discovery.channel,
        distribution,
        generation,
        py_package_manager: fields.py_package_manager,
        py_venv_root: fields.py_venv_root,
        version,
        is_prerelease,
        upgrade_cmd: fields.upgrade_cmd,
        uninstall_cmd: fields.uninstall_cmd,
    })
}

/// When a `dbt` doesn't support `dbt internal get-distribution-info` (a
/// legacy v1 Python `dbt-core`, or a v2 binary that predates the plumbing
/// command), fall back to `<dbt> --version` to recover a generation and
/// distribution signal.
///
/// v1 prints a multi-line `Core:\n  - installed: ...` block and is always
/// the legacy `dbt-core` distribution. v2 prints a single `<name> <version>`
/// banner line (clap's default `--version` format): the OSS v2 build
/// (`dbt-sa-cli`) will brand itself `dbt-oss`, but `dbt-core` is still
/// checked for too since preview builds already installed print that name;
/// everything else is treated as the proprietary distribution.
///
/// Callers should only reach for this once the path-based channel resolver
/// couldn't already name a distribution outright (e.g. a Homebrew-tap
/// conflict identifying the dbt Cloud CLI) — that binary isn't a v1/v2 `dbt`
/// at all, and its own `--version` output isn't something this function
/// knows how to interpret.
fn probe_generation_and_distribution(
    ctx: &DiscoveryContext,
    file_path: &Path,
) -> Option<(Generation, Distribution, Option<String>)> {
    let path_str = file_path.to_str()?;
    let output = (ctx.run)(path_str, &["--version"])?;
    if !output.success {
        return None;
    }
    classify_version_output(&output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn no_run(_: &str, _: &[&str]) -> Option<ProcessOutput> {
        None
    }

    fn env_from(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    // ---- sniff_file_kind ----

    #[test]
    fn sniffs_elf_as_native_binary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dbt");
        std::fs::write(&path, [0x7f, b'E', b'L', b'F', 0, 0, 0, 0]).unwrap();
        assert_eq!(sniff_file_kind(&path), FileKind::NativeBinary);
    }

    #[test]
    fn sniffs_macho_as_native_binary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dbt");
        std::fs::write(&path, [0xCF, 0xFA, 0xED, 0xFE, 0, 0, 0, 0]).unwrap();
        assert_eq!(sniff_file_kind(&path), FileKind::NativeBinary);
    }

    #[test]
    fn sniffs_pe_as_native_binary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dbt.exe");
        std::fs::write(&path, [b'M', b'Z', 0, 0]).unwrap();
        assert_eq!(sniff_file_kind(&path), FileKind::NativeBinary);
    }

    #[test]
    fn sniffs_shebang_as_script() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dbt");
        std::fs::write(&path, b"#!/usr/bin/env python\nprint('hi')\n").unwrap();
        assert_eq!(sniff_file_kind(&path), FileKind::Script);
    }

    #[test]
    fn sniffs_empty_file_as_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dbt");
        std::fs::write(&path, []).unwrap();
        assert_eq!(sniff_file_kind(&path), FileKind::Unknown);
    }

    #[test]
    fn sniffs_missing_file_as_unknown() {
        let path = PathBuf::from("/nonexistent/path/dbt");
        assert_eq!(sniff_file_kind(&path), FileKind::Unknown);
    }

    // ---- is_executable ----

    #[test]
    #[cfg(unix)]
    fn is_executable_true_when_exec_bit_set() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dbt");
        std::fs::write(&path, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(is_executable(&path));
    }

    #[test]
    #[cfg(unix)]
    fn is_executable_false_when_exec_bit_unset() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dbt");
        std::fs::write(&path, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!is_executable(&path));
    }

    #[test]
    fn is_executable_false_for_missing_file() {
        assert!(!is_executable(Path::new("/nonexistent/path/dbt")));
    }

    // ---- classify_path ----

    #[test]
    #[cfg(unix)]
    fn classify_path_resolves_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real-dbt");
        std::fs::write(&target, [0x7f, b'E', b'L', b'F']).unwrap();
        let link = dir.path().join("dbt");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let classified = classify_path(&link);
        assert_eq!(classified.given, link);
        assert_eq!(
            classified.resolved,
            dbt_common::stdfs::canonicalize(&target).unwrap()
        );
        assert_eq!(classified.kind, FileKind::NativeBinary);
    }

    #[test]
    fn classify_path_falls_back_to_given_for_dangling_path() {
        let path = PathBuf::from("/nonexistent/path/dbt");
        let classified = classify_path(&path);
        assert_eq!(classified.resolved, path);
        assert_eq!(classified.kind, FileKind::Unknown);
    }

    // ---- discover_channel ----

    #[test]
    fn standalone_native_binary_at_local_bin() {
        let map = env_from(&[("HOME", "/home/user")]);
        let env = |n: &str| map.get(n).cloned();
        let ctx = DiscoveryContext {
            env: &env,
            run: &no_run,
        };
        let path = PathBuf::from("/home/user/.local/bin/dbt");
        let result = discover_channel(&ctx, &path, &path, FileKind::NativeBinary);
        assert_eq!(result.channel, Some(Channel::Standalone));
    }

    #[test]
    fn pip_user_script_at_local_bin_is_not_standalone() {
        let map = env_from(&[("HOME", "/home/user")]);
        let env = |n: &str| map.get(n).cloned();
        let ctx = DiscoveryContext {
            env: &env,
            run: &no_run,
        };
        let path = PathBuf::from("/home/user/.local/bin/dbt");
        let result = discover_channel(&ctx, &path, &path, FileKind::Script);
        assert_eq!(result.channel, Some(Channel::Pypi));
    }

    #[test]
    fn brew_dbt_labs_dbt_tap_resolves_brew_channel() {
        let map = env_from(&[("HOME", "/home/user")]);
        let env = |n: &str| map.get(n).cloned();
        let run = |cmd: &str, args: &[&str]| -> Option<ProcessOutput> {
            if cmd != "brew" {
                return None;
            }
            if matches!(args, ["--prefix"]) {
                return Some(ProcessOutput {
                    success: true,
                    stdout: "/opt/homebrew\n".to_string(),
                    stderr: String::new(),
                });
            }
            if matches!(args, ["info", "dbt", "--json=v2"]) {
                return Some(ProcessOutput {
                    success: true,
                    stdout: r#"{"formulae":[{"tap":"dbt-labs/dbt"}]}"#.to_string(),
                    stderr: String::new(),
                });
            }
            None
        };
        let ctx = DiscoveryContext {
            env: &env,
            run: &run,
        };
        let path = PathBuf::from("/opt/homebrew/bin/dbt");
        let result = discover_channel(&ctx, &path, &path, FileKind::NativeBinary);
        assert_eq!(result.channel, Some(Channel::Brew));
        assert_eq!(result.distribution_override, None);
    }

    #[test]
    fn brew_dbt_cli_tap_resolves_cloud_cli_override() {
        let map = env_from(&[("HOME", "/home/user")]);
        let env = |n: &str| map.get(n).cloned();
        let run = |cmd: &str, args: &[&str]| -> Option<ProcessOutput> {
            if cmd != "brew" {
                return None;
            }
            if matches!(args, ["--prefix"]) {
                return Some(ProcessOutput {
                    success: true,
                    stdout: "/opt/homebrew\n".to_string(),
                    stderr: String::new(),
                });
            }
            if matches!(args, ["info", "dbt", "--json=v2"]) {
                return Some(ProcessOutput {
                    success: true,
                    stdout: r#"{"formulae":[{"tap":"dbt-labs/dbt-cli"}]}"#.to_string(),
                    stderr: String::new(),
                });
            }
            None
        };
        let ctx = DiscoveryContext {
            env: &env,
            run: &run,
        };
        let path = PathBuf::from("/opt/homebrew/bin/dbt");
        let result = discover_channel(&ctx, &path, &path, FileKind::NativeBinary);
        assert_eq!(result.channel, None);
        assert_eq!(result.distribution_override, Some(Distribution::CloudCLI));
    }

    #[test]
    fn brew_untrusted_tap_error_resolves_cloud_cli_override() {
        let map = env_from(&[("HOME", "/home/user")]);
        let env = |n: &str| map.get(n).cloned();
        let run = |cmd: &str, args: &[&str]| -> Option<ProcessOutput> {
            if cmd != "brew" {
                return None;
            }
            if matches!(args, ["--prefix"]) {
                return Some(ProcessOutput {
                    success: true,
                    stdout: "/opt/homebrew\n".to_string(),
                    stderr: String::new(),
                });
            }
            if matches!(args, ["info", "dbt", "--json=v2"]) {
                return Some(ProcessOutput {
                    success: false,
                    stdout: String::new(),
                    stderr: "Error: Refusing to load formula dbt-labs/dbt-cli/dbt from untrusted tap dbt-labs/dbt-cli".to_string(),
                });
            }
            None
        };
        let ctx = DiscoveryContext {
            env: &env,
            run: &run,
        };
        let path = PathBuf::from("/opt/homebrew/bin/dbt");
        let result = discover_channel(&ctx, &path, &path, FileKind::NativeBinary);
        assert_eq!(result.channel, None);
        assert_eq!(result.distribution_override, Some(Distribution::CloudCLI));
    }

    #[test]
    fn brew_prefix_with_unrecognized_tap_falls_back_to_brew() {
        let map = env_from(&[("HOME", "/home/user")]);
        let env = |n: &str| map.get(n).cloned();
        let run = |cmd: &str, args: &[&str]| -> Option<ProcessOutput> {
            if cmd != "brew" {
                return None;
            }
            if matches!(args, ["--prefix"]) {
                return Some(ProcessOutput {
                    success: true,
                    stdout: "/opt/homebrew\n".to_string(),
                    stderr: String::new(),
                });
            }
            if matches!(args, ["info", "dbt", "--json=v2"]) {
                return Some(ProcessOutput {
                    success: true,
                    stdout: r#"{"formulae":[{"tap":"some-other/tap"}]}"#.to_string(),
                    stderr: String::new(),
                });
            }
            None
        };
        let ctx = DiscoveryContext {
            env: &env,
            run: &run,
        };
        let path = PathBuf::from("/opt/homebrew/bin/dbt");
        let result = discover_channel(&ctx, &path, &path, FileKind::NativeBinary);
        assert_eq!(result.channel, Some(Channel::Brew));
    }

    #[test]
    fn brew_not_on_path_falls_back_to_cellar_path_match() {
        let map = env_from(&[("HOME", "/home/user")]);
        let env = |n: &str| map.get(n).cloned();
        // Simulates `brew` not being resolvable on PATH at all: every
        // invocation returns `None`, exactly like `real_run` does for a
        // program that can't be found/spawned.
        let ctx = DiscoveryContext {
            env: &env,
            run: &no_run,
        };
        let path = PathBuf::from("/opt/homebrew/Cellar/dbt/2.0.0/bin/dbt");
        let result = discover_channel(&ctx, &path, &path, FileKind::NativeBinary);
        assert_eq!(result.channel, Some(Channel::Brew));
        assert_eq!(result.distribution_override, None);
    }

    #[test]
    fn brew_not_on_path_and_not_under_cellar_still_falls_through_to_unclaimed() {
        let map = env_from(&[("HOME", "/home/user")]);
        let env = |n: &str| map.get(n).cloned();
        let ctx = DiscoveryContext {
            env: &env,
            run: &no_run,
        };
        let path = PathBuf::from("/usr/local/other/dbt");
        let result = discover_channel(&ctx, &path, &path, FileKind::NativeBinary);
        assert_eq!(result.channel, Some(Channel::Unclaimed));
    }

    #[test]
    fn not_under_homebrew_prefix_does_not_resolve_brew() {
        let map = env_from(&[("HOME", "/home/user")]);
        let env = |n: &str| map.get(n).cloned();
        let run = |cmd: &str, args: &[&str]| -> Option<ProcessOutput> {
            if cmd == "brew" && matches!(args, ["--prefix"]) {
                return Some(ProcessOutput {
                    success: true,
                    stdout: "/opt/homebrew\n".to_string(),
                    stderr: String::new(),
                });
            }
            None
        };
        let ctx = DiscoveryContext {
            env: &env,
            run: &run,
        };
        let path = PathBuf::from("/usr/local/other/dbt");
        let result = discover_channel(&ctx, &path, &path, FileKind::NativeBinary);
        assert_eq!(result.channel, Some(Channel::Unclaimed));
    }

    #[test]
    fn winget_links_dir_resolves_winget_channel() {
        let map = env_from(&[("HOME", "/home/user")]);
        let env = |n: &str| map.get(n).cloned();
        let ctx = DiscoveryContext {
            env: &env,
            run: &no_run,
        };
        let path = PathBuf::from(r"C:\Users\user\AppData\Local\Microsoft\WinGet\Links\dbt.exe");
        let result = discover_channel(&ctx, &path, &path, FileKind::NativeBinary);
        assert_eq!(result.channel, Some(Channel::Winget));
    }

    #[test]
    fn winget_packages_dir_resolves_winget_channel() {
        let map = env_from(&[("HOME", "/home/user")]);
        let env = |n: &str| map.get(n).cloned();
        let ctx = DiscoveryContext {
            env: &env,
            run: &no_run,
        };
        let given = PathBuf::from(r"C:\Users\user\AppData\Local\Microsoft\WinGet\Links\dbt.exe");
        let resolved = PathBuf::from(
            r"C:\Users\user\AppData\Local\Microsoft\WinGet\Packages\dbtLabs.dbt_abc123\dbt.exe",
        );
        let result = discover_channel(&ctx, &given, &resolved, FileKind::NativeBinary);
        assert_eq!(result.channel, Some(Channel::Winget));
    }

    #[test]
    fn scoop_apps_dir_resolves_unsupported_channel() {
        let map = env_from(&[("HOME", "/home/user")]);
        let env = |n: &str| map.get(n).cloned();
        let ctx = DiscoveryContext {
            env: &env,
            run: &no_run,
        };
        let path = PathBuf::from(r"C:\Users\user\scoop\apps\dbt\current\dbt.exe");
        let result = discover_channel(&ctx, &path, &path, FileKind::NativeBinary);
        assert_eq!(
            result.channel,
            Some(Channel::Unsupported("Scoop".to_string()))
        );
    }

    #[test]
    fn scoop_shims_dir_resolves_unsupported_channel() {
        let map = env_from(&[("HOME", "/home/user")]);
        let env = |n: &str| map.get(n).cloned();
        let ctx = DiscoveryContext {
            env: &env,
            run: &no_run,
        };
        let path = PathBuf::from(r"C:\Users\user\scoop\shims\dbt.exe");
        let result = discover_channel(&ctx, &path, &path, FileKind::NativeBinary);
        assert_eq!(
            result.channel,
            Some(Channel::Unsupported("Scoop".to_string()))
        );
    }

    #[test]
    fn chocolatey_dir_resolves_unsupported_channel() {
        let map = env_from(&[("HOME", "/home/user")]);
        let env = |n: &str| map.get(n).cloned();
        let ctx = DiscoveryContext {
            env: &env,
            run: &no_run,
        };
        let path = PathBuf::from(r"C:\ProgramData\chocolatey\bin\dbt.exe");
        let result = discover_channel(&ctx, &path, &path, FileKind::NativeBinary);
        assert_eq!(
            result.channel,
            Some(Channel::Unsupported("Chocolatey".to_string()))
        );
    }

    #[test]
    fn pipx_dir_resolves_pypi_with_pipx_manager() {
        let map = env_from(&[("HOME", "/home/user")]);
        let env = |n: &str| map.get(n).cloned();
        let ctx = DiscoveryContext {
            env: &env,
            run: &no_run,
        };
        let given = PathBuf::from("/home/user/.local/bin/dbt");
        let resolved = PathBuf::from("/home/user/.local/pipx/venvs/dbt/bin/dbt");
        let result = discover_channel(&ctx, &given, &resolved, FileKind::Script);
        assert_eq!(result.channel, Some(Channel::Pypi));
        assert_eq!(
            result.py_package_manager_hint,
            Some(PythonPackageManager::Pipx)
        );
    }

    #[test]
    fn uv_tool_dir_resolves_pypi_with_uv_manager() {
        let map = env_from(&[("HOME", "/home/user")]);
        let env = |n: &str| map.get(n).cloned();
        let ctx = DiscoveryContext {
            env: &env,
            run: &no_run,
        };
        let path = PathBuf::from("/home/user/.local/share/uv/tools/dbt/bin/dbt");
        let result = discover_channel(&ctx, &path, &path, FileKind::Script);
        assert_eq!(result.channel, Some(Channel::Pypi));
        assert_eq!(
            result.py_package_manager_hint,
            Some(PythonPackageManager::Uv)
        );
    }

    #[test]
    fn uv_tool_dir_native_binary_still_resolves_pypi_not_unclaimed() {
        // A Windows uv-tool/pipx/asdf/pyenv/mise entry point is a PE launcher
        // shim, which `sniff_file_kind` classifies as `NativeBinary` just
        // like a real standalone/brew/winget binary. The tool-dir location
        // rules must still win over the `Unclaimed` native-binary fallback.
        let map = env_from(&[("HOME", "/home/user")]);
        let env = |n: &str| map.get(n).cloned();
        let ctx = DiscoveryContext {
            env: &env,
            run: &no_run,
        };
        let path = PathBuf::from("/home/user/.local/share/uv/tools/dbt/bin/dbt.exe");
        let result = discover_channel(&ctx, &path, &path, FileKind::NativeBinary);
        assert_eq!(result.channel, Some(Channel::Pypi));
        assert_eq!(
            result.py_package_manager_hint,
            Some(PythonPackageManager::Uv)
        );
    }

    #[test]
    fn rye_tools_dir_resolves_pypi_with_rye_manager() {
        let map = env_from(&[("HOME", "/home/user")]);
        let env = |n: &str| map.get(n).cloned();
        let ctx = DiscoveryContext {
            env: &env,
            run: &no_run,
        };
        let path = PathBuf::from("/home/user/.rye/tools/dbt/bin/dbt");
        let result = discover_channel(&ctx, &path, &path, FileKind::Script);
        assert_eq!(result.channel, Some(Channel::Pypi));
        assert_eq!(
            result.py_package_manager_hint,
            Some(PythonPackageManager::Rye)
        );
    }

    #[test]
    fn asdf_shim_resolves_pypi_with_asdf_manager() {
        let map = env_from(&[("HOME", "/home/user")]);
        let env = |n: &str| map.get(n).cloned();
        let ctx = DiscoveryContext {
            env: &env,
            run: &no_run,
        };
        let path = PathBuf::from("/home/user/.asdf/shims/dbt");
        let result = discover_channel(&ctx, &path, &path, FileKind::Script);
        assert_eq!(result.channel, Some(Channel::Pypi));
        assert_eq!(
            result.py_package_manager_hint,
            Some(PythonPackageManager::Asdf)
        );
    }

    #[test]
    fn pyenv_shim_resolves_pypi_with_pyenv_manager() {
        let map = env_from(&[("HOME", "/home/user")]);
        let env = |n: &str| map.get(n).cloned();
        let ctx = DiscoveryContext {
            env: &env,
            run: &no_run,
        };
        let given = PathBuf::from("/home/user/.pyenv/shims/dbt");
        let resolved = PathBuf::from("/home/user/.pyenv/versions/3.12.0/bin/dbt");
        let result = discover_channel(&ctx, &given, &resolved, FileKind::Script);
        assert_eq!(result.channel, Some(Channel::Pypi));
        assert_eq!(
            result.py_package_manager_hint,
            Some(PythonPackageManager::Pyenv)
        );
    }

    #[test]
    fn mise_shim_resolves_pypi_with_mise_manager() {
        let map = env_from(&[("HOME", "/home/user")]);
        let env = |n: &str| map.get(n).cloned();
        let ctx = DiscoveryContext {
            env: &env,
            run: &no_run,
        };
        let path = PathBuf::from("/home/user/.local/share/mise/shims/dbt");
        let result = discover_channel(&ctx, &path, &path, FileKind::Script);
        assert_eq!(result.channel, Some(Channel::Pypi));
        assert_eq!(
            result.py_package_manager_hint,
            Some(PythonPackageManager::Mise)
        );
    }

    #[test]
    fn venv_script_resolves_pypi_without_manager_hint() {
        let map = env_from(&[
            ("HOME", "/home/user"),
            ("VIRTUAL_ENV", "/home/user/project/.venv"),
        ]);
        let env = |n: &str| map.get(n).cloned();
        let ctx = DiscoveryContext {
            env: &env,
            run: &no_run,
        };
        let path = PathBuf::from("/home/user/project/.venv/bin/dbt");
        let result = discover_channel(&ctx, &path, &path, FileKind::Script);
        assert_eq!(result.channel, Some(Channel::Pypi));
        assert_eq!(result.py_package_manager_hint, None);
    }

    #[test]
    fn generic_script_falls_back_to_pypi() {
        let map = env_from(&[("HOME", "/home/user")]);
        let env = |n: &str| map.get(n).cloned();
        let ctx = DiscoveryContext {
            env: &env,
            run: &no_run,
        };
        let path = PathBuf::from("/usr/local/bin/dbt");
        let result = discover_channel(&ctx, &path, &path, FileKind::Script);
        assert_eq!(result.channel, Some(Channel::Pypi));
    }

    #[test]
    fn unmatched_native_binary_resolves_to_unclaimed_channel() {
        let map = env_from(&[("HOME", "/home/user")]);
        let env = |n: &str| map.get(n).cloned();
        let ctx = DiscoveryContext {
            env: &env,
            run: &no_run,
        };
        let path = PathBuf::from("/opt/custom/dbt");
        let result = discover_channel(&ctx, &path, &path, FileKind::NativeBinary);
        assert_eq!(result.channel, Some(Channel::Unclaimed));
    }

    // ---- venv_root ----

    #[test]
    fn venv_root_set_when_inside_virtual_env() {
        let map = env_from(&[("VIRTUAL_ENV", "/home/user/project/.venv")]);
        let env = |n: &str| map.get(n).cloned();
        let ctx = DiscoveryContext {
            env: &env,
            run: &no_run,
        };
        let bin_dir = if cfg!(windows) { "Scripts" } else { "bin" };
        let resolved = PathBuf::from(format!("/home/user/project/.venv/{bin_dir}/dbt"));
        assert_eq!(
            venv_root(&ctx, &resolved),
            Some("/home/user/project/.venv".to_string())
        );
    }

    #[test]
    fn venv_root_none_when_outside_virtual_env() {
        let map = env_from(&[("VIRTUAL_ENV", "/home/user/project/.venv")]);
        let env = |n: &str| map.get(n).cloned();
        let ctx = DiscoveryContext {
            env: &env,
            run: &no_run,
        };
        let resolved = PathBuf::from("/usr/local/bin/dbt");
        assert_eq!(venv_root(&ctx, &resolved), None);
    }

    #[test]
    fn venv_root_set_for_conda_prefix() {
        let map = env_from(&[("CONDA_PREFIX", "/home/user/miniconda3/envs/proj")]);
        let env = |n: &str| map.get(n).cloned();
        let ctx = DiscoveryContext {
            env: &env,
            run: &no_run,
        };
        let bin_dir = if cfg!(windows) { "Scripts" } else { "bin" };
        let resolved = PathBuf::from(format!("/home/user/miniconda3/envs/proj/{bin_dir}/dbt"));
        assert_eq!(
            venv_root(&ctx, &resolved),
            Some("/home/user/miniconda3/envs/proj".to_string())
        );
    }

    // ---- resolve_package_manager ----

    #[test]
    fn hint_short_circuits_manager_resolution() {
        let map = env_from(&[]);
        let env = |n: &str| map.get(n).cloned();
        let ctx = DiscoveryContext {
            env: &env,
            run: &no_run,
        };
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("uv.lock"), "").unwrap();
        let manager =
            resolve_package_manager(&ctx, dir.path(), None, Some(PythonPackageManager::Pipx));
        assert_eq!(manager, Some(PythonPackageManager::Pipx));
    }

    #[test]
    fn manifest_signal_detected_from_ancestor_dir() {
        let map = env_from(&[]);
        let env = |n: &str| map.get(n).cloned();
        let ctx = DiscoveryContext {
            env: &env,
            run: &no_run,
        };
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("poetry.lock"), "").unwrap();
        let nested = dir.path().join("sub").join("dir");
        std::fs::create_dir_all(&nested).unwrap();
        let manager = resolve_package_manager(&ctx, &nested, None, None);
        assert_eq!(manager, Some(PythonPackageManager::Poetry));
    }

    #[test]
    fn conda_environment_yaml_signal_is_detected() {
        // `SIGNALS` must recognize the `.yaml` spelling of the conda manifest
        // filename, not just `.yml` -- otherwise a project whose only conda
        // signal is `environment.yaml` never resolves `py_package_manager`
        // to `Conda` at all, and the managed-project upgrade flow bails
        // before it can even compute a sync command.
        let map = env_from(&[]);
        let env = |n: &str| map.get(n).cloned();
        let ctx = DiscoveryContext {
            env: &env,
            run: &no_run,
        };
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("environment.yaml"), "").unwrap();
        let manager = resolve_package_manager(&ctx, dir.path(), None, None);
        assert_eq!(manager, Some(PythonPackageManager::Conda));
    }

    #[test]
    fn presence_probe_used_when_no_hint_or_manifest() {
        let map = env_from(&[]);
        let env = |n: &str| map.get(n).cloned();
        let run = |cmd: &str, _: &[&str]| -> Option<ProcessOutput> {
            if cmd == "pdm" {
                Some(ProcessOutput {
                    success: true,
                    stdout: String::new(),
                    stderr: String::new(),
                })
            } else {
                None
            }
        };
        let ctx = DiscoveryContext {
            env: &env,
            run: &run,
        };
        let dir = tempfile::tempdir().unwrap();
        let manager = resolve_package_manager(&ctx, dir.path(), None, None);
        assert_eq!(manager, Some(PythonPackageManager::Pdm));
    }

    #[test]
    fn presence_probe_returns_none_when_nothing_found() {
        let map = env_from(&[]);
        let env = |n: &str| map.get(n).cloned();
        let ctx = DiscoveryContext {
            env: &env,
            run: &no_run,
        };
        let dir = tempfile::tempdir().unwrap();
        let manager = resolve_package_manager(&ctx, dir.path(), None, None);
        assert_eq!(manager, None);
    }

    #[test]
    fn venv_scoped_pip_wins_over_unrelated_global_manager_presence() {
        // Reproduces the reported bug: a plain `pip install dbt` inside a
        // generic venv must resolve to Pip even when an unrelated `uv`
        // happens to be installed and runnable elsewhere on `PATH` -- `uv`
        // never touched this venv, so it must not win the probe.
        let map = env_from(&[]);
        let env = |n: &str| map.get(n).cloned();
        let venv_bin = tempfile::tempdir().unwrap();
        let pip_name = if cfg!(windows) { "pip.exe" } else { "pip" };
        let pip_path = venv_bin
            .path()
            .join(pip_name)
            .to_string_lossy()
            .into_owned();
        let run = move |cmd: &str, _: &[&str]| -> Option<ProcessOutput> {
            if cmd == pip_path || cmd == "uv" {
                Some(ProcessOutput {
                    success: true,
                    stdout: String::new(),
                    stderr: String::new(),
                })
            } else {
                None
            }
        };
        let ctx = DiscoveryContext {
            env: &env,
            run: &run,
        };
        let cwd = tempfile::tempdir().unwrap();
        let manager = resolve_package_manager(&ctx, cwd.path(), Some(venv_bin.path()), None);
        assert_eq!(manager, Some(PythonPackageManager::Pip));
    }

    #[test]
    fn venv_with_no_known_manager_does_not_fall_back_to_global_probe() {
        // A bare `uv venv` ships no `pip` (or any other manager executable)
        // inside its own bin dir. Even though `poetry` is runnable elsewhere
        // on `PATH`, it never touched this venv, so the probe must report
        // "unknown" rather than guess -- an uninstall command for the wrong
        // manager is worse than no command at all.
        let map = env_from(&[]);
        let env = |n: &str| map.get(n).cloned();
        let run = |cmd: &str, _: &[&str]| -> Option<ProcessOutput> {
            if cmd == "poetry" {
                Some(ProcessOutput {
                    success: true,
                    stdout: String::new(),
                    stderr: String::new(),
                })
            } else {
                None
            }
        };
        let ctx = DiscoveryContext {
            env: &env,
            run: &run,
        };
        let venv_bin = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let manager = resolve_package_manager(&ctx, cwd.path(), Some(venv_bin.path()), None);
        assert_eq!(manager, None);
    }

    // ---- probe_manager_for_manifest ----

    fn run_finding(found: &'static str) -> impl Fn(&str, &[&str]) -> Option<ProcessOutput> {
        move |cmd: &str, _: &[&str]| -> Option<ProcessOutput> {
            if cmd == found {
                Some(ProcessOutput {
                    success: true,
                    stdout: String::new(),
                    stderr: String::new(),
                })
            } else {
                None
            }
        }
    }

    #[test]
    fn probe_manager_for_manifest_finds_a_tool_only_manager_on_path() {
        // Reproduces the reported gap: dbt v2 shipped as a standalone binary
        // has no venv/tool-dir signal of its own, so `existing_hint` is
        // `None` -- but a Hatch- or Rye-managed pyproject.toml project (no
        // recognized lockfile of its own) still has a real manager
        // installed and runnable on `PATH`; this must find it instead of
        // leaving the project "undetermined".
        for manager_name in ["hatch", "rye", "uv"] {
            let map = env_from(&[]);
            let env = |n: &str| map.get(n).cloned();
            let run = run_finding(manager_name);
            let ctx = DiscoveryContext {
                env: &env,
                run: &run,
            };
            let dir = tempfile::tempdir().unwrap();
            let manager =
                probe_manager_for_manifest(&ctx, dir.path(), PythonManifestFormat::Pyproject);
            assert_eq!(
                manager,
                PythonPackageManager::parse_cli_name(manager_name),
                "manager_name={manager_name}"
            );
        }
    }

    #[test]
    fn probe_manager_for_manifest_falls_back_to_pip_for_requirements_format() {
        // The common case for `asdf`/`pyenv`/plain-`pip`
        // `requirements.txt` projects: no dedicated package-manager
        // executable exists (asdf/pyenv only manage the Python *version*),
        // but `pip` itself is always present -- and produces the correct
        // sync command regardless (see `sync_command_for_manager`'s Pip/
        // Asdf/Mise/Pyenv arm).
        let map = env_from(&[]);
        let env = |n: &str| map.get(n).cloned();
        let run = run_finding("pip");
        let ctx = DiscoveryContext {
            env: &env,
            run: &run,
        };
        let dir = tempfile::tempdir().unwrap();
        let manager =
            probe_manager_for_manifest(&ctx, dir.path(), PythonManifestFormat::Requirements);
        assert_eq!(manager, Some(PythonPackageManager::Pip));
    }

    #[test]
    fn probe_manager_for_manifest_ignores_incompatible_managers_on_path() {
        // A machine with `uv`/`poetry`/`pdm` globally installed alongside
        // plain `pip` must not credit any of them for a `requirements.txt`
        // project -- none of them are `is_compatible_with(Requirements)`,
        // so they must never even be checked, only `pip` (the last
        // candidate) should be found.
        let map = env_from(&[]);
        let env = |n: &str| map.get(n).cloned();
        let run = |cmd: &str, _: &[&str]| -> Option<ProcessOutput> {
            if matches!(cmd, "uv" | "poetry" | "pdm" | "pip") {
                Some(ProcessOutput {
                    success: true,
                    stdout: String::new(),
                    stderr: String::new(),
                })
            } else {
                None
            }
        };
        let ctx = DiscoveryContext {
            env: &env,
            run: &run,
        };
        let dir = tempfile::tempdir().unwrap();
        let manager =
            probe_manager_for_manifest(&ctx, dir.path(), PythonManifestFormat::Requirements);
        assert_eq!(
            manager,
            Some(PythonPackageManager::Pip),
            "uv/poetry/pdm must be skipped for a requirements.txt project even though they're \
             on PATH"
        );
    }

    #[test]
    fn probe_manager_for_manifest_prefers_a_venv_local_manager_over_global_path() {
        let map = env_from(&[]);
        let env = |n: &str| map.get(n).cloned();
        let dir = tempfile::tempdir().unwrap();
        let venv_bin_name = if cfg!(windows) { "Scripts" } else { "bin" };
        let venv_bin = dir.path().join(".venv").join(venv_bin_name);
        std::fs::create_dir_all(&venv_bin).unwrap();
        let hatch_name = if cfg!(windows) { "hatch.exe" } else { "hatch" };
        let hatch_path = venv_bin.join(hatch_name).to_string_lossy().into_owned();
        let run = move |cmd: &str, _: &[&str]| -> Option<ProcessOutput> {
            if cmd == hatch_path || cmd == "rye" {
                Some(ProcessOutput {
                    success: true,
                    stdout: String::new(),
                    stderr: String::new(),
                })
            } else {
                None
            }
        };
        let ctx = DiscoveryContext {
            env: &env,
            run: &run,
        };
        let manager = probe_manager_for_manifest(&ctx, dir.path(), PythonManifestFormat::Pyproject);
        assert_eq!(
            manager,
            Some(PythonPackageManager::Hatch),
            "a manager local to the project's own .venv should win over an unrelated one on \
             the global PATH"
        );
    }

    #[test]
    fn probe_manager_for_manifest_returns_none_when_nothing_found() {
        let map = env_from(&[]);
        let env = |n: &str| map.get(n).cloned();
        let ctx = DiscoveryContext {
            env: &env,
            run: &no_run,
        };
        let dir = tempfile::tempdir().unwrap();
        let manager = probe_manager_for_manifest(&ctx, dir.path(), PythonManifestFormat::Pyproject);
        assert_eq!(manager, None);
    }

    // ---- PathDiscovery::command_strings ----

    #[test]
    fn command_strings_for_channel_and_manager() {
        for (channel, distribution_override, manager, expected) in [
            (
                Some(Channel::Standalone),
                None,
                None,
                (Some("dbt system update"), Some("dbt system uninstall")),
            ),
            (
                Some(Channel::Unclaimed),
                None,
                None,
                (Some("dbt system update"), Some("dbt system uninstall")),
            ),
            (
                Some(Channel::Brew),
                None,
                None,
                (Some("brew upgrade dbt"), Some("brew uninstall dbt")),
            ),
            (
                Some(Channel::Winget),
                None,
                None,
                (
                    Some("winget upgrade --id dbtLabs.dbt --exact"),
                    Some("winget uninstall --id dbtLabs.dbt --exact"),
                ),
            ),
            (
                Some(Channel::Pypi),
                None,
                Some(PythonPackageManager::Pip),
                (Some("pip install --upgrade dbt"), Some("pip uninstall dbt")),
            ),
            (
                Some(Channel::Pypi),
                None,
                Some(PythonPackageManager::Uv),
                (Some("uv tool upgrade dbt"), Some("uv tool uninstall dbt")),
            ),
            (None, None, None, (None, None)),
            (Some(Channel::Pypi), None, None, (None, None)),
            (
                Some(Channel::Unsupported("Scoop".to_string())),
                None,
                None,
                (None, None),
            ),
            (
                None,
                Some(Distribution::CloudCLI),
                None,
                (None, Some("brew uninstall dbt-labs/dbt-cli/dbt")),
            ),
        ] {
            let path_discovery = PathDiscovery {
                channel: channel.clone(),
                distribution_override,
                ..Default::default()
            };
            let (upgrade, uninstall) = path_discovery.command_strings(manager, false);
            assert_eq!(
                (upgrade.as_deref(), uninstall.as_deref()),
                expected,
                "channel={channel:?}, distribution_override={distribution_override:?}, manager={manager:?}"
            );
        }
    }

    #[test]
    fn command_strings_add_prerelease_flag_per_manager() {
        for (manager, stable, prerelease) in [
            (
                PythonPackageManager::Pip,
                "pip install --upgrade dbt",
                "pip install --pre --upgrade dbt",
            ),
            (
                PythonPackageManager::Asdf,
                "pip install --upgrade dbt",
                "pip install --pre --upgrade dbt",
            ),
            (
                PythonPackageManager::Pipx,
                "pipx upgrade dbt",
                "pipx upgrade --pip-args=\"--pre\" dbt",
            ),
            (
                PythonPackageManager::Uv,
                "uv tool upgrade dbt",
                "uv tool upgrade --prerelease allow dbt",
            ),
            (
                PythonPackageManager::Poetry,
                "poetry update dbt",
                "poetry add --allow-prereleases dbt",
            ),
            (
                PythonPackageManager::Pdm,
                "pdm update dbt",
                "pdm update --prerelease dbt",
            ),
            (
                PythonPackageManager::Pipenv,
                "pipenv update dbt",
                "pipenv update --pre dbt",
            ),
            (
                PythonPackageManager::Hatch,
                "hatch run pip install --upgrade dbt",
                "hatch run pip install --pre --upgrade dbt",
            ),
            // No prerelease-aware variant: same command either way.
            (
                PythonPackageManager::Conda,
                "conda update dbt",
                "conda update dbt",
            ),
            (
                PythonPackageManager::Rye,
                "rye install dbt",
                "rye install dbt",
            ),
        ] {
            let path_discovery = PathDiscovery {
                channel: Some(Channel::Pypi),
                ..Default::default()
            };
            let (stable_cmd, _) = path_discovery.command_strings(Some(manager), false);
            let (prerelease_cmd, _) = path_discovery.command_strings(Some(manager), true);
            assert_eq!(stable_cmd.as_deref(), Some(stable), "{manager:?} stable");
            assert_eq!(
                prerelease_cmd.as_deref(),
                Some(prerelease),
                "{manager:?} prerelease"
            );
        }
    }

    // ---- run_dbt_internal / parse_dist_info_json ----

    #[test]
    fn run_dbt_internal_returns_stdout_on_success() {
        let run = |_: &str, _: &[&str]| -> Option<ProcessOutput> {
            Some(ProcessOutput {
                success: true,
                stdout: "{}".to_string(),
                stderr: String::new(),
            })
        };
        let ctx = DiscoveryContext {
            env: &real_env,
            run: &run,
        };
        let result = run_dbt_internal(&ctx, Path::new("/usr/local/bin/dbt"));
        assert_eq!(result.as_deref(), Some("{}"));
    }

    #[test]
    fn run_dbt_internal_none_on_nonzero_exit() {
        let run = |_: &str, _: &[&str]| -> Option<ProcessOutput> {
            Some(ProcessOutput {
                success: false,
                stdout: String::new(),
                stderr: "boom".to_string(),
            })
        };
        let ctx = DiscoveryContext {
            env: &real_env,
            run: &run,
        };
        let result = run_dbt_internal(&ctx, Path::new("/usr/local/bin/dbt"));
        assert_eq!(result, None);
    }

    #[test]
    fn run_dbt_internal_none_when_spawn_fails() {
        let ctx = DiscoveryContext {
            env: &real_env,
            run: &no_run,
        };
        let result = run_dbt_internal(&ctx, Path::new("/usr/local/bin/dbt"));
        assert_eq!(result, None);
    }

    #[test]
    fn parse_dist_info_json_success() {
        let json = r#"{
            "path": "/home/user/.local/share/uv/tools/dbt/bin/dbt",
            "channel": "pypi",
            "distribution": "dbt",
            "generation": "v2",
            "py_package_manager": "uv",
            "py_venv_root": null,
            "upgrade_cmd": "uv tool upgrade dbt",
            "uninstall_cmd": "uv tool uninstall dbt"
        }"#;
        let info = parse_dist_info_json(json).unwrap();
        assert_eq!(info.channel, Some(Channel::Pypi));
        assert_eq!(info.distribution, Some(Distribution::Dbt));
    }

    #[test]
    fn parse_dist_info_json_malformed_returns_none() {
        assert!(parse_dist_info_json("not json").is_none());
    }

    #[test]
    fn parse_dist_info_json_empty_returns_none() {
        assert!(parse_dist_info_json("").is_none());
    }

    // ---- probe_generation_and_distribution ----
    //
    // classify_version_output itself is tested in the dbt-dist-classify
    // crate; these tests only cover this crate's process-invocation wrapper
    // around it.

    const V1_VERSION_OUTPUT: &str = "\
Core:
  - installed: 1.12.0
  - latest:    1.12.0 - Up to date!

Plugins:
";

    #[test]
    fn is_prerelease_version_distinguishes_stable_from_prerelease() {
        assert!(!is_prerelease_version("2.0.0"));
        assert!(is_prerelease_version("2.0.0-preview.203"));
        assert!(is_prerelease_version("1.10.0rc1"));
        assert!(is_prerelease_version("1.10.0a1"));
        assert!(is_prerelease_version("1.10.0.dev0"));
    }

    #[test]
    fn probe_generation_and_distribution_none_on_nonzero_exit() {
        let run = |_: &str, _: &[&str]| -> Option<ProcessOutput> {
            Some(ProcessOutput {
                success: false,
                stdout: V1_VERSION_OUTPUT.to_string(),
                stderr: String::new(),
            })
        };
        let ctx = DiscoveryContext {
            env: &real_env,
            run: &run,
        };
        let result = probe_generation_and_distribution(&ctx, Path::new("/usr/local/bin/dbt"));
        assert_eq!(result, None);
    }

    #[test]
    fn probe_generation_and_distribution_none_when_spawn_fails() {
        let ctx = DiscoveryContext {
            env: &real_env,
            run: &no_run,
        };
        let result = probe_generation_and_distribution(&ctx, Path::new("/usr/local/bin/dbt"));
        assert_eq!(result, None);
    }

    // ---- get_all_in_path / discover_all_in_path_value ----

    #[test]
    fn discover_all_in_path_value_none_returns_empty() {
        let result = discover_all_in_path_value(None, "dbt-core").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    #[cfg(unix)]
    fn discover_all_in_path_value_finds_one_executable() {
        use std::os::unix::fs::PermissionsExt;

        let dir1 = tempfile::tempdir().unwrap();
        let dir2 = tempfile::tempdir().unwrap();

        // dir1 has a non-executable dbt; dir2 has the real (executable) one.
        std::fs::write(dir1.path().join("dbt"), b"#!/bin/sh\n").unwrap();

        let exe = dir2.path().join("dbt");
        std::fs::write(&exe, b"#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();

        let path_var = env::join_paths([dir1.path(), dir2.path()]).unwrap();
        let result = discover_all_in_path_value(Some(path_var.as_os_str()), "dbt-core").unwrap();
        assert_eq!(result.len(), 1);
    }

    // ---- get_at_path ----

    #[test]
    fn get_at_path_errors_for_missing_file() {
        let result = get_at_path(Path::new("/nonexistent/path/dbt"), "dbt-core");
        assert!(result.is_err());
    }

    #[test]
    #[cfg(unix)]
    fn get_at_path_falls_back_to_legacy_detection() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("dbt");
        // A script that isn't a real dbt: `internal get-distribution-info`
        // will "succeed" (exit 0) but print nothing parseable as DistInfo,
        // so introspection fails and legacy detection takes over.
        std::fs::write(&exe, b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();

        let result = get_at_path(&exe, "dbt-core").unwrap();
        assert_eq!(result.generation, Generation::NotApplicable);
        assert_eq!(
            result.path,
            dbt_common::stdfs::canonicalize(&exe)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        );
    }

    #[test]
    #[cfg(unix)]
    fn get_at_path_falls_back_to_version_probe_for_v1_dbt_core() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("dbt");
        // `internal get-distribution-info` isn't a recognized subcommand for
        // this legacy dbt-core stand-in, so it fails; `--version` prints the
        // classic dbt-core block.
        std::fs::write(
            &exe,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then cat <<'EOF'\n{V1_VERSION_OUTPUT}EOF\nelse exit 1\nfi\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();

        let result = get_at_path(&exe, "dbt-core").unwrap();
        assert_eq!(result.generation, Generation::V1);
        assert_eq!(result.distribution, Some(Distribution::Core));
    }

    #[test]
    #[cfg(unix)]
    fn get_at_path_falls_back_to_version_probe_for_v2_binary() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("dbt");
        // A pre-`dbt internal` v2 binary: `--version` prints the plain
        // clap-style banner.
        std::fs::write(
            &exe,
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'dbt-fusion 2.0.0-preview.196'; else exit 1; fi\n",
        )
        .unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();

        let result = get_at_path(&exe, "dbt-core").unwrap();
        assert_eq!(result.generation, Generation::V2);
        assert_eq!(result.distribution, Some(Distribution::Dbt));
    }

    #[test]
    #[cfg(unix)]
    fn get_at_path_falls_back_to_version_probe_for_v2_oss_binary() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("dbt");
        // The OSS-only v2 build (`dbt-sa-cli`) brands its `--version` banner
        // as `dbt-core`, even though it's a v2 binary, not the legacy v1
        // Python `dbt-core`.
        std::fs::write(
            &exe,
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'dbt-core 2.0.0-preview.200'; else exit 1; fi\n",
        )
        .unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();

        let result = get_at_path(&exe, "dbt-core").unwrap();
        assert_eq!(result.generation, Generation::V2);
        assert_eq!(result.distribution, Some(Distribution::Oss));
    }

    #[test]
    fn get_at_path_targeting_the_running_binary_short_circuits_to_current() {
        // Regression test: this must not spawn a subprocess. If it did, that
        // subprocess would hit this exact same "target is myself" case and
        // spawn again, forever.
        let current = env::current_exe().unwrap();
        let result = get_at_path(&current, "dbt-core").unwrap();
        assert_eq!(result.generation, Generation::V2);
        assert_eq!(result.distribution, Some(Distribution::Oss));
    }

    #[test]
    fn is_current_executable_true_for_current_exe() {
        let current = env::current_exe().unwrap();
        assert!(is_current_executable(&current));
    }

    #[test]
    fn is_current_executable_false_for_other_path() {
        assert!(!is_current_executable(Path::new("/nonexistent/path/dbt")));
    }

    // ---- get_current (smoke test against the real test binary) ----

    #[test]
    fn get_current_succeeds_against_real_binary() {
        let info = get_current("dbt-core").unwrap();
        assert_eq!(info.generation, Generation::V2);
        assert_eq!(info.distribution, Some(Distribution::Oss));
        assert!(!info.path.is_empty());
        assert_eq!(
            Path::new(&info.path),
            dbt_common::stdfs::canonicalize(env::current_exe().unwrap()).unwrap()
        );
    }
}

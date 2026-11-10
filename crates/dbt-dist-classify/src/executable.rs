//! Locates installed executables in conventional per-platform locations.

use std::{
    collections::HashSet,
    env,
    ffi::OsStr,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy)]
enum CandidatePlatform {
    Windows,
    Unix,
}

/// Return conventional locations for an installed executable in search order.
///
/// The list includes candidates derived from `PATH` (and `PATHEXT` on
/// Windows), followed by user-level installer, package-manager, and
/// version-manager locations. Candidates are returned whether or not they
/// currently exist so callers can apply the validation appropriate to their
/// use case.
#[must_use]
pub fn executable_candidates(executable: &str) -> Vec<PathBuf> {
    let cwd = env::current_dir().ok();
    executable_candidates_in_environment(executable, cwd.as_deref())
}

/// Return conventional executable locations while resolving relative Unix
/// `PATH` entries against the supplied lookup directory.
#[must_use]
pub fn executable_candidates_in(executable: &str, cwd: &Path) -> Vec<PathBuf> {
    executable_candidates_in_environment(executable, Some(cwd))
}

fn executable_candidates_in_environment(executable: &str, cwd: Option<&Path>) -> Vec<PathBuf> {
    let platform = if cfg!(windows) {
        CandidatePlatform::Windows
    } else {
        CandidatePlatform::Unix
    };
    let path_dirs = env::var_os("PATH")
        .as_deref()
        .map(env::split_paths)
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let home = env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .filter(|value| !value.is_empty())
        .or_else(|| {
            env::var_os(if cfg!(windows) { "HOME" } else { "USERPROFILE" })
                .filter(|value| !value.is_empty())
        })
        .map(PathBuf::from)
        .filter(|path| is_absolute_candidate(path, platform));
    let local_app_data = env::var_os("LOCALAPPDATA")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| is_absolute_candidate(path, platform))
        .or_else(|| {
            if cfg!(windows) {
                home.as_ref()
                    .map(|home| join_windows_candidate_path(home, r"AppData\Local"))
            } else {
                None
            }
        });
    let extensions = if cfg!(windows) {
        windows_extensions(env::var("PATHEXT").ok().as_deref())
    } else {
        Vec::new()
    };
    executable_candidates_with(
        executable,
        platform,
        &path_dirs,
        &extensions,
        home.as_deref(),
        local_app_data.as_deref(),
        cwd,
    )
}

/// Return PATH-derived candidates without mutating the process environment.
/// Windows consults `PATHEXT`; relative, root-relative, non-UNC separator-led,
/// and empty PATH components are ignored. Unix resolves relative and empty
/// components against `cwd`, matching POSIX PATH lookup semantics.
#[must_use]
pub fn executable_candidates_from_path(executable: &str, path: &OsStr, cwd: &Path) -> Vec<PathBuf> {
    let path_dirs = env::split_paths(path).collect::<Vec<_>>();
    let extensions = if cfg!(windows) {
        windows_extensions(env::var("PATHEXT").ok().as_deref())
    } else {
        Vec::new()
    };
    executable_candidates_with(
        executable,
        if cfg!(windows) {
            CandidatePlatform::Windows
        } else {
            CandidatePlatform::Unix
        },
        &path_dirs,
        &extensions,
        None,
        None,
        Some(cwd),
    )
}

const DEFAULT_WINDOWS_EXTENSIONS: [&str; 4] = [".COM", ".EXE", ".BAT", ".CMD"];

fn windows_extensions(pathext: Option<&str>) -> Vec<String> {
    let extensions = pathext
        .unwrap_or_default()
        .split(';')
        .filter_map(|extension| {
            let extension = extension.trim();
            if extension.is_empty() {
                return None;
            }
            let extension = if extension.starts_with('.') {
                extension.to_owned()
            } else {
                format!(".{extension}")
            };
            is_windows_launch_extension(&extension).then_some(extension)
        })
        .collect::<Vec<_>>();
    if extensions.is_empty() {
        DEFAULT_WINDOWS_EXTENSIONS
            .iter()
            .map(|extension| (*extension).to_owned())
            .collect()
    } else {
        extensions
    }
}

fn executable_candidates_with(
    executable: &str,
    platform: CandidatePlatform,
    path_dirs: &[PathBuf],
    extensions: &[String],
    home: Option<&Path>,
    local_app_data: Option<&Path>,
    cwd: Option<&Path>,
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let mut seen = HashSet::<String>::new();
    let mut push = |candidate: PathBuf| {
        if !is_absolute_candidate(&candidate, platform) {
            return;
        }
        let key = candidate_key(&candidate, platform);
        if seen.insert(key) {
            candidates.push(candidate);
        }
    };

    for directory in path_dirs {
        let directory = match platform {
            CandidatePlatform::Windows => {
                if directory.as_os_str().is_empty() || !is_absolute_candidate(directory, platform) {
                    continue;
                }
                directory.clone()
            }
            CandidatePlatform::Unix => {
                if is_absolute_candidate(directory, platform) {
                    directory.clone()
                } else {
                    let Some(cwd) = cwd.filter(|cwd| is_absolute_candidate(cwd, platform)) else {
                        continue;
                    };
                    join_unix_candidate_path(cwd, &directory.to_string_lossy())
                }
            }
        };
        if matches!(platform, CandidatePlatform::Windows) {
            if Path::new(executable).extension().is_some() {
                push(join_windows_candidate_path(&directory, executable));
            } else {
                for extension in extensions {
                    push(join_windows_candidate_path(
                        &directory,
                        &format!("{executable}{extension}"),
                    ));
                }
            }
        } else {
            push(join_unix_candidate_path(&directory, executable));
        }
    }

    match platform {
        CandidatePlatform::Windows => {
            if let Some(home) = home {
                push_windows_candidates(
                    &mut push,
                    join_windows_candidate_path(home, r".local\bin"),
                    executable,
                    extensions,
                );
            }
            if let Some(local_app_data) = local_app_data {
                push_windows_candidates(
                    &mut push,
                    join_windows_candidate_path(local_app_data, r"Microsoft\WinGet\Links"),
                    executable,
                    extensions,
                );
            }
            if let Some(home) = home {
                push_windows_candidates(
                    &mut push,
                    join_windows_candidate_path(home, r".pyenv\pyenv-win\shims"),
                    executable,
                    extensions,
                );
            }
        }
        CandidatePlatform::Unix => {
            let platform_name = executable.to_string();
            if let Some(home) = home {
                let local_bin = join_unix_candidate_path(home, ".local/bin");
                push(join_unix_candidate_path(&local_bin, &platform_name));
                let pyenv_shims = join_unix_candidate_path(home, ".pyenv/shims");
                push(join_unix_candidate_path(&pyenv_shims, &platform_name));
            }
            push(join_unix_candidate_path(
                Path::new("/opt/homebrew/bin"),
                &platform_name,
            ));
            push(join_unix_candidate_path(
                Path::new("/usr/local/bin"),
                &platform_name,
            ));
        }
    }

    candidates
}

fn join_unix_candidate_path(base: &Path, child: &str) -> PathBuf {
    if child.is_empty() {
        return base.to_path_buf();
    }
    if child.starts_with('/') {
        return PathBuf::from(child);
    }
    let mut joined = base.as_os_str().to_os_string();
    if !joined.is_empty() && !joined.to_string_lossy().ends_with('/') {
        joined.push("/");
    }
    joined.push(child);
    PathBuf::from(joined)
}

fn join_windows_candidate_path(base: &Path, child: &str) -> PathBuf {
    if child.is_empty() {
        return base.to_path_buf();
    }
    let mut joined = base.as_os_str().to_os_string();
    if !joined.is_empty() && !joined.to_string_lossy().ends_with(['/', '\\']) {
        joined.push("\\");
    }
    joined.push(child);
    PathBuf::from(joined)
}

fn push_windows_candidates(
    push: &mut impl FnMut(PathBuf),
    directory: PathBuf,
    executable: &str,
    extensions: &[String],
) {
    if Path::new(executable).extension().is_some() {
        push(join_windows_candidate_path(&directory, executable));
    } else {
        push(join_windows_candidate_path(
            &directory,
            &format!("{executable}.exe"),
        ));
        for extension in extensions {
            push(join_windows_candidate_path(
                &directory,
                &format!("{executable}{extension}"),
            ));
        }
    }
}

fn candidate_key(path: &Path, platform: CandidatePlatform) -> String {
    let path = path.to_string_lossy();
    match platform {
        CandidatePlatform::Windows => path
            .trim_end_matches(['/', '\\'])
            .replace('/', "\\")
            .to_lowercase(),
        CandidatePlatform::Unix => path.into_owned(),
    }
}

fn is_absolute_candidate(path: &Path, platform: CandidatePlatform) -> bool {
    match platform {
        CandidatePlatform::Windows => is_windows_absolute_path(&path.to_string_lossy()),
        CandidatePlatform::Unix => path.to_string_lossy().starts_with('/'),
    }
}

fn is_windows_absolute_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    if value.starts_with(r"\\?\") {
        return true;
    }
    if bytes.len() >= 2
        && is_windows_path_separator(bytes[0])
        && is_windows_path_separator(bytes[1])
    {
        let rest = &bytes[2..];
        if rest.len() >= 2 && rest[0] == b'.' && is_windows_path_separator(rest[1]) {
            return true;
        }
        let mut parts = rest.split(|byte| is_windows_path_separator(*byte));
        let server = parts.next().unwrap_or_default();
        let share = parts.next().unwrap_or_default();
        return !server.is_empty() && !share.is_empty();
    }
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && is_windows_path_separator(bytes[2])
}

fn is_windows_path_separator(byte: u8) -> bool {
    matches!(byte, b'/' | b'\\')
}

fn is_windows_launch_extension(extension: &str) -> bool {
    matches!(
        extension.to_ascii_lowercase().as_str(),
        ".exe" | ".com" | ".bat" | ".cmd"
    )
}

/// Return the first absolute candidate that can be launched on this platform.
/// The candidate list remains ordered, so an invalid file cannot shadow a
/// later valid installation.
#[must_use]
pub fn find_executable(candidates: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    find_executable_with_platform(
        candidates,
        if cfg!(windows) {
            CandidatePlatform::Windows
        } else {
            CandidatePlatform::Unix
        },
    )
}

fn find_executable_with_platform(
    candidates: impl IntoIterator<Item = PathBuf>,
    platform: CandidatePlatform,
) -> Option<PathBuf> {
    candidates
        .into_iter()
        .filter(|candidate| is_absolute_candidate(candidate, platform))
        .find(|candidate| is_launchable_candidate(candidate, platform))
}

fn is_launchable_candidate(path: &Path, platform: CandidatePlatform) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    match platform {
        CandidatePlatform::Windows => path
            .extension()
            .map(|extension| {
                is_windows_launch_extension(&format!(".{}", extension.to_string_lossy()))
            })
            .unwrap_or(false),
        CandidatePlatform::Unix => {
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_executable_candidates_include_path_installer_winget_and_pyenv() {
        let path_dirs = [
            PathBuf::from(r"C:\windows\tools"),
            PathBuf::from(r"C:\windows\other"),
        ];
        let extensions = [".COM".to_string(), ".EXE".to_string()];
        let candidates = executable_candidates_with(
            "dbt",
            CandidatePlatform::Windows,
            &path_dirs,
            &extensions,
            Some(Path::new(r"C:\windows\Users\me")),
            Some(Path::new(r"C:\windows\Users\me\AppData\Local")),
            None,
        );

        assert_eq!(
            candidates,
            vec![
                PathBuf::from(r"C:\windows\tools\dbt.COM"),
                PathBuf::from(r"C:\windows\tools\dbt.EXE"),
                PathBuf::from(r"C:\windows\other\dbt.COM"),
                PathBuf::from(r"C:\windows\other\dbt.EXE"),
                PathBuf::from(r"C:\windows\Users\me\.local\bin\dbt.exe"),
                PathBuf::from(r"C:\windows\Users\me\.local\bin\dbt.COM"),
                PathBuf::from(r"C:\windows\Users\me\AppData\Local\Microsoft\WinGet\Links\dbt.exe",),
                PathBuf::from(r"C:\windows\Users\me\AppData\Local\Microsoft\WinGet\Links\dbt.COM",),
                PathBuf::from(r"C:\windows\Users\me\.pyenv\pyenv-win\shims\dbt.exe"),
                PathBuf::from(r"C:\windows\Users\me\.pyenv\pyenv-win\shims\dbt.COM"),
            ]
        );
    }

    #[test]
    fn windows_fallbacks_always_include_fixed_exe_when_pathext_omits_exe() {
        let candidates = executable_candidates_with(
            "dbt",
            CandidatePlatform::Windows,
            &[PathBuf::from(r"C:\windows\tools")],
            &[".BAT".to_owned()],
            Some(Path::new(r"C:\windows\Users\me")),
            Some(Path::new(r"C:\windows\Users\me\AppData\Local")),
            None,
        );

        assert_eq!(
            candidates,
            vec![
                PathBuf::from(r"C:\windows\tools\dbt.BAT"),
                PathBuf::from(r"C:\windows\Users\me\.local\bin\dbt.exe"),
                PathBuf::from(r"C:\windows\Users\me\.local\bin\dbt.BAT"),
                PathBuf::from(r"C:\windows\Users\me\AppData\Local\Microsoft\WinGet\Links\dbt.exe",),
                PathBuf::from(r"C:\windows\Users\me\AppData\Local\Microsoft\WinGet\Links\dbt.BAT",),
                PathBuf::from(r"C:\windows\Users\me\.pyenv\pyenv-win\shims\dbt.exe"),
                PathBuf::from(r"C:\windows\Users\me\.pyenv\pyenv-win\shims\dbt.BAT"),
            ]
        );
    }

    #[test]
    fn windows_absolute_candidate_requires_drive_or_unc_prefix() {
        for path in [
            r"\tools\dbt.exe",
            "/tools/dbt.exe",
            r"tools\dbt.exe",
            r"C:tools\dbt.exe",
            r"C:",
            r"\\server",
            r"\\server\",
            r"\\\tools\dbt.exe",
            "///tools/dbt.exe",
            r"1:\tools\dbt.exe",
            "",
        ] {
            assert!(
                !is_absolute_candidate(Path::new(path), CandidatePlatform::Windows),
                "expected Windows path to be relative: {path}"
            );
        }

        for path in [
            r"C:\tools\dbt.exe",
            "C:/tools/dbt.exe",
            r"\\server\share\tools\dbt.exe",
            "//server/share/tools/dbt.exe",
            r"\\?\C:\tools\dbt.exe",
            r"\\.\C:\tools\dbt.exe",
            r"\\?\UNC\server\share\dbt.exe",
            r"\\.\pipe\x",
            "//?/C:/x",
        ] {
            assert!(
                is_absolute_candidate(Path::new(path), CandidatePlatform::Windows),
                "expected Windows path to be absolute: {path}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_absolute_candidate_matches_std_oracle() {
        let paths = [
            r"\tools\dbt.exe",
            "/tools/dbt.exe",
            r"tools\dbt.exe",
            r"C:tools\dbt.exe",
            r"C:",
            r"\\server",
            r"\\server\",
            r"\\\tools\dbt.exe",
            "///tools/dbt.exe",
            r"1:\tools\dbt.exe",
            "",
            r"C:\tools\dbt.exe",
            "C:/tools/dbt.exe",
            r"\\server\share\tools\dbt.exe",
            "//server/share/tools/dbt.exe",
            r"\\?\C:\tools\dbt.exe",
            r"\\.\C:\tools\dbt.exe",
            r"\\?\UNC\server\share\dbt.exe",
            r"\\.\pipe\x",
            "//?/C:/x",
        ];
        for path in paths {
            assert_eq!(
                is_absolute_candidate(Path::new(path), CandidatePlatform::Windows),
                Path::new(path).is_absolute(),
                "Windows absolute-path mismatch: {path}"
            );
        }
    }

    #[test]
    fn launchable_resolver_skips_invalid_candidate_before_valid_candidate() {
        let root = tempfile::tempdir().unwrap();
        let invalid = root
            .path()
            .join(if cfg!(windows) { "dbt-invalid" } else { "dbt" });
        let valid = root.path().join(if cfg!(windows) {
            "dbt-valid.exe"
        } else {
            "dbt-valid"
        });
        std::fs::write(&invalid, b"not executable").unwrap();
        std::fs::write(&valid, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&invalid, std::fs::Permissions::from_mode(0o644)).unwrap();
            std::fs::set_permissions(&valid, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        assert_eq!(find_executable([invalid, valid.clone()]), Some(valid));
    }

    #[test]
    fn windows_launchable_candidate_requires_extension() {
        let root = tempfile::tempdir().unwrap();
        let extensionless = root.path().join("dbt");
        let executable = root.path().join("dbt.exe");
        std::fs::write(&extensionless, b"pyenv shim").unwrap();
        std::fs::write(&executable, b"MZ").unwrap();

        assert!(!is_launchable_candidate(
            &extensionless,
            CandidatePlatform::Windows
        ));
        assert!(is_launchable_candidate(
            &executable,
            CandidatePlatform::Windows
        ));
    }

    #[cfg(windows)]
    #[test]
    fn windows_launchable_resolver_rejects_extensionless_file() {
        let root = tempfile::tempdir().unwrap();
        let extensionless = root.path().join("dbt");
        let executable = root.path().join("dbt.exe");
        std::fs::write(&extensionless, b"pyenv shim").unwrap();
        std::fs::write(&executable, b"MZ").unwrap();

        assert_eq!(
            find_executable_with_platform(
                [extensionless, executable.clone()],
                CandidatePlatform::Windows,
            ),
            Some(executable)
        );
    }

    #[test]
    fn windows_candidate_resolution_rejects_extensionless_pyenv_shim() {
        let root = tempfile::tempdir().unwrap();
        let extensionless = root.path().join("dbt");
        let batch = root.path().join("dbt.bat");
        std::fs::write(&extensionless, b"pyenv shim").unwrap();
        std::fs::write(&batch, b"@echo dbt").unwrap();

        assert!(!is_launchable_candidate(
            &extensionless,
            CandidatePlatform::Windows
        ));
        assert!(is_launchable_candidate(&batch, CandidatePlatform::Windows));
    }

    #[cfg(windows)]
    #[test]
    fn windows_candidate_resolution_skips_extensionless_pyenv_shim() {
        let root = tempfile::tempdir().unwrap();
        let extensionless = root.path().join("dbt");
        let batch = root.path().join("dbt.bat");
        std::fs::write(&extensionless, b"pyenv shim").unwrap();
        std::fs::write(&batch, b"@echo dbt").unwrap();

        let generated = executable_candidates_with(
            "dbt",
            CandidatePlatform::Windows,
            &[root.path().to_path_buf()],
            &[".bat".to_owned()],
            None,
            None,
            None,
        );
        let candidates = std::iter::once(extensionless).chain(generated);
        assert_eq!(
            find_executable_with_platform(candidates, CandidatePlatform::Windows),
            Some(batch)
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_find_executable_returns_absolute_candidate() {
        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join("dbt.exe");
        std::fs::write(&executable, b"MZ").unwrap();

        let candidates = executable_candidates_with(
            "dbt",
            CandidatePlatform::Windows,
            &[root.path().to_path_buf()],
            &[".EXE".to_owned()],
            None,
            None,
            None,
        );
        let found = find_executable(candidates).expect("Windows executable should be found");
        assert!(found.is_absolute());
        assert!(
            found
                .to_string_lossy()
                .eq_ignore_ascii_case(&executable.to_string_lossy()),
            "found path {found:?} did not match executable {executable:?}"
        );
    }

    #[test]
    fn path_candidate_precedes_conventional_home_candidate() {
        let path_dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let platform = if cfg!(windows) {
            CandidatePlatform::Windows
        } else {
            CandidatePlatform::Unix
        };
        let name = if cfg!(windows) { "dbt.EXE" } else { "dbt" };
        let path_candidate = path_dir.path().join(name);
        let home_candidate = home.path().join(".local/bin").join(name);
        std::fs::create_dir_all(home_candidate.parent().unwrap()).unwrap();
        std::fs::write(&path_candidate, b"path dbt").unwrap();
        std::fs::write(&home_candidate, b"home dbt").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path_candidate, std::fs::Permissions::from_mode(0o755))
                .unwrap();
            std::fs::set_permissions(&home_candidate, std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }

        let extensions = if cfg!(windows) {
            vec![".EXE".to_owned()]
        } else {
            vec![]
        };
        let candidates = executable_candidates_with(
            "dbt",
            platform,
            &[path_dir.path().to_path_buf()],
            &extensions,
            Some(home.path()),
            None,
            None,
        );
        assert_eq!(
            find_executable_with_platform(candidates, platform),
            Some(path_candidate)
        );
    }

    #[test]
    fn candidate_generation_filters_empty_relative_and_case_duplicate_windows_dirs() {
        let candidates = executable_candidates_with(
            "dbt",
            CandidatePlatform::Windows,
            &[
                PathBuf::new(),
                PathBuf::from("relative/tools"),
                PathBuf::from(r"\Windows\Tools"),
                PathBuf::from("/windows/tools"),
                PathBuf::from(r"C:\Windows\Tools"),
                PathBuf::from("c:/windows/tools"),
            ],
            &[".EXE".to_owned()],
            None,
            None,
            None,
        );

        assert_eq!(
            candidates,
            vec![PathBuf::from(r"C:\Windows\Tools\dbt.EXE"),]
        );
    }

    #[test]
    fn empty_or_missing_pathext_uses_windows_default_extensions() {
        let expected = vec![
            ".COM".to_owned(),
            ".EXE".to_owned(),
            ".BAT".to_owned(),
            ".CMD".to_owned(),
        ];
        assert_eq!(windows_extensions(None), expected);
        assert_eq!(windows_extensions(Some("")), expected);
        assert_eq!(windows_extensions(Some(".EXE;;.BAT")), vec![".EXE", ".BAT"]);
    }

    #[test]
    fn unix_executable_candidates_include_path_and_well_known_locations_once() {
        let candidates = executable_candidates_with(
            "dbt-wizard",
            CandidatePlatform::Unix,
            &[
                PathBuf::from("/home/me/.local/bin"),
                PathBuf::from("/usr/local/bin"),
            ],
            &[],
            Some(Path::new("/home/me")),
            None,
            None,
        );

        assert_eq!(
            candidates,
            vec![
                PathBuf::from("/home/me/.local/bin/dbt-wizard"),
                PathBuf::from("/usr/local/bin/dbt-wizard"),
                PathBuf::from("/home/me/.pyenv/shims/dbt-wizard"),
                PathBuf::from("/opt/homebrew/bin/dbt-wizard"),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_executable_candidates_resolve_relative_and_empty_path_entries() {
        let cwd = Path::new("/project");
        let candidates = executable_candidates_from_path("dbt", OsStr::new("bin:"), cwd);

        assert_eq!(
            candidates,
            vec![
                cwd.join("bin/dbt"),
                cwd.join("dbt"),
                PathBuf::from("/opt/homebrew/bin/dbt"),
                PathBuf::from("/usr/local/bin/dbt"),
            ]
        );
    }
}

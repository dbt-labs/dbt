# dbt-ci

Release-pipeline commands for dbt-fusion. Exposed as `cargo ci` via the
workspace `.cargo/config.toml` alias.

## Subcommands

- `cargo ci bump-cargo-version <version> [--dry-run] [--no-lockfile]` — writes `version` into `[workspace.package].version` and refreshes `Cargo.lock`.
- `cargo ci pypi pack --binaries-dir DIR --version X.Y.Z [--out DIR] [--bin-name NAME]` — writes one `py3-none-{platform}` wheel per pre-built binary in `--binaries-dir` (files named by cargo target triple, `.exe` for Windows).
- `cargo ci pypi publish --environment {staging|prod|test-pypi} [--version V] [--dist DIR]` — **wheel-upload mode**: publishes wheels in `--dist` (default `target/wheels`) matching the workspace pyproject's `[project].name`. `--version` filters to that PEP 440 version.
- `cargo ci pypi publish --environment {staging|prod|test-pypi} --version V --download-base-url URL --target TRIPLE [--target TRIPLE]…` — **sdist mode**: downloads this release's wheels from the `https://` base URL (one per `--target`), hashes the live bytes, assembles the download-at-install sdist, and publishes only the sdist (`filetype=sdist`). `--download-base-url` requires `--version` and at least one `--target`.
- `cargo ci homebrew render --tarballs-dir DIR --version X.Y.Z --url-template URL [--out PATH] [--formula-name NAME] [--binary-name NAME] [--conflicts-with NAME]…` — writes a `Formula/<name>.rb` from release tarballs (`fs-v{version}-{target}.tar.gz`). Reads name/license/homepage from pyproject. Linux + macOS only — Windows tarballs are skipped.
- `cargo ci homebrew publish --formula PATH --tap-repo URL --version X.Y.Z [--tap-branch B] [--token-env VAR] [--dry-run]` — clones the tap, copies the formula into `Formula/`, commits, and pushes. Idempotent: no-op if the formula is byte-identical. `--token-env` defaults to `HOMEBREW_TAP_REPO_TOKEN`.

### Install-time namespace notice

Pre-release sdists named `dbt-core` (2.x and up) embed a `notice` in the sdist
manifest, which the PEP 517 backend prints at install time pointing at the `dbt`
/ `dbt-oss` distributions. Nothing reaches a `dbt-core` pre-release by accident
— a resolver only picks one under `--pre` or an explicit `dbt-core==2.0.0rc…`
pin — so the notice is exactly the set of installs worth redirecting.

`dbt-core` 2.x keeps shipping in parallel with `dbt` / `dbt-oss`, so the notice
is a pointer at the preferred name, not a deprecation. Final releases, older
majors, and the other distributions get no notice; widening it to every 2.x
`dbt-core` sdist means dropping the pre-release check in `install_notice`
(`src/sdist.rs`).

### sdist runtime metadata

An sdist must declare the same `requires-python` and dependencies as the wheel it
hands back: pip re-reads the built wheel's metadata, but uv trusts the sdist and
installs exactly what it declares. `dbt-core` / `dbt-oss` reference the maturin
extension wheels, so they pass `--runtime-metadata-from crates/dbt-python` to
take those two fields from there while keeping the root pyproject's descriptive
metadata. `dbt-core-experimental-parser` ships the `py3-none` binary CLI wheel,
which has no Python dependencies, and needs no flag.

## Version shapes

| SemVer            | PEP 440 |
|-------------------|---------|
| `X.Y.Z`           | `X.Y.Z` |
| `X.Y.Z-alpha.N`   | `X.Y.ZaN` |
| `X.Y.Z-beta.N`    | `X.Y.ZbN` |
| `X.Y.Z-rc.N`      | `X.Y.ZrcN` |
| `X.Y.Z-preview.N` | `X.Y.ZrcN` |
| `X.Y.Z-dev.N`     | `X.Y.Z.devN` |

## Env vars

- `staging`: `DBT_PYPI_STAGING_DOMAIN`, `DBT_PYPI_STAGING_DOMAIN_OWNER`, `DBT_PYPI_STAGING_REGION`, `DBT_PYPI_STAGING_REPOSITORY`, `DBT_PYPI_STAGING_PROFILE`
- `prod`: `DBT_PYPI_PROD_TOKEN`
- `test-pypi`: `DBT_PYPI_TEST_TOKEN`
- `homebrew publish`: `HOMEBREW_TAP_REPO_TOKEN` (or whatever `--token-env` points to) — a PAT with write access to the tap repo.

## Typical CI sequence

```
cargo ci bump-cargo-version X.Y.Z
cargo build --release --bin <bin> --target <triple>   # per platform
# stage each binary into binaries/ named by its target triple (.exe for Windows):
#   binaries/x86_64-unknown-linux-gnu, binaries/x86_64-pc-windows-msvc.exe, …
cargo ci pypi pack --binaries-dir binaries --version X.Y.Z
cargo ci pypi publish --environment staging --version X.Y.Z   # uploads wheels
cargo ci pypi publish --environment prod --version X.Y.Z      # uploads wheels

# Publish the download-at-install sdist pointing at the release's wheel host
# (one --target per published wheel). For dbt-core / dbt-oss add
# `--python-tag cp311 --abi-tag abi3 --runtime-metadata-from crates/dbt-python`,
# since those reference the maturin extension wheels:
cargo ci pypi publish --environment prod --version X.Y.Z \
  --download-base-url https://github.com/dbt-labs/dbt-core/releases/download/vX.Y.Z \
  --target x86_64-unknown-linux-gnu \
  --target aarch64-unknown-linux-gnu \
  --target x86_64-apple-darwin \
  --target aarch64-apple-darwin \
  --target x86_64-pc-windows-msvc

# Homebrew (requires release tarballs, not raw binaries):
cargo ci homebrew render \
  --tarballs-dir release-artifacts \
  --version X.Y.Z \
  --url-template "https://public.cdn.getdbt.com/fs/cli/{filename}" \
  --conflicts-with dbt-core
cargo ci homebrew publish \
  --formula target/homebrew/Formula/dbt.rb \
  --tap-repo https://github.com/dbt-labs/homebrew-dbt.git \
  --version X.Y.Z
```

# syntax=docker/dockerfile:1

# Self-hosted dbt docs v2 server. Downloads a released dbt binary from this
# repository's GitHub Releases — no source build, no cargo, and nothing is
# COPYed from the build context (see .dockerignore).
#
# Build. The context is this directory, so the command is the same from the
# repo root or from here:
#   docker build -t dbt-docs-server crates/dbt-docs-server
#
# By default the newest release carrying a Linux binary is resolved at build
# time. Pin a version instead with:
#   docker build -t dbt-docs-server \
#     --build-arg DBT_VERSION=2.0.0-beta.2 crates/dbt-docs-server
#
# Run (mount a target/ dir with target/index/*.parquet from
# `dbt --write-index <run|build|compile>`). The named volume caches the ADBC
# DuckDB driver, downloaded from public.cdn.getdbt.com on first boot (needs
# outbound HTTPS once; offline after):
#   docker run --rm -p 8580:8580 \
#     -v "$PWD/target:/data/target:ro" \
#     -v dbt-adbc-cache:/var/cache/dbt \
#     dbt-docs-server

# This stage only downloads and verifies a tarball, so pin it to the builder's
# own architecture: a multi-arch build then needs no emulation to fetch.
FROM --platform=$BUILDPLATFORM debian:bookworm-slim AS fetch

# `latest` resolves the newest release carrying a Linux binary, at build time.
# Any other value pins that version; a leading `v` is optional.
ARG DBT_VERSION=latest
ARG TARGETARCH

# Cache mounts keep apt's archives between builds. Removing docker-clean has to
# come first — it tells apt to discard every .deb as soon as it is unpacked.
RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt/lists,sharing=locked \
    rm -f /etc/apt/apt.conf.d/docker-clean \
    && apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl jq

WORKDIR /tmp

# Resolve the version, download the tarball for the target architecture, check
# it against the release's published SHA256SUMS, then install the single binary
# it contains as `dbt`. Releases through 2.0.0-beta.2 shipped that binary named
# `dbt-sa-cli`; later ones name it `dbt`, so accept either.
#
# Why the release list and not /releases/latest: this repository also publishes
# the dbt Core Python package, and those are the releases marked "latest", so
# that endpoint hands back a version with no binary attached. The list comes
# back newest-first, so take the first non-draft release that actually carries
# the asset for this architecture.
RUN <<'EOF'
set -eu

case "$TARGETARCH" in
  amd64) triple=x86_64-unknown-linux-gnu ;;
  arm64) triple=aarch64-unknown-linux-gnu ;;
  *) echo "unsupported TARGETARCH: ${TARGETARCH:-unset}" >&2; exit 1 ;;
esac

if [ "$DBT_VERSION" = latest ]; then
  version=$(
    curl -fsSL -H 'Accept: application/vnd.github+json' \
      'https://api.github.com/repos/dbt-labs/dbt-core/releases?per_page=100' \
      | jq -r --arg triple "$triple" '
          [ .[]
            | select(.draft == false)
            | .assets[].name
            | capture("^dbt-core-(?<v>.+)-\($triple)\\.tar\\.gz$").v
          ] | first // empty'
  )
  if [ -z "$version" ]; then
    echo "could not resolve a release carrying a ${triple} binary." >&2
    echo "GitHub may be rate-limiting this build (60 requests/hour for anonymous callers)." >&2
    echo "Pass --build-arg DBT_VERSION=<version> to pin one instead." >&2
    exit 1
  fi
  echo "resolved DBT_VERSION=latest to ${version}"
else
  # Accept the plain version and the tag spelling: 2.0.0-beta.2, v2.0.0-beta.2.
  version=${DBT_VERSION#v}
fi

base="dbt-core-${version}-${triple}"
url="https://github.com/dbt-labs/dbt-core/releases/download/v${version}"

if ! curl -fsSL -o "${base}.tar.gz" "${url}/${base}.tar.gz"; then
  echo "could not download ${base}.tar.gz from ${url}" >&2
  echo "check that this version exists and ships a ${triple} binary:" >&2
  echo "  https://github.com/dbt-labs/dbt-core/releases" >&2
  exit 1
fi
curl -fsSL -o SHA256SUMS "${url}/SHA256SUMS"

# Match the filename as a literal string — the version is full of dots, which a
# regex would read as wildcards. Exiting non-zero when the entry is missing
# keeps a release without this asset from silently skipping the checksum.
awk -v want="${base}.tar.gz" '$2 == want { print; found = 1 } END { exit !found }' \
    SHA256SUMS > wanted.sha256
sha256sum -c wanted.sha256

tar -xzf "${base}.tar.gz"
if [ -f "${base}/dbt" ]; then
  install -Dm755 "${base}/dbt" /out/dbt
elif [ -f "${base}/dbt-sa-cli" ]; then
  install -Dm755 "${base}/dbt-sa-cli" /out/dbt
else
  echo "no dbt binary found in ${base}.tar.gz" >&2
  ls -la "${base}" >&2
  exit 1
fi
EOF


FROM debian:bookworm-slim AS runtime

LABEL org.opencontainers.image.title="dbt-docs-server" \
      org.opencontainers.image.description="Self-hosted dbt docs v2 server (dbt docs serve)." \
      org.opencontainers.image.source="https://github.com/dbt-labs/dbt-core" \
      org.opencontainers.image.licenses="Apache-2.0"

# ca-certificates and libstdc++6 are both for the ADBC DuckDB driver: it is
# fetched over HTTPS on first boot, and the .so is C++. curl backs the
# HEALTHCHECK below.
RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt/lists,sharing=locked \
    rm -f /etc/apt/apt.conf.d/docker-clean \
    && apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl libstdc++6 \
    && useradd --uid 10001 --user-group --no-create-home --shell /usr/sbin/nologin dbt

COPY --from=fetch /out/dbt /usr/local/bin/dbt

# Bind every interface: the server defaults to 127.0.0.1, which nothing outside
# the container could reach. Map the port at `docker run`. DBT_LOG_PATH points
# file logging somewhere writable — dbt otherwise creates ./logs relative to the
# working directory.
ENV DBT_DOCS_HOST=0.0.0.0 \
    DBT_DOCS_PORT=8580 \
    DBT_DOCS_TARGET_PATH=/data/target \
    XDG_CACHE_HOME=/var/cache/dbt \
    DBT_LOG_PATH=/var/log/dbt \
    HOME=/home/dbt

# Everything the unprivileged user has to write to: the driver cache (mount a
# named volume here to keep it between runs), the log dir, and its home.
# /data/target is created so that forgetting the artifact mount shows up as an
# empty directory rather than a permission error.
RUN install -d -o dbt -g dbt /var/cache/dbt /var/log/dbt /home/dbt /data/target

USER dbt
WORKDIR /home/dbt

# Smoke-test the binary as the user that will run it, so a bad download or an
# unwritable HOME fails the build instead of the first `docker run`.
RUN dbt --version

EXPOSE 8580

# The site is static and has no health endpoint, so check `/`: it is served as
# index.html the moment the listener is up, which is the signal we want. 8580 is
# the in-container port, unaffected by host-side remapping.
# `--start-interval` needs Docker Engine 25.0 or newer.
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --start-interval=2s --retries=3 \
    CMD ["curl", "-fsS", "--head", "--max-time", "5", "http://127.0.0.1:8580/"]

ENTRYPOINT ["dbt", "docs", "serve", "--no-open"]

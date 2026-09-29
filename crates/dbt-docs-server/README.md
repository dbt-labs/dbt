<div align="center">
  <h1>dbt-docs-server</h1>
  <p><strong>The next generation of dbt docs.</strong></p>
  <p>
    A static, interactive docs site for your dbt project — the parquet artifacts the dbt v2 engine writes on every run, queried in the browser by DuckDB-WASM.
  </p>
  <p>
    <a href="./API-CONTRACTS.md">Contracts &amp; decisions</a> ·
    <a href="https://github.com/dbt-labs/dbt-core">dbt Core repo</a> ·
    <a href="https://docs.getdbt.com/docs/fusion/about-fusion">About dbt v2</a> ·
    <a href="https://docs.getdbt.com">Official dbt docs</a>
  </p>
  <p>
    <img alt="License: Apache 2.0" src="https://img.shields.io/badge/license-Apache%202.0-blue.svg" />
    <img alt="Rust" src="https://img.shields.io/badge/built%20with-Rust-orange.svg" />
  </p>
</div>

---

## 👋 Introduction

`dbt-docs-server` is the successor to dbt Core v1's `dbt docs generate` + `dbt docs serve`, rebuilt for the Rust dbt v2 runtime. It ships inside the dbt v2 binary as `dbt docs generate` and `dbt docs serve`, and can also be self-hosted in a container.

### Static sites (`dbt docs generate`)

`dbt docs generate` writes into your target directory, which you can host on any plain file server — GitLab Pages, GitHub Pages, S3 — with no process and no state. From a clean checkout, one command is enough:

```bash
dbt docs generate
```

That runs `compile --write-index` and exports the index it writes, so the site always describes your project as it is now. Compiling is unconditional, as it is in dbt Core v1.

To reuse an index you already have, ask for that explicitly with `--no-compile`:

```bash
dbt compile --write-index --static-analysis strict   # or: dbt build --write-index …
dbt docs generate --no-compile                       # exports the index above, as-is
```

That two-step form is how you get column-level lineage, which `--static-analysis strict` produces and the plain compile above does not. It is also the form to use where the warehouse is unreachable, or where you want the export to stay cheap and read-only. With `--no-compile` and no index, the command is an error rather than a compile.

```
target/index.html        SPA entry, with window.__DBT_DOCS__ injected
target/assets/           hashed JS/CSS
target/index/            site data: parquet copied from the engine index
```

`index.html` lands exactly where dbt Core v1 wrote it, so an existing pipeline that
publishes `target/` keeps working unchanged.

dbt v2 writes the engine index to `target/private/index/` (not a user API). `docs generate`
copies those parquet files to `target/index/` — the static-site URL layout (`DATA_DIR =
"index"`). A self-contained `--output-dir` export does the same under `<dir>/index/`.

Tables that `--write-index` omits because they hold no rows (no run yet, no sources, no catalog) are declared as empty relations in the browser from DDL, which is why the queries need no missing-table variants.

`--output-dir <dir>` still gives a self-contained directory: the index travels along as
a byte-for-byte copy under `<dir>/index/`.

> [!NOTE]
> Publishing `target/` publishes everything in it: `manifest.json`, `run_results.json`,
> compiled SQL, and any stored test failures under `target/data/`. Use
> `--output-dir <dir>` for a self-contained directory holding only the site.

The browser loads DuckDB-WASM from a CDN at runtime (never bundled; override the base with `--duckdb-cdn-base`) and queries the parquet directly. Column-level lineage rides along when the index has it, which is what `--static-analysis strict` on the compile or build produces; the site derives whether the feature is available from whether `dbt.column_lineage.parquet` is present, so nothing else has to be kept in sync.

`dbt docs serve` generates this site when it is missing or older than the index, then serves it. It is a static file host for local preview — a convenience, not a requirement. There is no server-side query engine and no HTTP API: every query the site runs, it runs in the browser.

## ⚙️ How it works

Data comes from parquet files that the dbt v2 engine writes to `<target>/private/index/` when you run with `--write-index`.

```
dbt project
    │  dbt compile --write-index --static-analysis strict
    │  …or nothing: `docs generate` runs that compile unless `--no-compile`
    ▼
<target>/private/index/*.parquet  ← engine index (not a user API)
    │  dbt docs generate — copies parquet to index/ for the site URL layout
    ▼
<target>/index.html        ← hostable anywhere; reads index/ beside it
    │  fetched by the browser
    ▼
DuckDB-WASM, in the page
    ▼
React SPA  (embedded in the binary, hash routing)
```

The SPA is baked into the binary at compile time (the `embed-ui` feature, on by default), so one self-contained executable both generates a site and previews it.

## ✨ Features

- **Full project catalog** — models, sources, seeds, snapshots, tests, unit tests, exposures, groups, macros, metrics, semantic models, and saved queries.
- **Interactive lineage** — node-to-node DAG lineage.
- **Execution info** — last-run status, completion time, and errors surfaced inline on resource detail pages (when run results are present in the artifacts).
- **Statically hostable** — the generated site needs no process and no state, so any file host will do.
- **No live warehouse dependency** — everything is read from the parquet snapshot.

## 🪜 Tiers

The richness of the docs depends on how the artifacts were produced.


## 🚀 Getting started

### 📋 Prerequisites

Generate the artifacts first. From your dbt project, run dbt with `--write-index`:

```bash
dbt --write-index compile      # or: run / build
```

This writes the parquet to `./target/private/index/`.

### 💻 Option A: command line

If you already have a dbt binary installed (core v2 or dbt v2), just run:

```bash
dbt docs serve                 # binds 127.0.0.1:8580, opens a browser tab
```

Useful flags:

| Flag | Env var | Default | Meaning |
|---|---|---|---|
| `--target-path <DIR>` | `DBT_DOCS_TARGET_PATH` | `./target` | Directory whose `private/index/` holds the parquet |
| `--host <HOST>` | `DBT_DOCS_HOST` | `127.0.0.1` | Bind address |
| `--port <PORT>` | `DBT_DOCS_PORT` | `8580` | Listen port |
| `--no-open` | — | off | Don't auto-open a browser tab |

### 📦 Option B: docker compose

A `docker-compose.yml` is included that wires the build, the artifact mount, the port, and a persistent driver-cache volume. Point `DBT_TARGET_PATH` at your project's `target/` directory (an absolute path is recommended) and bring it up:

```bash
DBT_TARGET_PATH=/abs/path/to/project/target docker compose up --build
```

Then open <http://localhost:8580>.

Set `DBT_VERSION` to pin a dbt version instead of tracking the newest release:

```bash
DBT_VERSION=2.0.0-beta.2 DBT_TARGET_PATH=/abs/path/to/project/target docker compose up --build
```

### 🐋 Option C: Docker

A `Dockerfile` is included. It downloads a released dbt binary from this repo's [GitHub Releases](https://github.com/dbt-labs/dbt-core/releases) and verifies it against the release's published `SHA256SUMS`, so the build is quick and needs no cargo toolchain. It is a multi-stage BuildKit build, produces `linux/amd64` and `linux/arm64`, and runs as an unprivileged user.

Nothing is copied out of the build context, so the context is this crate's directory and the command is the same whether you run it from the repo root or from `crates/dbt-docs-server/`:

```bash
docker build -t dbt-docs-server crates/dbt-docs-server
```

By default the newest release carrying a Linux binary is resolved at build time. Pin a version with `--build-arg DBT_VERSION=<version>`:

```bash
docker build -t dbt-docs-server --build-arg DBT_VERSION=2.0.0-beta.2 crates/dbt-docs-server
```

> **On `latest` and the build cache.** Because the default build args don't change between builds, Docker reuses the cached download layer and keeps whatever version it resolved the first time. To pick up a newer release, build with `--no-cache` or pass the version explicitly.

Run it, mounting a project `target/` that already contains `target/private/index/*.parquet`:

```bash
docker run --rm -p 8580:8580 \
  -v "$PWD/target:/data/target:ro" \
  dbt-docs-server
```

Then open <http://localhost:8580>.

> **The artifact mount is required.** With no `target/private/index/*.parquet` behind `/data/target`, the server has nothing to serve and the container exits on startup. Generate them first with `dbt --write-index` (see [Prerequisites](#-prerequisites)).

**First-run network note.** The parquet is queried through an ADBC DuckDB driver that is **not** bundled in the image. On first boot the driver is downloaded from `public.cdn.getdbt.com` into the cache dir (`/var/cache/dbt`). The container therefore needs outbound HTTPS the first time it runs. 

To avoid re-downloading on every run — and to run fully offline once warmed — persist the cache with a named volume:

```bash
docker run --rm -p 8580:8580 \
  -v "$PWD/target:/data/target:ro" \
  -v dbt-adbc-cache:/var/cache/dbt \
  dbt-docs-server
```

### 🔨 Option D: build from source

> **Coming soon.**

## 🎨 The web UI

The SPA lives in `web/`. Its built output, `web/dist/`, is **committed to the repo**
and embedded into the binary at compile time by `rust-embed`.

That means **building the server needs no JavaScript toolchain at all**:

```bash
cargo build -p dbt-cli   # embeds the committed web/dist/ as-is
```

### Rebuilding the UI

Only needed if you change anything under `web/`. The SPA has no private
dependencies — everything it needs is on the public npm registry, no token
required:

```bash
cd crates/dbt-docs-server/web
pnpm install
pnpm build          # writes web/dist/
```

Other useful commands, all from `web/`:

```bash
pnpm dev            # vite dev server on :3002; set DBT_DOCS_DEV_SITE=<a generated site> for real data
pnpm test           # vitest
pnpm typecheck
pnpm lint           # eslint + prettier
```

> [!IMPORTANT]
> **Always rebuild `web/dist/` and commit it in the same change as any `web/` source
> edit.** This is a manual step — nothing in CI or in a git hook rebuilds or checks
> the bundle, so a source-only commit will silently ship a stale UI.

### Customizing the look

The UI ships with no dbt branding baked in on purpose — it's meant to be
white-labeled for whoever is hosting it. The common tweaks, none of which need
React or component knowledge:

- **Accent color** — edit the `--bgBrand*` custom properties in
  [`web/src/styles/tokens.css`](./web/src/styles/tokens.css). Both light and
  dark variants live in that file.
- **Page/tab title** — edit the `<title>` tag in
  [`web/index.html`](./web/index.html).
- **Favicon** — none is set by default. Add an icon file under `web/public/`
  and a `<link rel="icon" href="/your-icon.ico">` tag in `web/index.html`.

Rebuild (`pnpm build`) and commit `web/dist/` after any of these, same as any
other `web/` change.

## 🤝 Contributing

Development happens in the [`dbt-labs/dbt-core`](https://github.com/dbt-labs/dbt-core) monorepo, under `crates/dbt-docs-server`.

## 📄 License

Licensed under the Apache License, Version 2.0.

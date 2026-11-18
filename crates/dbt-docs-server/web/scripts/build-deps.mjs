// Builds this package's workspace dependencies, that is dbt-docs-lib, through turbo.
// `dist/` bundles the lib, and the type checks, tests and Storybook import its build,
// so a script that needs it runs this first: `node scripts/build-deps.mjs && <command>`.
// pnpm appends a script's arguments to its end, so they still reach <command> alone.
// Under turbo, which sets `TURBO_HASH` for each task it runs, the dependencies are
// built already and this does nothing. Long-running scripts use `watch.mjs` instead.
import { spawnSync } from 'node:child_process';

if (!process.env.TURBO_HASH) {
  // Through a shell, which runs pnpm's `.cmd` shim on Windows. The quotes stop cmd.exe
  // from reading `^` as an escape character.
  const { status } = spawnSync('pnpm turbo run build "--filter=dbt-docs-web^..."', {
    stdio: 'inherit',
    shell: true,
  });
  if (status !== 0) process.exit(status ?? 1);
}

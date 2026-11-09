// Runs a long-running command, such as the dev server, under `turbo watch`, as the
// workspace root's `pnpm dev` does: turbo builds dbt-docs-lib first and rebuilds it
// whenever its sources change, and the command picks up each new build. A script
// passes its command as one argument, `node scripts/watch.mjs "<command>"`, and needs
// a turbo task of the same name, which turbo runs by running the script again with
// `TURBO_HASH` set. That time, it runs the command. Arguments to the script reach the
// command either way: turbo passes them on to that task only, never to the lib's build.
// Under turbo the command's stdin is not a terminal, so it has no keyboard shortcuts,
// and vitest needs `watch` spelled out or it runs once.
import { spawnSync } from 'node:child_process';

const [command, ...args] = process.argv.slice(2);
const argv = process.env.TURBO_HASH
  ? [...command.split(' '), ...args]
  : [
      'pnpm',
      'turbo',
      'watch',
      process.env.npm_lifecycle_event,
      '--filter=dbt-docs-web',
      '--',
      ...args,
    ];
const { status } = spawnSync(argv[0], argv.slice(1), {
  stdio: 'inherit',
  // pnpm and the commands are `.cmd` shims on Windows, and only a shell can run one.
  shell: process.platform === 'win32',
});
process.exit(status ?? 1);

import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { storybookTest } from '@storybook/addon-vitest/vitest-plugin';
import react from '@vitejs/plugin-react';
import { defineConfig } from 'vitest/config';

const dirname = path.dirname(fileURLToPath(import.meta.url));

/**
 * Two test projects, run by the same `vitest`.
 *
 * `unit` is the suite that has always been here — plain vitest in happy-dom. It is
 * unchanged, and `pnpm test` still runs exactly it and nothing else, so adding
 * Storybook did not quietly make the fast suite depend on a browser download.
 *
 * `storybook` runs every story as a component test through
 * `@storybook/addon-vitest`, in real Chromium via playwright. Stories are already the
 * canonical description of each component's states, so this is the cheapest possible
 * coverage of them: nothing to write per story, and a story that throws or fails its
 * play function fails the build. A real browser is also the point — layout,
 * `getBoundingClientRect` and the react-flow lineage canvas do not work in happy-dom,
 * so these are states the unit suite structurally cannot assert on.
 *
 * New component tests should go in a story's `play` function, which both suites'
 * readers can see rendered.
 *
 * There is deliberately no setup file for the `storybook` project: since Storybook
 * 10.3 the addon applies `.storybook/preview.tsx` itself, and a setup file calling
 * `setProjectAnnotations` makes it skip that automatic wiring instead.
 */
export default defineConfig({
  test: {
    projects: [
      {
        plugins: [react()],
        test: {
          name: 'unit',
          globals: true,
          environment: 'happy-dom',
          setupFiles: ['./vitest.setup.ts'],
          include: ['src/**/*.test.{ts,tsx}'],
          css: false,
          server: {
            deps: {
              // Let vite transform these rather than loading them through node's ESM
              // resolver. `@dbt-labs/sourdough` pulls CJS-only deps (react-use), whose
              // named exports node refuses to bind; vite's interop handles them.
              inline: [/@dbt-labs\//, 'react-use'],
            },
          },
        },
      },
      {
        // No `react()` here: `storybookTest` builds through `.storybook/main.ts`, which
        // already applies the app's own vite config (react + the compiler babel
        // plugin). Adding it again would run the transform twice.
        plugins: [
          storybookTest({
            configDir: path.join(dirname, '.storybook'),
            // Lets the Storybook UI's test widget start the dev server itself.
            storybookScript: 'pnpm storybook --no-open',
          }),
        ],
        test: {
          name: 'storybook',
          browser: {
            enabled: true,
            provider: 'playwright',
            headless: true,
            instances: [{ browser: 'chromium' }],
            // Run story files one at a time. In parallel, the browser workers race
            // each other badly enough to fail in ways that have nothing to do with the
            // stories: "Vitest failed to find the runner" on the addon's setup file,
            // and React's dispatcher coming back null (`Cannot read properties of null
            // (reading 'useState')`) from what looks like a torn module registry. Every
            // affected file passes on its own. Each is only a few hundred milliseconds,
            // so serialising costs far less than the flakes did.
            //
            // Set here rather than as `test.fileParallelism`, which vitest only accepts
            // at the root and would therefore serialise the unit project too.
            fileParallelism: false,
          },
        },
      },
    ],
  },
});

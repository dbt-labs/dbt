import type { StorybookConfig } from '@storybook/react-vite';

/**
 * Storybook for the dbt docs v2 SPA.
 *
 * Stories are colocated with their components (`Foo.stories.tsx` next to `Foo.tsx`),
 * the same convention the `.test.tsx` files follow.
 *
 * The builder picks up `vite.config.ts` automatically, which is what we want for the
 * React-compiler babel plugin and the `dedupe` list — sourdough, react-query and react
 * must resolve to one copy each or context lookups silently miss. The one thing we do
 * not want from it is the `devSite` plugin: it exists to point `pnpm dev` at a
 * generated site's parquet, and stories deliberately render from in-memory fakes
 * instead, so leaving it in would only emit a "no generated site" warning on every
 * boot.
 */
const config: StorybookConfig = {
  stories: ['../src/**/*.stories.@(ts|tsx)'],

  addons: [
    '@storybook/addon-docs',
    // Surfaces the same axe rules the shared eslint config's jsx-a11y rules can only
    // check statically.
    '@storybook/addon-a11y',
    // The app flips `.dark` / `.light` on <html> (see `src/hooks/useTheme.ts`); the
    // toolbar switcher in `preview.tsx` drives that same class.
    '@storybook/addon-themes',
    // Runs every story as a Vitest component test in real Chromium, and adds the
    // "Run tests" widget to the sidebar. Configured as the `storybook` project in
    // `vitest.config.ts`.
    '@storybook/addon-vitest',
  ],

  framework: {
    name: '@storybook/react-vite',
    options: {},
  },

  core: {
    // The bundle is committed and embedded in the `dbt` binary; nothing here should
    // phone home from a developer's machine.
    disableTelemetry: true,
  },

  viteFinal(viteConfig) {
    return {
      ...viteConfig,
      plugins: (viteConfig.plugins ?? []).filter(
        (plugin) =>
          !(plugin && 'name' in plugin && plugin.name === 'dbt-docs-dev-site'),
      ),
      build: {
        ...viteConfig.build,
        // `vite.config.ts` pins a single `index.html` input and hand-rolled
        // manualChunks for the app bundle. Storybook builds its own entries, so both
        // would fight it.
        rollupOptions: {},
      },
    };
  },
};

export default config;

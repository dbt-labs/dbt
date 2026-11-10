import { useState, type ReactNode } from 'react';
// import { HelmetProvider } from 'react-helmet-async';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { withThemeByClassName } from '@storybook/addon-themes';
import type { Decorator, Preview } from '@storybook/react-vite';
import { action } from 'storybook/actions';

// import { BootstrapProvider } from '../src/lib/bootstrapContext';
import type { MetadataDataSource } from '../lib/data-sources/MetadataDataSource';
import { MetadataDataProvider } from '../lib/context/MetadataDataProvider';
import {
  type LinkTo,
  type Navigation,
  NavigationProvider,
} from '../lib/context/NavigationProvider';
import { type BootstrapData, storyBootstrapData } from '../lib/shared/testing/storyFixtures';
import { storyDataSource } from '../lib/shared/testing/storySources';
import { ReactFlow, ReactFlowProvider } from '@xyflow/react';

// The whole app stylesheet: biga tokens, dbt-dag's react-flow CSS, sourdough, tailwind,
// then `app.css`. Components here are styled almost entirely by CSS custom properties
// from that chain, so importing anything less renders them colorless.
import '../lib/index.css';

const hrefOf = (to: LinkTo) =>
  typeof to === 'string' ? to : `${to.pathname ?? ''}${to.search ?? ''}${to.hash ?? ''}`;

/**
 * Stands in for the host app's router, which this package does not have: links render
 * as plain anchors carrying the href the app would build, and navigation is logged to
 * the Actions panel instead of performed.
 */
const storyNavigation: Navigation = {
  // Stand-ins: with no app behind them, a path only has to read well in the panel.
  paths: {
    home: () => '/',
    details: (uniqueId) => `/details/${uniqueId}/`,
    lineage: (uniqueId) => `/lineage/?uniqueId=${uniqueId}`,
  },
  navigate: action('navigate'),
  href: hrefOf,
  Link: ({ to, target, rel, className, children, onClick }) => (
    <a
      href={hrefOf(to)}
      target={target}
      rel={rel}
      className={className}
      onClick={(event) => {
        onClick?.(event);
        event.preventDefault();
        storyNavigation.navigate(hrefOf(to));
      }}
    >
      {children}
    </a>
  ),
};

/**
 * Per-story overrides for the provider stack.
 *
 * Declared as a parameter rather than a set of args because none of it is a prop of
 * the component under test — it is the environment the component is mounted in, and
 * args are reserved for the real props so the controls panel stays meaningful.
 */
export interface DocsStoryParameters {
  /** Backing data source. Defaults to the fully-populated `storyDataSource()`. */
  source?: MetadataDataSource;
  /** The first-paint node read, for components calling `useAllNodes`. Defaults to
   *  the resolved story fixture — the context's own default never settles. */
  bootstrap?: BootstrapData | Promise<BootstrapData>;
}

function Providers({
  children,
  source,
  bootstrap,
}: DocsStoryParameters & { children: ReactNode }) {
  // One client per story mount. Created in state rather than in the render body so a
  // control change re-renders against the same cache instead of silently dropping it.
  const [queryClient] = useState(
    () =>
      new QueryClient({
        // No retries: a story that shows an error state should show it immediately
        // rather than after three backoffs.
        defaultOptions: { queries: { retry: false } },
      }),
  );
  const [read] = useState(() =>
    bootstrap instanceof Promise
      ? bootstrap
      : Promise.resolve(bootstrap ?? storyBootstrapData()),
  );

  return (
    <QueryClientProvider client={queryClient}>
        <MetadataDataProvider source={source ?? storyDataSource()}>
          {/* `#/` matches `main.tsx`: the real app is hash-routed so the built
              directory works at any path. Keeping the prefix identical means a story
              renders the same hrefs the shipped app does. */}
          {/* <LinkPrefixProvider prefix="#/">
            <BootstrapProvider value={read}> */}
              <ReactFlowProvider>
                <NavigationProvider navigation={storyNavigation}>
                  {children}
                </NavigationProvider>
              </ReactFlowProvider>
            {/* </BootstrapProvider>
          </LinkPrefixProvider> */}
        </MetadataDataProvider>
    </QueryClientProvider>
  );
}

/** Mounts every story under the same provider stack `main.tsx` builds. */
const withProviders: Decorator = (Story, context) => {
  const { source, bootstrap } = (context.parameters.docsApp ?? {}) as DocsStoryParameters;
  return (
    <Providers source={source} bootstrap={bootstrap}>
      <Story />
    </Providers>
  );
};

const preview: Preview = {
  decorators: [
    withProviders,
    withThemeByClassName({
      themes: { dark: 'dark', light: 'light' },
      // Dark is the app's default (`hooks/useTheme.ts`), so it is the default here too
      // — a component reviewed only in light mode is a component half-reviewed.
      defaultTheme: 'dark',
      // `useTheme` toggles the class on <html>, not on the story root.
      parentSelector: 'html',
    }),
  ],

  parameters: {
    // `app.css` paints the page background from `--bgMain`; the addon's own
    // backgrounds would sit on top of it and disagree with the theme switcher.
    backgrounds: { disable: true },

    controls: {
      matchers: {
        color: /(background|color)$/i,
        date: /(At|Date)$/,
      },
    },

    a11y: {
      // Report violations in the panel rather than failing the story: several
      // sourdough primitives have known contrast findings that are not this app's to
      // fix, and a hard failure would hide new regressions among them.
      test: 'todo',
    },
  },

  // Opts every story into an autogenerated docs page.
  tags: ['autodocs'],
};

export default preview;

import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { HashRouter } from 'react-router-dom';
import { QueryClientProvider } from '@tanstack/react-query';
import { ReactFlowProvider } from '@xyflow/react';

import App from './App';
import { BootstrapProvider } from './lib/bootstrapContext';
import { dataBaseUrl, readSiteBootstrap } from './lib/siteBootstrap';
import { queryClient } from './queryClient';
import { LinkPrefixProvider, MetadataDataProvider, wrapDataSource } from './shared';
import { createDuckDbDataSource } from './shared/data-sources/duckdb';

import './index.css';

const root = document.getElementById('root');
if (!root) throw new Error('missing #root element');

/**
 * Every deployment is a static site now.
 *
 * `window.__DBT_DOCS__` is injected by `dbt docs generate`, and in development by the
 * `devSite` Vite plugin. Its absence used to mean "talk to the API"; there is no API
 * left, so it means the page was served by something that did not generate it — a bad
 * copy, a stale `index.html`, a hand-rolled host. Failing loudly beats rendering an
 * app with no data layer.
 */
const siteBootstrap = readSiteBootstrap();
if (!siteBootstrap) {
  throw new Error(
    'dbt docs: missing window.__DBT_DOCS__. Serve a directory written by ' +
      '`dbt docs generate` — index.html carries the site configuration and cannot ' +
      'be substituted.',
  );
}

// Start the first-paint read immediately, before React mounts: the shell needs it, so
// the request should already be in flight while the bundle finishes evaluating.
// Dynamically imported to keep the parquet reader and its decompressors out of the
// entry chunk.
const bootstrapData = import('./shared/data-sources/duckdb/bootstrap').then((m) =>
  m.readBootstrap(dataBaseUrl()),
);

const dataSource = createDuckDbDataSource({
  dataBaseUrl: dataBaseUrl(),
  bootstrap: siteBootstrap,
  data: bootstrapData,
});

createRoot(root).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      {/*
        Hash routing, so the output directory works unmodified at any path on any
        host. Real paths need the host to answer unknown URLs with `index.html`;
        GitLab Pages project sites live under `/<group>/<project>/` and plain file
        servers rewrite nothing. The hash never reaches the server, which also
        keeps `document.baseURI` stable for resolving `data/` URLs.
      */}
      <HashRouter>
        <LinkPrefixProvider prefix="/">
          <ReactFlowProvider>
            <BootstrapProvider value={bootstrapData}>
              <MetadataDataProvider source={wrapDataSource(dataSource, queryClient)}>
                <App />
              </MetadataDataProvider>
            </BootstrapProvider>
          </ReactFlowProvider>
        </LinkPrefixProvider>
      </HashRouter>
    </QueryClientProvider>
  </StrictMode>,
);

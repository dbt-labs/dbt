import type { ReactElement, ReactNode } from 'react';
import { HelmetProvider } from 'react-helmet-async';
import { MemoryRouter } from 'react-router-dom';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, type RenderOptions, type RenderResult } from '@testing-library/react';

import {
  LinkPrefixProvider,
  MetadataDataProvider,
  type MetadataDataSource,
} from '../shared';
import { createFakeDataSource } from '../shared/testing/createFakeDataSource';

/** A fresh QueryClient per render so cache never leaks between tests. Retries
 *  are off (failures surface immediately) and gcTime is 0 (no lingering data). */
export function makeTestQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
}

interface ProviderOptions {
  /** Initial history entries for the MemoryRouter. */
  initialEntries?: string[];
  /** Override the metadata data source. Defaults to a fully-capable fake, so a test
   *  only describes the surface it exercises. */
  source?: MetadataDataSource;
}

function Providers({
  children,
  initialEntries,
  source,
}: ProviderOptions & { children: ReactNode }) {
  return (
    <QueryClientProvider client={makeTestQueryClient()}>
      <HelmetProvider>
        <MetadataDataProvider
          source={source ?? createFakeDataSource({}, { full: true })}
        >
          <LinkPrefixProvider prefix="/">
            <MemoryRouter initialEntries={initialEntries}>{children}</MemoryRouter>
          </LinkPrefixProvider>
        </MetadataDataProvider>
      </HelmetProvider>
    </QueryClientProvider>
  );
}

/** Render a component under the full provider stack (react-query + Helmet +
 *  MetadataDataProvider + MemoryRouter) that the app expects at runtime. */
export function renderWithProviders(
  ui: ReactElement,
  {
    initialEntries,
    source,
    ...options
  }: ProviderOptions & Omit<RenderOptions, 'wrapper'> = {},
): RenderResult {
  return render(ui, {
    wrapper: ({ children }) => (
      <Providers initialEntries={initialEntries} source={source}>
        {children}
      </Providers>
    ),
    ...options,
  });
}

/** Wrapper for `renderHook` — provides a fresh QueryClient. */
export function createQueryWrapper() {
  const queryClient = makeTestQueryClient();
  return function QueryWrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

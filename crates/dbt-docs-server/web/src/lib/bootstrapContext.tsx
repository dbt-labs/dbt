/**
 * Carries the hyparquet first-paint read down to the hooks that need it.
 *
 * `MetadataDataSource` has no "every node in the project" method — the sidebar, the
 * file tree, and the resource-type resolver all want the whole index at once, which is
 * not a shape the per-type list methods express — so the node list cannot ride the
 * data-source seam. A context keeps it explicit and scoped instead of a mutable module
 * global.
 */

import { createContext, type ReactNode, useContext } from 'react';

import type { BootstrapData } from '../shared/data-sources/duckdb/bootstrap';

/**
 * The in-flight read.
 *
 * The default is a promise that never settles rather than `null`: a consumer rendered
 * outside a provider should sit in its loading state, not branch on an absence that
 * `main.tsx` has already ruled out by throwing.
 */
const BootstrapContext = createContext<Promise<BootstrapData>>(
  new Promise<BootstrapData>(() => {}),
);

export function BootstrapProvider({
  value,
  children,
}: {
  value: Promise<BootstrapData>;
  children: ReactNode;
}) {
  return (
    <BootstrapContext.Provider value={value}>{children}</BootstrapContext.Provider>
  );
}

/** The first-paint read. Always present in a generated site. */
export function useBootstrapData(): Promise<BootstrapData> {
  return useContext(BootstrapContext);
}

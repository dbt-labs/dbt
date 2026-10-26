import { createContext, type ReactNode, useContext } from 'react';

import type { MetadataDataSource } from '../data-sources/MetadataDataSource';

const MetadataDataSourceContext = createContext<MetadataDataSource | undefined>(
  undefined,
);

export const MetadataDataProvider = ({
  source,
  children,
}: {
  source: MetadataDataSource;
  children: ReactNode;
}) => (
  <MetadataDataSourceContext.Provider value={source}>
    {children}
  </MetadataDataSourceContext.Provider>
);

/** Read the active data source. Throws if no {@link MetadataDataProvider} is in
 *  the tree — mirrors `useLinkPrefixRequired`. */
export const useMetadataDataSource = (): MetadataDataSource => {
  const source = useContext(MetadataDataSourceContext);
  if (source === undefined) throw new Error('MetadataDataProvider not found in tree');
  return source;
};

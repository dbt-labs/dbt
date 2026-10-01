import type { ComponentProps } from 'react';

import { AssetListView } from '../components/AssetListView';

/** `:resourceType` route param drives narrowing via `filters.resourceType`
 *  (synced in App.tsx); AssetListView reads filters directly. */
export default function ResourceFilter(props: ComponentProps<typeof AssetListView>) {
  return <AssetListView {...props} />;
}

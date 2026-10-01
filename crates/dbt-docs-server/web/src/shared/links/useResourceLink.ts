import { useMemo } from 'react';

import {
  homeLink,
  packageDetailsLink,
  resourceDetailsLink,
  resourceFilterLink,
} from './linkGenerators';
import { useLinkPrefixRequired } from './LinkPrefixContext';
import type {
  PackageDetailsParams,
  ResourceDetailsParams,
  ResourceFilterParams,
} from './linkTypes';

export const useResourceLink = () => {
  const prefix = useLinkPrefixRequired();
  return useMemo(
    () => ({
      home: () => homeLink(prefix),
      resourceDetails: (p: ResourceDetailsParams) => resourceDetailsLink(prefix, p),
      resourceFilter: (p: ResourceFilterParams) => resourceFilterLink(prefix, p),
      packageDetails: (p: PackageDetailsParams) => packageDetailsLink(prefix, p),
    }),
    [prefix],
  );
};

import type {
  PackageDetailsParams,
  ResourceDetailsParams,
  ResourceFilterParams,
} from './linkTypes';

export const homeLink = (prefix: string | undefined): string | undefined => prefix;

export const resourceFilterLink = (
  prefix: string | undefined,
  p: ResourceFilterParams,
): string | undefined =>
  prefix !== undefined
    ? `${prefix}resource/${encodeURIComponent(p.resourceType)}/`
    : undefined;

export const resourceDetailsLink = (
  prefix: string | undefined,
  p: ResourceDetailsParams,
): string | undefined =>
  prefix !== undefined
    ? `${prefix}details/${encodeURIComponent(p.dbtUniqueId)}/`
    : undefined;

export const packageDetailsLink = (
  prefix: string | undefined,
  p: PackageDetailsParams,
): string | undefined =>
  prefix !== undefined
    ? `${prefix}package/${encodeURIComponent(p.packageName)}/`
    : undefined;

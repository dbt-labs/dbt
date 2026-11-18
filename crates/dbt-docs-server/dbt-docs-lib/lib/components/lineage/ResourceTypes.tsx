import { z } from 'zod';

export const BUILT_IN_RESOURCE_TYPES = [
  'model',
  'source',
  'seed',
  'exposure',
  'test',
  'group',
  'metric',
  'semantic_model',
  'macro',
  'snapshot',
  'saved_query',
  'analysis',
  'unit_test',
  'function',
] as const;

export type BuiltInResourceType = (typeof BUILT_IN_RESOURCE_TYPES)[number];
export type ResourceType = BuiltInResourceType | (string & {});
export const ResourceType = z.string();

export const DEFAULT_VISIBLE_RESOURCE_TYPES = new Set<ResourceType>([
  'model',
  'source',
  'seed',
  'exposure',
]);

/** The types the resource filter can't switch off: models, and the root's own type,
 *  so filtering never hides the node the lineage is about. The type is the prefix of
 *  the root's unique_id (`<resource_type>.<package>.<...>`). */
export function alwaysVisibleResourceTypes(
  rootUniqueId: string | null,
): Set<ResourceType> {
  const types = new Set<ResourceType>(['model']);
  if (rootUniqueId) types.add(rootUniqueId.split('.')[0]);
  return types;
}

export const DEFAULT_ADDITIONAL_RESOURCE_TYPES: Set<ResourceType> =
  new Set<ResourceType>(BUILT_IN_RESOURCE_TYPES).difference(
    DEFAULT_VISIBLE_RESOURCE_TYPES,
  );

export const DEFAULT_RESOURCE_TYPE_FILTER_LABELS: Record<ResourceType, string> = {
  model: 'Models',
  exposure: 'Exposures',
  source: 'Sources',
  seed: 'Seeds',
  test: 'Tests',
  group: 'Groups',
  metric: 'Metrics',
  semantic_model: 'Semantic models',
  macro: 'Macros',
  snapshot: 'Snapshots',
  saved_query: 'Saved queries',
  function: 'Functions',
  analysis: 'Analyses',
  unit_test: 'Unit tests',
};

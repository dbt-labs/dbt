import type { AssetArgs, ResourceType } from '../shared';
import type { NodeSummary } from '../types';

/**
 * Resource types whose `unique_id` is a bare name (no
 * `<resource_type>.<package>.<name>` prefix). Today this is only
 * `saved_query` — `dbt.saved_queries` holds ids like
 * `dbt_invocations_by_billing_email`. Used as the inference fallback when a
 * dotless id arrives via the URL and the caller didn't pass `resourceType`.
 */
const BARE_ID_RESOURCE_TYPE = 'saved_query';

/**
 * Infer the resource type from a unique_id. Most follow
 * `<resource_type>.<package>.<name>`; dotless ids fall back to
 * {@link BARE_ID_RESOURCE_TYPE}.
 */
export function inferResourceType(uniqueId: string): string {
  if (uniqueId.includes('.')) return uniqueId.split('.')[0] ?? '';
  return BARE_ID_RESOURCE_TYPE;
}

/**
 * Resolve the `{ uniqueId, resourceType }` a detail fetch needs from a unique_id
 * plus the loaded `nodes` list. Prefer the list's authoritative `resource_type`;
 * fall back to prefix inference for ids absent from `nodes` (e.g. saved_query).
 * Returns `null` when there's no id or no resolvable type — the single tested
 * unit for both detail call sites in `App.tsx`.
 */
export function resolveAssetArgs(
  uniqueId: string | null,
  nodes: NodeSummary[] | null | undefined,
): AssetArgs | null {
  if (!uniqueId) return null;
  const fromList = nodes?.find((n) => n.unique_id === uniqueId)?.resource_type;
  const resourceType = fromList ?? (inferResourceType(uniqueId) || undefined);
  if (!resourceType) return null;
  return { uniqueId, resourceType: resourceType as ResourceType };
}

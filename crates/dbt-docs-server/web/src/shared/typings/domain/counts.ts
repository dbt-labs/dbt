import type { ResourceType } from './asset';

/** Project-wide per-resource-type asset tallies. A source omits types it has no
 *  count for; consumers fall back to a client-side derive for absent types.
 *  Mirrors dbt-docs-server's `GET /api/v1/nodes/counts` (where `unit_test` folds
 *  into `test`). */
export type AssetCounts = Partial<Record<ResourceType, number>>;

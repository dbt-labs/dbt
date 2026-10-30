import type { Asset, AssetColumn, ModelAsset, RelationshipItem } from '../shared';

/** Columns for assets that carry them (models/seeds/snapshots/sources); `[]` otherwise. */
export function getColumns(asset: Asset): AssetColumn[] {
  if ('columns' in asset) return (asset as ModelAsset).columns;
  return [];
}

/** Build a relationship list item from a bare `unique_id`. */
export function toRelationshipItem(uniqueId: string): RelationshipItem {
  return {
    uniqueId,
    name: uniqueId.split('.').slice(2).join('.') || uniqueId,
    resourceType: uniqueId.split('.')[0] ?? 'unknown',
  };
}

import type { ResourceTypeExplorer } from '../../lib/resourceType';
import type { AssetHeaderProps } from '../components/AssetHeader';
import type { Asset, Relation } from '../typings/domain/asset';

function relationOf(asset: Asset): Relation | null {
  return 'relation' in asset ? (asset.relation ?? null) : null;
}

/** Bridge the domain {@link Asset} union to the presentational `AssetHeader`. */
export function assetToHeaderProps(asset: Asset): AssetHeaderProps {
  const relation = relationOf(asset);
  return {
    name: asset.name,
    // Domain `ResourceType` is a subset of `ResourceTypeExplorer` aside from
    // `operation`, which has no explorer icon; render it as-is.
    resourceType: asset.resourceType as ResourceTypeExplorer,
    packageName: asset.packageName || null,
    description: asset.description,
    relation: relation
      ? {
          database: relation.database,
          schema: relation.schema,
          identifier: relation.identifier,
        }
      : null,
  };
}

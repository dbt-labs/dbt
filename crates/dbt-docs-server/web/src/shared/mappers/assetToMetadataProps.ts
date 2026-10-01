import type { AssetMetadataProps } from '../components/AssetMetadata';
import type { Asset, Relation } from '../typings/domain/asset';

function relationOf(asset: Asset): Relation | null {
  return 'relation' in asset ? (asset.relation ?? null) : null;
}

/** Bridge the domain {@link Asset} union to the presentational `AssetMetadata`.
 *  Pulls resource-specific fields by narrowing on `resourceType`. */
export function assetToMetadataProps(asset: Asset): AssetMetadataProps {
  const relation = relationOf(asset);
  const props: AssetMetadataProps = {
    resourceType: asset.resourceType,
    uniqueId: asset.uniqueId,
    packageName: asset.packageName || null,
    relation: relation
      ? {
          database: relation.database,
          schema: relation.schema,
          identifier: relation.identifier,
        }
      : null,
    tags: asset.tags,
    meta: asset.meta ?? null,
    filePath: asset.filePath ?? null,
  };

  switch (asset.resourceType) {
    case 'model':
    case 'seed':
    case 'snapshot':
      props.language = asset.language ?? undefined;
      props.accessLevel = asset.access ?? undefined;
      props.contractEnforced = asset.contractEnforced ?? undefined;
      props.materialization = asset.materializedType ?? undefined;
      props.group = asset.group ?? undefined;
      break;
    case 'source':
      props.loader = asset.loader ?? undefined;
      break;
    case 'exposure':
      props.exposureType = asset.exposureType;
      props.maturity = asset.maturity ?? undefined;
      props.url = asset.url ?? undefined;
      props.ownerName = asset.ownerName ?? undefined;
      props.ownerEmail = asset.ownerEmail ?? undefined;
      break;
    case 'metric':
      props.group = asset.group ?? undefined;
      break;
    default:
      break;
  }

  return props;
}

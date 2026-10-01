import { useAssetDetail } from '../hooks/useAssetDetail';
import { assetToHeaderProps } from '../mappers/assetToHeaderProps';
import { assetToMetadataProps } from '../mappers/assetToMetadataProps';
import type { AssetArgs } from '../typings/args';
import type { Asset, AssetColumn } from '../typings/domain/asset';
import { AssetCode } from './AssetCode';
import { AssetColumns } from './AssetColumns';
import { AssetHeader } from './AssetHeader';
import { AssetMetadata } from './AssetMetadata';
import { Spinner } from './Spinner';

function codeOf(
  asset: Asset,
): { rawCode: string | null; compiledCode: string | null } | null {
  if ('rawCode' in asset)
    return {
      rawCode: asset.rawCode,
      compiledCode: 'compiledCode' in asset ? asset.compiledCode : null,
    };
  return null;
}

function columnsOf(asset: Asset): AssetColumn[] | null {
  return 'columns' in asset ? asset.columns : null;
}

/** Presentational composition of a fully-resolved domain {@link Asset}. */
export function AssetDetailView({ asset }: { asset: Asset }) {
  const code = codeOf(asset);
  const columns = columnsOf(asset);
  return (
    <div>
      <AssetHeader {...assetToHeaderProps(asset)} />
      <AssetMetadata {...assetToMetadataProps(asset)} />
      {code?.rawCode && (
        <AssetCode rawCode={code.rawCode} compiledCode={code.compiledCode} />
      )}
      {columns && columns.length > 0 && <AssetColumns columns={columns} />}
    </div>
  );
}

export type AssetDetailProps = { asset: Asset } | AssetArgs;

function AssetDetailLoader(args: AssetArgs) {
  const { data, isLoading } = useAssetDetail(args);
  if (isLoading) return <Spinner />;
  if (!data) return null;
  return <AssetDetailView asset={data} />;
}

/**
 * Render an asset's detail surface. Pass a resolved `asset` to render directly,
 * or pass {@link AssetArgs} to fetch via {@link useAssetDetail}.
 */
export function AssetDetail(props: AssetDetailProps) {
  if ('asset' in props) return <AssetDetailView asset={props.asset} />;
  return <AssetDetailLoader {...props} />;
}

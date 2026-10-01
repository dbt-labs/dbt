import type { FC, ReactNode } from 'react';

import { Link } from '../../components/ui/Link';
import { Tooltip } from '../../components/ui/Tooltip';
import {
  RESOURCE_TYPE_SINGULAR,
  type ResourceTypeExplorer,
} from '../../lib/resourceType';
import { useResourceLink } from '../links/useResourceLink';
import { DataPlatformChip } from './DataPlatformChip';
import { PageHeading } from './PageHeading';
import { SimpleLinkBreadcrumbs } from './SimpleLinkBreadcrumbs';

export type AssetHeaderIconItem = {
  icon: ReactNode;
  /** Text label. Ignored when `Contents` is provided. */
  text?: string;
  /** Custom inner content; takes precedence over `text` when set. */
  Contents?: FC;
  tooltip?: string;
  /** When set, the icon+label is wrapped in an external link. */
  href?: string;
};

export type AssetHeaderProps = {
  name: string;
  resourceType: ResourceTypeExplorer;
  packageName: string | null;
  /** Extra icon+text pairs shown below the heading (resource type, materialization, etc.) */
  headerIcons?: AssetHeaderIconItem[];
  /** Pre-formatted label shown under the heading (e.g. "Last run: …"). */
  lastRunLabel?: string;
  /** Optional element rendered before the heading (e.g. AutoExposureIcon). */
  leftAdornment?: ReactNode;
  /** When set, replaces the heading text (e.g. version dropdown). Breadcrumbs still use `name`. */
  headingOverride?: ReactNode;
  actions?: ReactNode;
  rightAdornment?: ReactNode;
  // TODO: render relation and description when design is finalized
  relation?: {
    database: string | null;
    schema: string | null;
    identifier: string | null;
  } | null;
  description?: string | null;
};

export function AssetHeader({
  name,
  resourceType,
  packageName,
  headerIcons,
  lastRunLabel,
  leftAdornment,
  headingOverride,
  actions,
  rightAdornment,
}: AssetHeaderProps) {
  const links = useResourceLink();
  return (
    <div>
      <SimpleLinkBreadcrumbs
        className="font-caption mb-3 block text-fgDecorative"
        breadcrumbs={[
          { text: packageName ?? '—', href: links.home() },
          {
            text: RESOURCE_TYPE_SINGULAR[resourceType],
            href: links.resourceFilter({ resourceType }),
          },
          { text: name },
        ]}
      />
      <div className="flex">
        <span className="h-10 min-w-0 flex-1">
          <div className="flex items-center overflow-hidden truncate whitespace-nowrap">
            {leftAdornment}
            <PageHeading
              className="flex w-min items-center gap-2"
              additional={{
                left: <DataPlatformChip platform="dbt" bordered={false} />,
                right: rightAdornment ?? null,
              }}
            >
              {headingOverride ?? name}
            </PageHeading>
          </div>
        </span>
        {actions && (
          <div className="flex flex-shrink-0 items-center gap-2">{actions}</div>
        )}
      </div>
      {lastRunLabel && (
        <div className="font-caption mb-1 text-fgAlt">{lastRunLabel}</div>
      )}
      {headerIcons && headerIcons.length > 0 && (
        <div className="mb-2 flex min-h-[26px] items-center gap-4 duration-300 motion-reduce:duration-0">
          {headerIcons.map((item, i) => (
            <AssetHeaderIcon key={item.text ?? i} {...item} />
          ))}
        </div>
      )}
    </div>
  );
}

function AssetHeaderIcon({ icon, text, Contents, tooltip, href }: AssetHeaderIconItem) {
  const inner = (
    <span className="font-caption flex items-center gap-1 text-fgDecorative">
      {icon}
      <span className="align-middle">{Contents ? <Contents /> : text}</span>
    </span>
  );
  const wrapped = href ? (
    <Link isInternal={false} to={href}>
      {inner}
    </Link>
  ) : (
    inner
  );
  return tooltip ? <Tooltip content={tooltip}>{wrapped}</Tooltip> : wrapped;
}

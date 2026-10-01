import {
  BadgeAlert,
  BadgeCheck,
  BadgeMinus,
  BadgeX,
  CircleMinus,
  type LucideIcon,
  Repeat,
} from 'lucide-react';

import { Link } from '../../components/ui/Link';
import { Tooltip } from '../../components/ui/Tooltip';
import { CollapsibleSection } from './CollapsibleSection';
import { DetailsSection } from './SectionWithCard';
import { TimestampDisplay } from './Timestamp';

export enum ResourceStatusResult {
  unknown = 0,
  pass = 1,
  warn = 2,
  error = 3,
  stale = 4,
  skipped = 5,
  reused = 6,
}

export const runStatusResultIconMap: Record<ResourceStatusResult, LucideIcon> = {
  0: BadgeMinus,
  1: BadgeCheck,
  2: BadgeAlert,
  3: BadgeX,
  4: BadgeMinus,
  5: CircleMinus,
  6: Repeat,
};

export const freshnessStatusResultIconMap: Record<ResourceStatusResult, LucideIcon> = {
  0: BadgeMinus,
  1: BadgeCheck,
  2: BadgeAlert,
  3: BadgeX,
  4: BadgeMinus,
  5: CircleMinus,
  6: Repeat,
};

export type ResourceStatusSectionProps = {
  header: string;
  viewRunUrl?: string | null;
  shouldIndent: boolean;
  status: ResourceStatusResult;
  statusIcon: LucideIcon;
  tooltip: React.ReactNode;
};

export const ResourceStatusSection = ({
  header,
  viewRunUrl,
  shouldIndent,
  status,
  statusIcon: StatusIcon,
  tooltip,
}: ResourceStatusSectionProps) => (
  <CollapsibleSection
    disable={true}
    shouldIndent={shouldIndent}
    className="-mb-[1px] mt-1 flex w-full overflow-hidden border-b border-borderMuted p-4 text-fgMain"
    toExpand={undefined}
    closeAltText={''}
    expandAltText={''}
  >
    <span className="flex items-center" data-testid={`resource-status-${status}`}>
      <Tooltip content={tooltip} className="pointer-events-auto flex items-center">
        <StatusIcon className="pointer-events-auto size-4 align-middle" />
        <div className="sr-only">{tooltip}</div>
      </Tooltip>
    </span>
    <span>
      <p className="font-label-lg mx-2 align-middle">{header}</p>
    </span>
    <span className="flex-1 truncate text-right">
      {viewRunUrl && (
        <span>
          <Link isInternal={false} to={viewRunUrl} className="pointer-events-auto">
            View run
          </Link>
        </span>
      )}
    </span>
  </CollapsibleSection>
);

export type LatestStatusSectionDisplayProps = {
  header: string;
  checkCompletedAt?: string;
  checkCompletedAtUtc?: string;
  status: ResourceStatusResult;
  statusIcon: LucideIcon;
  tooltip: React.ReactNode;
  viewRunUrl?: string | null;
  shouldIndent?: boolean;
  children?: React.ReactNode;
};

export const LatestStatusSectionDisplay = ({
  header,
  checkCompletedAt,
  checkCompletedAtUtc,
  status,
  statusIcon,
  tooltip,
  viewRunUrl,
  shouldIndent = false,
  children,
}: LatestStatusSectionDisplayProps) => (
  <DetailsSection
    heading="Latest status"
    withCard={false}
    withHeading={
      <TimestampDisplay
        timestamp={checkCompletedAt}
        timestampUtc={checkCompletedAtUtc}
        prependedText="as of "
      />
    }
  >
    <div className="overflow-hidden rounded-md border border-borderMuted">
      <ResourceStatusSection
        header={header}
        viewRunUrl={viewRunUrl}
        shouldIndent={shouldIndent}
        status={status}
        statusIcon={statusIcon}
        tooltip={tooltip}
      />
      {children}
    </div>
  </DetailsSection>
);

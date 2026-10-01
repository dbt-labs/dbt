import { createElement, useMemo } from 'react';
import {
  BadgeAlert,
  BadgeCheck,
  BadgeMinus,
  BadgeX,
  CircleMinus,
  type LucideIcon,
} from 'lucide-react';

import { Tooltip } from '../../components/ui/Tooltip';
import { truthy } from '../util/array';
import { CollapsibleSection } from './CollapsibleSection';

export type SharedUpstreamSource = {
  uniqueId: string;
  name: string;
  freshnessStatus: string | null;
};

// exported for external use
export const upstreamFreshnessToIconMap: Record<string, LucideIcon> = {
  error: BadgeX,
  warn: BadgeAlert,
  unknown: BadgeMinus,
  pass: BadgeCheck,
  skipped: CircleMinus,
  outdated: BadgeX,
  unconfigured: BadgeMinus,
};

// used to sort results by severity (lower = worse)
export const freshnessStatusOrder: Record<string, number> = {
  error: 0,
  warn: 1,
  outdated: 2,
  unconfigured: 2,
  unknown: 3,
  pass: 4,
  skipped: 5,
};

const freshnessStatusMessages: Record<string, string> = {
  error: 'One or more upstream sources failed its freshness checks',
  warn: 'One or more upstream sources are warning',
  outdated: "Freshness hasn't run recently for one or more upstream sources",
  unconfigured: 'One or more upstream sources do not have freshness checks configured',
  unknown: 'Freshness status of one or more upstream sources is unknown',
  pass: 'All upstream sources are fresh',
  skipped: 'Freshness checks were skipped for one or more upstream sources',
};

interface UpstreamSourcesSectionProps {
  sources: SharedUpstreamSource[] | undefined;
  toExpand?: React.ReactNode;
  isOpen?: boolean;
}

export const UpstreamSourcesSection: React.FC<UpstreamSourcesSectionProps> = ({
  sources,
  toExpand,
  isOpen,
}) => {
  const { sortedSources, worstFreshnessStatus, tooltipMessage } = useMemo(() => {
    if (!sources) {
      return {
        sortedSources: undefined,
        worstFreshnessStatus: 'unknown',
        tooltipMessage:
          'The freshness status of upstream sources could not be determined',
      };
    }

    const sortedSources = sources.filter(truthy);

    let worstStatus = 'skipped';
    sortedSources.forEach((source) => {
      const status = source.freshnessStatus ?? 'unknown';
      const currentOrder = freshnessStatusOrder[status] ?? freshnessStatusOrder.unknown;
      const worstOrder =
        freshnessStatusOrder[worstStatus] ?? freshnessStatusOrder.unknown;
      if (currentOrder < worstOrder) {
        worstStatus = status;
      }
    });

    const message =
      sortedSources.length === 0
        ? 'This model has no upstream sources'
        : (freshnessStatusMessages[worstStatus] ?? freshnessStatusMessages.unknown);

    // sort by freshness status severity, then name alphabetically if tie
    sortedSources.sort((a, b) => {
      const aStatus = a.freshnessStatus ?? 'unknown';
      const bStatus = b.freshnessStatus ?? 'unknown';
      const aOrder = freshnessStatusOrder[aStatus] ?? freshnessStatusOrder.unknown;
      const bOrder = freshnessStatusOrder[bStatus] ?? freshnessStatusOrder.unknown;
      const statusComparison = aOrder - bOrder;
      if (statusComparison !== 0) {
        return statusComparison;
      }
      return a.name.localeCompare(b.name);
    });

    return {
      sortedSources,
      worstFreshnessStatus: worstStatus,
      tooltipMessage: message,
    };
  }, [sources]);

  return (
    <>
      {sortedSources && sortedSources.length > 0 && (
        <CollapsibleSection
          isOpen={isOpen}
          closeAltText={`Hide upstream source details`}
          expandAltText={`Show upstream source details`}
          toExpand={toExpand}
          disable={sortedSources.length === 0 || toExpand == null}
          shouldIndent={sortedSources.length > 0}
          className="-mb-[1px] mt-1 flex w-full overflow-hidden border-b border-borderMuted p-4 text-fgMain"
        >
          <span
            className="flex"
            data-testid={`freshness-status-${worstFreshnessStatus}`}
          >
            <span className="flex items-center">
              <Tooltip
                content={tooltipMessage}
                className="pointer-events-auto flex items-center"
              >
                {createElement(
                  upstreamFreshnessToIconMap[worstFreshnessStatus] ?? BadgeMinus,
                  { className: 'size-4 pointer-events-auto align-middle' },
                )}
                <div className="sr-only">{tooltipMessage}</div>
              </Tooltip>
            </span>
            <span>
              <p className="font-label-lg mx-2 align-middle">Upstream sources</p>
            </span>
          </span>
        </CollapsibleSection>
      )}
    </>
  );
};

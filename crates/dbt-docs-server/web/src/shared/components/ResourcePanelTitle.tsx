import { FC } from 'react';
import { twMerge } from 'tailwind-merge';

import { Tooltip } from '../../components/ui/Tooltip';
import { AccessLevelIcon, getAccessType } from '../../lib/accessLevel';
import type { ResourceType, ResourceTypeExplorer } from '../../lib/resourceType';
import {
  TrustSignals,
  trustSignalsSupportedResourceTypes,
} from '../typings/trustSignals';
import { TrustSignalsBadgeContainer } from './TrustSignalsBadge';

interface ResourcePanelTitleProps {
  name: string | undefined;
  packageName: string | null | undefined;
  resourceType: ResourceTypeExplorer;
  access?: string | null;
  trustSignals?: TrustSignals | null;
  className?: string;
}

export const ResourcePanelTitle: FC<ResourcePanelTitleProps> = ({
  name,
  packageName,
  resourceType,
  access,
  trustSignals,
  className,
}) => {
  const accessType = getAccessType(access ?? undefined);
  const showAccessIcon = !!accessType;
  const showTrustSignalsBadge =
    !!trustSignals &&
    (trustSignalsSupportedResourceTypes as readonly string[]).includes(resourceType);

  return (
    <div className={twMerge('w-full', className)}>
      <div className="w-full overflow-hidden text-xl font-medium">
        <Tooltip displayOnlyWhenTruncated content={name} placement="top-end">
          {(ref) => (
            <div ref={ref} className="flex items-center truncate">
              {showAccessIcon && (
                <span>
                  <AccessLevelIcon
                    access={accessType}
                    tooltipConfig={{ placement: 'bottom' }}
                    iconClassName="h-6 w-6"
                  />
                </span>
              )}
              <span className={showAccessIcon ? 'ml-1' : ''}>{name}</span>
              {showTrustSignalsBadge && (
                <span>
                  <TrustSignalsBadgeContainer
                    trustSignals={trustSignals}
                    resourceType={resourceType as ResourceType}
                    className="ml-2 mt-1"
                  />
                </span>
              )}
            </div>
          )}
        </Tooltip>
      </div>
      <div className="mb-4 mt-1 text-xs font-normal">{packageName}</div>
    </div>
  );
};

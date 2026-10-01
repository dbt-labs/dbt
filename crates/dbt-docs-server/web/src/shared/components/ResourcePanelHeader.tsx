import { FC, ReactNode } from 'react';

import type { ResourceTypeExplorer } from '../../lib/resourceType';
import { ResourceChip } from './ResourceChip';

interface ResourcePanelHeaderProps {
  resourceType: ResourceTypeExplorer;
  actions: ReactNode;
  /** Override the default ResourceChip with a custom chip element. */
  chip?: ReactNode;
}

export const ResourcePanelHeader: FC<ResourcePanelHeaderProps> = ({
  resourceType,
  actions,
  chip,
}) => {
  return (
    <div className="flex w-full items-center border-b border-borderMuted p-4">
      <span className="flex-1">
        {chip ?? <ResourceChip resourceType={resourceType} />}
      </span>
      <span className="flex-0 flex items-center gap-2">{actions}</span>
    </div>
  );
};

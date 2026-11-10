import { createElement, FC } from 'react';
import { twMerge } from 'tailwind-merge';

import {
  iconForType,
  resourceTypeColor,
  type ResourceTypeExplorer,
  WAREHOUSE_TYPES,
  type WarehouseType,
} from '../../lib/resourceType';
import { capitalizedResourceNames } from '../util/resourceType';
import { DataPlatformChip } from './DataPlatformChip';

interface ResourceChipProps extends React.ComponentPropsWithoutRef<'div'> {
  resourceType: ResourceTypeExplorer | WarehouseType;
  showText?: boolean;
}

export const ResourceChip: FC<ResourceChipProps> = ({
  resourceType,
  showText = true,
  className,
  ...props
}) => {
  const classes = 'flex w-fit space-x-2 rounded px-2 py-1 text-xs';

  if ((WAREHOUSE_TYPES as readonly string[]).includes(resourceType)) {
    return (
      <DataPlatformChip
        platform={resourceType as WarehouseType}
        className={twMerge(classes, 'text-bgMain', className)}
        bordered={false}
        {...props}
        style={{ backgroundColor: resourceTypeColor('column') }}
      />
    );
  }

  const capitalizedName =
    capitalizedResourceNames[resourceType as ResourceTypeExplorer];
  return (
    <div
      className={twMerge(classes, 'text-bgMain', className)}
      {...props}
      style={{ backgroundColor: resourceTypeColor(resourceType) }}
    >
      {createElement(iconForType(resourceType), {
        className: 'mt-0.5 align-middle',
        size: 16,
      })}
      {showText && (
        <span className="align-middle font-normal leading-5">{capitalizedName}</span>
      )}
    </div>
  );
};

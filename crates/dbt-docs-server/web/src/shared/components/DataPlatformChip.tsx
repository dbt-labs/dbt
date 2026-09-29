import { FC } from 'react';
import { twMerge } from 'tailwind-merge';

import type { WarehouseType } from '../../lib/resourceType';

interface DataPlatformChipProps extends React.ComponentPropsWithoutRef<'div'> {
  platform: 'dbt' | WarehouseType;
  bordered?: boolean;
}

/** Display name per warehouse -- mirrors dbt-dag's `warehouseNameMap` exactly
 *  (verified against its compiled output) rather than title-casing the raw
 *  value, which would render "Bigquery" instead of "BigQuery". */
const WAREHOUSE_NAME: Record<WarehouseType, string> = {
  snowflake: 'Snowflake',
  databricks: 'Databricks',
  bigquery: 'BigQuery',
  redshift: 'Redshift',
};

/** Renders the warehouse name as plain text -- no per-platform brand icon.
 *  dbt-dag's `WarehouseTypeIcon` rendered actual Snowflake/Databricks/BigQuery/
 *  Redshift logos; those aren't something to bundle into an OSS-published
 *  repo, so this is text-only by design, not a placeholder pending a real
 *  icon. */
export const DataPlatformChip: FC<DataPlatformChipProps> = ({
  platform,
  bordered = true,
  className,
  ...props
}) => {
  if (platform === 'dbt') {
    // dbt-native resources (no specific warehouse) have no chip to show —
    // there's no dbt brand icon here (white-labeled for OSS) and no other
    // platform to name.
    return null;
  }

  const borderClasses = bordered ? ' border p-2 border-borderMuted' : '';
  const classes = `flex w-fit space-x-2 rounded text-xs${borderClasses}`;

  return (
    <div className={twMerge(classes, className)} {...props}>
      <span className="align-middle font-normal leading-5 text-fgMain">
        {WAREHOUSE_NAME[platform]}
      </span>
    </div>
  );
};

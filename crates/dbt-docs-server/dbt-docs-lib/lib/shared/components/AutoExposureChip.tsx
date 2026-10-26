import { FC } from 'react';
import { twMerge } from 'tailwind-merge';

export type AutoExposureBiProvider = 'tableau' | 'powerbi';

interface AutoExposureChipProps extends React.ComponentPropsWithoutRef<'div'> {
  biProvider: AutoExposureBiProvider | null;
}

/** Renders the BI provider name as plain text -- no Tableau/Power BI brand
 *  mark. dbt-dag's `AutoExposureIcon` rendered the actual logos; those
 *  aren't something to bundle into an OSS-published repo, so this is
 *  text-only by design, same treatment as `DataPlatformChip`. */
export const AutoExposureChip: FC<AutoExposureChipProps> = ({
  biProvider,
  className,
  ...props
}) => {
  if (!biProvider) {
    return null;
  }
  const biProviderName = biProvider === 'tableau' ? 'Tableau' : 'Power BI';
  return (
    <div
      className={twMerge(
        'flex w-fit items-center space-x-2 rounded px-2 py-1 font-mono text-xs',
        'border border-borderMuted',
        className,
      )}
      {...props}
    >
      <span>{biProviderName}</span>
    </div>
  );
};

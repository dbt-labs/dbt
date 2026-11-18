import { FC } from 'react';

import { Badge } from './Badge';

type ColumnCardProps = {
  name: string;
  type?: string | null;
  description?: string | null;
};

export const ColumnCard: FC<ColumnCardProps> = ({ name, type, description }) => {
  return (
    <li
      data-testid={`${name}-card`}
      aria-label={name}
      className="divide-y divide-borderMuted overflow-hidden rounded-lg border border-borderMuted bg-bgMain text-sm"
    >
      <div className="px-4 py-2">
        <div className="flex items-center text-fgMain">
          <span className="font-label not-sr-only mx-1 flex-1">{name}</span>
          <span className="flex-0">
            <Badge aria-label="data type">{type?.toUpperCase()}</Badge>
          </span>
        </div>
        {description && <p className="mt-1 text-sm text-fgDecorative">{description}</p>}
      </div>
    </li>
  );
};

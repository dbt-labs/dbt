import { FC } from 'react';

import { Argument } from '../typings/domain/asset';
import { truthy } from '../util/array';
import { ColumnCard } from './ColumnCard';

type ArgumentsViewProps = {
  macroArguments: Argument[] | null;
};

export const ArgumentsView: FC<ArgumentsViewProps> = ({ macroArguments }) => {
  if (!macroArguments?.length) return <div className="mt-10">No arguments</div>;
  return (
    <ul aria-label="Arguments" className="mt-10 space-y-2">
      {macroArguments.filter(truthy).map((argument) => {
        return (
          <ColumnCard
            key={argument.name}
            name={argument.name}
            description={argument.description}
            type={argument.type}
          />
        );
      })}
    </ul>
  );
};

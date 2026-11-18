import { FC } from 'react';

import { Dimension } from '../typings/domain/asset';
import { SemanticAspectCard } from './SemanticAspectCard';

type DimensionsViewProps = {
  dimensions: Dimension[];
};

export const DimensionsView: FC<DimensionsViewProps> = ({ dimensions }) => {
  if (!dimensions.length) {
    return <div className="mt-10 text-fgDecorative">No dimensions found.</div>;
  }

  return (
    <ul aria-label="Dimensions" className="mt-10 space-y-2">
      {dimensions.map((dimension, index) => {
        return (
          <SemanticAspectCard
            name={dimension.name}
            description={dimension.description}
            type={dimension.type}
            key={dimension.name ?? `${index}`}
          />
        );
      })}
    </ul>
  );
};

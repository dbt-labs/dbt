import { FC } from 'react';

import { Measure } from '../typings/domain/asset';
import { SemanticAspectCard } from './SemanticAspectCard';

type MeasuresViewProps = {
  measures: Measure[];
};

export const MeasuresView: FC<MeasuresViewProps> = ({ measures }) => {
  if (!measures.length) {
    return <div className="mt-10 text-fgDecorative">No measures found.</div>;
  }

  return (
    <ul aria-label="Measures" className="mt-10 space-y-2">
      {measures.map((measure, index) => {
        return (
          <SemanticAspectCard
            name={measure.name}
            description={measure.description}
            type={measure.agg}
            key={measure.name ?? `${index}`}
          />
        );
      })}
    </ul>
  );
};

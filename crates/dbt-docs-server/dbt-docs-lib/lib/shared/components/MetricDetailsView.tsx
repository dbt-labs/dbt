import { FC } from 'react';

import { MetricInfo } from '../typings/domain/asset';
import { ColumnTable } from './ColumnTable';
import { DetailsSection } from './SectionWithCard';

type MetricDetailsViewProps = {
  metric: MetricInfo | null;
};

export const MetricDetailsView: FC<MetricDetailsViewProps> = ({ metric }) => {
  if (metric == null || !metric.type) {
    return <div className="mt-10 text-fgDecorative">No metric details.</div>;
  }

  const windowLabel =
    metric.window != null
      ? `${metric.window.count} ${metric.window.granularity}`
      : null;

  const tableEntries = [
    { key: 'Type', data: metric.type },
    metric.expression != null ? { key: 'Expression', data: metric.expression } : null,
    metric.grainToDate != null
      ? { key: 'Grain to date', data: metric.grainToDate }
      : null,
    windowLabel != null ? { key: 'Window', data: windowLabel } : null,
    metric.measure != null ? { key: 'Measure', data: metric.measure } : null,
    metric.numerator != null ? { key: 'Numerator', data: metric.numerator } : null,
    metric.denominator != null
      ? { key: 'Denominator', data: metric.denominator }
      : null,
  ].filter((e): e is NonNullable<typeof e> => e !== null);

  const hasFilters = metric.filters != null && metric.filters.length > 0;

  return (
    <div className="mt-10 space-y-4">
      <DetailsSection heading="Metric details">
        <ColumnTable isLoading={false} tableEntries={tableEntries} />
      </DetailsSection>
      {hasFilters && (
        <DetailsSection heading="Filters">
          <ul className="divide-y px-6 py-2">
            {metric.filters!.map((f, i) => (
              <li key={i} className="py-2">
                <pre className="overflow-x-auto text-xs">
                  <code>{f}</code>
                </pre>
              </li>
            ))}
          </ul>
        </DetailsSection>
      )}
    </div>
  );
};

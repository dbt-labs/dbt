import { FC } from 'react';

import { QueryExport } from '../typings/domain/asset';
import { ColumnTable } from './ColumnTable';
import { DetailsSection } from './SectionWithCard';

type QueryExportsViewProps = {
  exports: QueryExport[] | null;
};

export const QueryExportsView: FC<QueryExportsViewProps> = ({ exports }) => {
  if (exports == null) {
    return <div className="mt-10 text-fgDecorative">Loading...</div>;
  }

  if (!exports.length) {
    return <div className="mt-10 text-fgDecorative">No query exports found.</div>;
  }

  return (
    <ul aria-label="queryExports" className="mt-10 space-y-2">
      {exports.map((queryExport, index) => {
        const c = queryExport.config;
        return (
          <DetailsSection
            heading={queryExport.name || undefined}
            key={queryExport.name ?? `${index}`}
          >
            <ColumnTable
              key={queryExport.name ?? `${index}`}
              isLoading={false}
              tableEntries={[
                { key: 'Alias', data: c?.alias as React.ReactNode },
                { key: 'Export As', data: c?.exportAs as React.ReactNode },
                { key: 'Schema', data: c?.schema as React.ReactNode },
                { key: 'Database', data: c?.database as React.ReactNode },
              ]}
            />
          </DetailsSection>
        );
      })}
    </ul>
  );
};

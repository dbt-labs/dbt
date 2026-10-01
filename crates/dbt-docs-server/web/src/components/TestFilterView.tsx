import { useMemo, useState } from 'react';
import { type ColumnDef } from '@tanstack/react-table';

import { facetOptions, selectedFacetOption } from '../lib/facetOptions';
import { makeNameCell } from '../lib/nameCell';
import type { TestSummary } from '../shared';
import type { Project } from '../shared';
import {
  asCellRenderer,
  FilterDropdown,
  NodeStatusIconBadge,
  type TestStatusValue,
  TruncatedCell,
  useFacets,
} from '../shared';
import { GenericFilterView } from './GenericFilterView';

interface Props {
  project: Project;
  onPeek(uniqueId: string): void;
}

const capitalize = (s: string) => (s ? s[0].toUpperCase() + s.slice(1) : s);

const TEST_STATUSES = new Set<TestStatusValue>([
  'pass',
  'fail',
  'warn',
  'error',
  'skipped',
]);

function toTestStatus(status: string | null | undefined): TestStatusValue {
  if (status === 'success') return 'pass';
  if (status && TEST_STATUSES.has(status as TestStatusValue))
    return status as TestStatusValue;
  return 'unknown';
}

export function TestFilterView({ project, onPeek }: Props) {
  const [result, setResult] = useState('');
  const [testType, setTestType] = useState('');
  const { data: facets } = useFacets('test');

  const resultOptions = facetOptions(facets?.results, capitalize);
  const testTypeOptions = facetOptions(facets?.testTypes, capitalize);

  const columns = useMemo<ColumnDef<TestSummary>[]>(
    () => [
      makeNameCell(onPeek, (r) => r.uniqueId),
      {
        id: 'test_type',
        header: 'Test type',
        size: 120,
        accessorFn: (row) => row.testType ?? 'data',
        cell: asCellRenderer<TestSummary>(TruncatedCell),
      },
      {
        id: 'test_result',
        header: 'Test result',
        size: 140,
        cell: (info) => (
          <NodeStatusIconBadge
            kind="test"
            status={toTestStatus(info.row.original.status)}
          />
        ),
      },
      {
        id: 'tested_resource',
        header: 'Tested resource',
        size: 200,
        accessorFn: (row) => row.testedNodeUniqueId?.split('.').pop() ?? '',
        cell: asCellRenderer<TestSummary>(TruncatedCell),
      },
      {
        id: 'tested_column',
        header: 'Column',
        size: 160,
        accessorFn: (row) => row.testedColumn ?? '',
        cell: asCellRenderer<TestSummary>(TruncatedCell),
      },
    ],
    [onPeek],
  );

  return (
    <GenericFilterView
      label="Tests"
      project={project}
      resourceType="test"
      columns={columns}
      emptyMessage="No tests found."
      filter={{
        results: result ? [result] : undefined,
        testTypes: testType ? [testType] : undefined,
      }}
      filterControls={
        <>
          <FilterDropdown
            name="Test result"
            options={resultOptions}
            defaultOption={selectedFacetOption(resultOptions, result)}
            onChange={(opt) => setResult(String(opt.value))}
          />
          <FilterDropdown
            name="Test type"
            options={testTypeOptions}
            defaultOption={selectedFacetOption(testTypeOptions, testType)}
            onChange={(opt) => setTestType(String(opt.value))}
          />
        </>
      }
    />
  );
}

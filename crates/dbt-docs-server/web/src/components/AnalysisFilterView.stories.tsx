import type { Meta, StoryObj } from '@storybook/react-vite';

import { makeFakeProject } from '../shared';
import type { BootstrapData } from '../shared/data-sources/duckdb/bootstrap';
import { storyBootstrapData, storyNodes } from '../shared/testing/storyFixtures';
import { never, rejected } from '../shared/testing/storySources';
import type { NodeSummary } from '../types';
import { AnalysisFilterView } from './AnalysisFilterView';

/** Analyses are not in the default node fixture — nothing else needs them — so these
 *  stories add their own. */
function analysis(name: string, packageName: string): NodeSummary {
  return {
    unique_id: `analysis.${packageName}.${name}`,
    name,
    resource_type: 'analysis',
    package_name: packageName,
    description: `Ad-hoc analysis: ${name}.`,
    original_file_path: `analyses/${name}.sql`,
  };
}

const WITH_ANALYSES: BootstrapData = storyBootstrapData({
  nodes: [
    ...storyNodes(),
    analysis('customer_cohorts', 'jaffle_shop'),
    analysis('revenue_backfill_check', 'jaffle_shop'),
    analysis('audit_row_counts', 'audit_helper'),
  ],
});

const meta: Meta<typeof AnalysisFilterView> = {
  component: AnalysisFilterView,
  args: { project: makeFakeProject(), onPeek: () => {} },
  parameters: { docsApp: { bootstrap: WITH_ANALYSES } },
};

export default meta;
type Story = StoryObj<typeof AnalysisFilterView>;

/**
 * The one list view that does not go through `useAssetList`: analyses have no list
 * projection, so this filters the in-memory node index instead. The package dropdown
 * is likewise derived from the rows rather than from a facet fetch — which is why it
 * shows bare names with no counts.
 */
export const Default: Story = {};

/** A single package collapses the dropdown to "All" plus one entry. */
export const SinglePackage: Story = {
  parameters: {
    docsApp: {
      bootstrap: storyBootstrapData({
        nodes: [...storyNodes(), analysis('customer_cohorts', 'jaffle_shop')],
      }),
    },
  },
};

/** A project with no analyses at all — the common case, since analyses are rare. */
export const NoAnalyses: Story = {
  parameters: { docsApp: { bootstrap: storyBootstrapData() } },
};

/** The first-paint read has not resolved yet. */
export const Loading: Story = {
  parameters: { docsApp: { bootstrap: never<BootstrapData>() } },
};

/**
 * A failed read renders an error, not an empty list — an empty list here would claim
 * the project has no analyses, which is a very different statement from "the read
 * failed".
 */
export const LoadError: Story = {
  parameters: {
    docsApp: { bootstrap: rejected<BootstrapData>('failed to read dbt.nodes') },
  },
};

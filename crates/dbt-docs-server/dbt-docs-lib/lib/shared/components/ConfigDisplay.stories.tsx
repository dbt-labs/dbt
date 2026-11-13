import type { Meta, StoryObj } from '@storybook/react-vite';

import { ConfigDisplay } from './ConfigDisplay';

const meta: Meta<typeof ConfigDisplay> = {
  component: ConfigDisplay,
  args: {
    config: {
      materialized: 'incremental',
      unique_key: 'customer_id',
      incremental_strategy: 'merge',
      on_schema_change: 'append_new_columns',
      tags: ['daily', 'marts'],
      enabled: true,
    },
  },
  decorators: [(Story) => <div className="max-w-md">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof ConfigDisplay>;

/**
 * Note the trailing "Static analysis — Enable dbt v2 to view" row: it appears whenever
 * the key is *absent*, deliberately, so a Core user learns the capability exists rather
 * than seeing nothing.
 */
export const Default: Story = {};

/** With `static_analysis` present the teaser row disappears and the real value shows. */
export const WithStaticAnalysis: Story = {
  args: {
    config: {
      materialized: 'table',
      static_analysis: 'strict',
    },
  },
};

/** Nested objects recurse, indented and rule-marked. `docs` and `meta` are the usual
 *  ones. */
export const NestedObjects: Story = {
  args: {
    config: {
      materialized: 'view',
      docs: { show: true, node_color: '#ff6849' },
      meta: {
        owner: 'finance-analytics',
        sla: { hours: 4, escalation: 'pagerduty' },
      },
    },
  },
};

/** Any key ending in "color" renders a swatch instead of the hex string — the value
 *  is still the accessible name. */
export const ColorSwatches: Story = {
  args: {
    config: {
      node_color: '#ff6849',
      docs: { node_color: 'rebeccapurple' },
    },
  },
};

/** Arrays are space-joined and booleans stringified — the two coercions worth seeing
 *  side by side. */
export const ValueFormatting: Story = {
  args: {
    config: {
      tags: ['daily', 'marts', 'pii'],
      enabled: true,
      full_refresh: false,
      post_hook: ['grant select on {{ this }} to role reporter'],
      persist_docs: { relation: true, columns: true },
    },
  },
};

/** An empty config still shows the static-analysis teaser, which is the only row that
 *  does not come from the data. */
export const EmptyConfig: Story = {
  args: { config: {} },
};

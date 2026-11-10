import type { Meta, StoryObj } from '@storybook/react-vite';

import { storyColumnLineage } from '../shared/testing/storyFixtures';
import { ColumnLineageMini } from './ColumnLineageView';

const meta: Meta<typeof ColumnLineageMini> = {
  component: ColumnLineageMini,
  args: {
    rootUniqueId: 'model.jaffle_shop.customers',
    columnName: 'customer_id',
    onSelect: () => {},
    userState: null,
    load: () => {},
    state: { kind: 'ready', result: { kind: 'ok', graph: storyColumnLineage() } },
  },
  decorators: [(Story) => <div className="w-[760px]">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof ColumnLineageMini>;

/**
 * The per-column expanded body inside the Columns tab. The fetch is node-scoped and
 * cached, so expanding a second column on the same model reuses this response — which
 * is why `state` and `load` are owned by the parent rather than by this component.
 *
 * The graph is BFS'd both directions from the target column, so only the subgraph that
 * actually touches `customer_id` is drawn.
 */
export const Ready: Story = {};

/** `idle` fires `load()` on mount, so it and `loading` render the same spinner —
 *  the reader should not see a flash of "nothing here" before the fetch starts. */
export const Idle: Story = {
  args: { state: { kind: 'idle' } },
};

export const Loading: Story = {
  args: { state: { kind: 'loading' } },
};

/** Errors are retryable in place, since a failed CLL read is often transient. */
export const LoadError: Story = {
  args: {
    state: { kind: 'error', message: 'column lineage read failed' },
  },
};

/**
 * A column the graph knows nothing about. Distinct from an error and from gating: the
 * fetch succeeded, the column simply has no edges — which is exactly what a non-strict
 * export produces for a computed column.
 */
export const NoEdgesTouchThisColumn: Story = {
  args: { columnName: 'lifetime_value' },
};

/**
 * `gated` means the data source reports column lineage as unavailable. With a
 * `userState` the upsell card renders in place of the graph — the whole reason
 * `fetchColumnLineage` returns a discriminated result rather than an empty graph.
 */
export const GatedWithUpsell: Story = {
  args: {
    state: { kind: 'ready', result: { kind: 'gated' } },
    userState: 'core',
  },
};

export const GatedProprietaryAnon: Story = {
  args: {
    state: { kind: 'ready', result: { kind: 'gated' } },
    userState: 'proprietary-anon',
  },
};

/** Gated but with no resolved user state yet — capabilities are still loading, so
 *  nothing renders rather than an upsell aimed at the wrong user state. */
export const GatedWithoutUserState: Story = {
  args: {
    state: { kind: 'ready', result: { kind: 'gated' } },
    userState: null,
  },
};

/** A longer chain, to see the horizontal layout and the transformation labels. */
export const DeeperChain: Story = {
  args: {
    columnName: 'customer_id',
    state: {
      kind: 'ready',
      result: {
        kind: 'ok',
        graph: {
          nodes: [],
          edges: [
            {
              fromNodeUniqueId: 'source.jaffle_shop.raw.customers',
              fromColumn: 'id',
              toNodeUniqueId: 'model.jaffle_shop.stg_customers',
              toColumn: 'customer_id',
              transformationType: 'rename',
            },
            {
              fromNodeUniqueId: 'model.jaffle_shop.stg_customers',
              fromColumn: 'customer_id',
              toNodeUniqueId: 'model.jaffle_shop.customers',
              toColumn: 'customer_id',
              transformationType: 'passthrough',
            },
            {
              fromNodeUniqueId: 'model.jaffle_shop.customers',
              fromColumn: 'customer_id',
              toNodeUniqueId: 'model.jaffle_shop.fct_orders',
              toColumn: 'customer_id',
              transformationType: 'passthrough',
            },
          ],
        },
      },
    },
  },
};

/** An unrecognised transformation type degrades to `UNKNOWN` rather than being passed
 *  through to the DAG, which only accepts its own vocabulary. */
export const UnknownTransformationType: Story = {
  args: {
    state: {
      kind: 'ready',
      result: {
        kind: 'ok',
        graph: {
          nodes: [],
          edges: [
            {
              fromNodeUniqueId: 'model.jaffle_shop.stg_customers',
              fromColumn: 'customer_id',
              toNodeUniqueId: 'model.jaffle_shop.customers',
              toColumn: 'customer_id',
              transformationType: 'something_new',
            },
          ],
        },
      },
    },
  },
};

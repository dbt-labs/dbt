import type { Meta, StoryObj } from '@storybook/react-vite';
import { ReactFlow } from '@xyflow/react';
import { expect, userEvent, waitFor, within } from 'storybook/test';

import {
  DAG_NODE_HEIGHT,
  DAG_NODE_TYPE,
  DAG_NODE_TYPES,
  DAG_NODE_WIDTH,
  type DagNodeData,
  type DagNodeType,
} from './DagNode';

/**
 * `DagNode` is a registered React Flow node type, not a standalone component: its
 * `Handle`s need the canvas's store, and React Flow is what positions it. So every
 * story mounts a real (tiny) canvas — which is also the only way to see that edges
 * actually attach to the handles.
 */
type PreviewProps = {
  nodes: { data: DagNodeData; selected?: boolean }[];
  /** Draw an edge between consecutive nodes, to check handle placement. */
  connected?: boolean;
  /**
   * Has to be passed explicitly, and has to agree with the story's theme.
   *
   * React Flow stamps `light` or `dark` on the canvas root, and biga's tokens are
   * scoped by those same class names — so the canvas re-themes everything inside it,
   * and a canvas in the wrong mode renders a light node on a dark page. In the app
   * `BaseDag` feeds this from `useTheme`; here it is an arg, which means flipping
   * Storybook's global theme toolbar does *not* follow. Use the `LightMode` story to
   * review the light variant.
   */
  colorMode?: 'light' | 'dark';
};

/** A real component rather than an inline `render` closure, because the node list has
 *  to be built from args before React Flow sees it. */
function DagNodePreview({ nodes, connected, colorMode = 'dark' }: PreviewProps) {
  // Spaced by the layout constants, the same way dagre spaces the real graph, and
  // wrapped so a story with several nodes stays legible instead of running off-canvas.
  const perRow = connected ? nodes.length : 3;
  const flowNodes: DagNodeType[] = nodes.map((node, i) => ({
    id: `n${i}`,
    type: DAG_NODE_TYPE,
    position: {
      x: (i % perRow) * (DAG_NODE_WIDTH + 60),
      y: Math.floor(i / perRow) * (DAG_NODE_HEIGHT + 48),
    },
    data: node.data,
    selected: node.selected,
  }));

  const edges = connected
    ? flowNodes.slice(1).map((node, i) => ({
        id: `e${i}`,
        source: `n${i}`,
        target: node.id,
        type: 'smoothstep',
      }))
    : [];

  return (
    <div style={{ width: '100%', height: 320 }}>
      <ReactFlow
        nodes={flowNodes}
        edges={edges}
        nodeTypes={DAG_NODE_TYPES}
        colorMode={colorMode}
        fitView
        fitViewOptions={{ padding: 0.25 }}
        nodesDraggable={false}
        nodesConnectable={false}
        proOptions={{ hideAttribution: true }}
      />
    </div>
  );
}

const meta: Meta<typeof DagNodePreview> = {
  component: DagNodePreview,
  args: {
    nodes: [
      { data: { name: 'dim_customers', resourceType: 'model', columnCount: 25 } },
    ],
  },
};

export default meta;
type Story = StoryObj<typeof DagNodePreview>;

/** The design's resting state: name header, resource-type badge, column-count chip. */
export const Default: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText('dim_customers')).toBeVisible());
    await expect(canvas.getByText('Model')).toBeVisible();
    await expect(canvas.getByText('25')).toBeVisible();
  },
};

/** The same node in light mode. Both the story theme and the canvas's `colorMode` are
 *  switched, since they are two independent knobs — see `colorMode` above. */
export const LightMode: Story = {
  args: { colorMode: 'light' },
  parameters: { themes: { themeOverride: 'light' } },
};

/** Selected. Only the border changes — the four-layer shadow is the node's resting
 *  elevation, not a selection cue, so it stays put. */
export const Active: Story = {
  args: {
    nodes: [
      {
        data: { name: 'dim_customers', resourceType: 'model', columnCount: 25 },
        selected: true,
      },
    ],
  },
};

/**
 * No column-level lineage. `columnCount` absent hides the chip entirely, which is the
 * common case for this site — the exporter only writes column lineage after a compile
 * with `--static-analysis strict`.
 */
export const WithoutColumnLineage: Story = {
  args: {
    nodes: [{ data: { name: 'dim_customers', resourceType: 'model' } }],
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText('dim_customers')).toBeVisible());
    await expect(canvas.queryByTitle(/columns with lineage/)).toBeNull();
  },
};

/** Zero is a count, not an absence: the chip renders. Only `null`/`undefined` hides
 *  it, which is what separates "analysed, no columns" from "not analysed". */
export const ZeroColumns: Story = {
  args: {
    nodes: [{ data: { name: 'dim_customers', resourceType: 'model', columnCount: 0 } }],
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText('0')).toBeVisible());
  },
};

/** A long name ellipses rather than widening the node — the width is fixed because
 *  dagre lays the graph out from the same number. Hover reveals the full name, and
 *  only because it is truncated. */
export const LongName: Story = {
  args: {
    nodes: [
      {
        data: {
          name: 'int_order_items_joined_to_customers_and_products',
          resourceType: 'model',
          columnCount: 132,
        },
      },
    ],
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);

    await waitFor(() =>
      expect(canvas.getByText(/int_order_items_joined/)).toBeVisible(),
    );
    await userEvent.hover(canvas.getByText(/int_order_items_joined/));
    // Portalled to <body>, behind the tooltip's 200ms open delay.
    await within(document.body).findByText(
      'int_order_items_joined_to_customers_and_products',
      undefined,
      { timeout: 3000 },
    );
  },
};

/** Two nodes and an edge: the handles are invisible but present, so the edge meets the
 *  node border on the correct sides for a left-to-right layout. */
export const Connected: Story = {
  args: {
    connected: true,
    nodes: [
      { data: { name: 'stg_customers', resourceType: 'model', columnCount: 9 } },
      { data: { name: 'dim_customers', resourceType: 'model', columnCount: 25 } },
    ],
  },
};

/** The badge colour is per resource type, from the `--fgViz*` tokens. */
export const ResourceTypes: Story = {
  args: {
    nodes: [
      { data: { name: 'dim_customers', resourceType: 'model', columnCount: 25 } },
      { data: { name: 'raw_customers', resourceType: 'seed', columnCount: 4 } },
      { data: { name: 'jaffle_shop', resourceType: 'source', columnCount: 12 } },
      { data: { name: 'scd_orders', resourceType: 'snapshot', columnCount: 18 } },
      { data: { name: 'revenue', resourceType: 'metric' } },
      { data: { name: 'weekly_report', resourceType: 'exposure' } },
      { data: { name: 'cents_to_dollars', resourceType: 'macro' } },
      { data: { name: 'orders_semantics', resourceType: 'semantic_model' } },
    ],
  },
};

/** An unrecognised type still renders: neutral fill, and the raw string as the label
 *  rather than a blank badge. */
export const UnknownResourceType: Story = {
  args: {
    nodes: [{ data: { name: 'mystery', resourceType: 'sql_operation' } }],
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await waitFor(() => expect(canvas.getByText('sql_operation')).toBeVisible());
  },
};

/** Fixed 245×108 regardless of content, so the graph dagre lays out matches what
 *  renders. This asserts the contract the layout depends on. */
export const FixedDimensions: Story = {
  play: async ({ canvasElement }) => {
    const node = await waitFor(() => {
      const el = canvasElement.querySelector<HTMLElement>('.dag-node');
      expect(el).not.toBeNull();
      expect(el).toBeVisible();
      return el as HTMLElement;
    });

    // fitView scales the canvas, so compare the unscaled offset box rather than the
    // rendered rect.
    await expect(node.offsetWidth).toBe(DAG_NODE_WIDTH);
    await expect(node.offsetHeight).toBe(DAG_NODE_HEIGHT);
  },
};

import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, userEvent, waitFor, within } from 'storybook/test';

import { storyModel } from '../testing/storyFixtures';
import { CodePreview } from './CodePreview';

const model = storyModel();

const meta: Meta<typeof CodePreview> = {
  component: CodePreview,
  args: {
    source: model.rawCode ?? '',
    compiled: model.compiledCode ?? undefined,
  },
  decorators: [(Story) => <div className="max-w-3xl">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof CodePreview>;

/** Both tabs. "Source" is the authored Jinja, "Compiled" the rendered SQL. */
export const Default: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);

    // Asserted against `textContent` rather than with `getByText`: the syntax
    // highlighter wraps every token in its own element, so the code is never one text
    // node and a text query cannot match across it.
    // Opens on Source — the authored Jinja, `ref()` intact.
    await expect(canvasElement.textContent).toContain("ref('stg_customers')");

    await userEvent.click(canvas.getByText('Compiled'));

    // Compiled resolves the ref to a real relation, which is the whole point of
    // having the second tab.
    await waitFor(() =>
      expect(canvasElement.textContent).toContain('"analytics"."dbt"."stg_customers"'),
    );
  },
};

/** Without compiled code the second tab is not rendered at all, rather than
 *  rendered-and-disabled. */
export const SourceOnly: Story = {
  args: { compiled: undefined },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // Not rendered at all rather than rendered-and-disabled, so there is nothing to
    // click and nothing to explain.
    await expect(canvas.queryByText('Compiled')).toBeNull();
    await expect(canvas.getByText('Source')).toBeVisible();
  },
};

/**
 * Past 256 KB syntax highlighting is switched off and a warning explains why —
 * highlighting a file that size has crashed browsers. Generated models routinely
 * clear the threshold, so this is a real state, not a defensive one.
 */
export const LargeFileWarning: Story = {
  args: {
    // Just over the 256 KB threshold, reached with a few very long lines rather than
    // thousands of short ones. The component only measures `length`, so the byte count
    // is what matters — and a fixture with 12,000 lines in it takes long enough to lay
    // out in a real browser to time the story test out.
    source: `select\n${Array.from(
      { length: 40 },
      (_, i) => `    ${'very_long_column_alias_'.repeat(300)}${i},`,
    ).join('\n')}\nfrom {{ ref('wide_source') }}`,
    compiled: undefined,
  },
};

export const Python: Story = {
  args: {
    source:
      'def model(dbt, session):\n' +
      '    dbt.config(materialized="table")\n' +
      '    return dbt.ref("stg_orders")',
    compiled: undefined,
  },
};

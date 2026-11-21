import { type ComponentProps, useState } from 'react';
import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, userEvent, within } from 'storybook/test';

import { DetailTabs, type TabInfo, type TabType } from './DetailTabs';

const body = (tab: TabType) => (
  <div className="mt-4 rounded-lg border border-borderMuted p-6 text-fgMain">
    {tab} panel
  </div>
);

const MODEL_TABS: TabInfo[] = [
  { type: 'general' },
  { type: 'columns', count: 4 },
  { type: 'code' },
  { type: 'relationships' },
  { type: 'config' },
];

const meta: Meta<typeof DetailTabs> = {
  component: DetailTabs,
  args: { tabs: MODEL_TABS, show: true, children: body },
  decorators: [(Story) => <div className="max-w-3xl">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof DetailTabs>;

/** Uncontrolled: the component keeps its own active tab and defaults to the first.
 *  Note "Relationships" carries an `alpha` badge, which only renders when the tab has
 *  no count. */
export const Default: Story = {
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);

    // Defaults to the first tab rather than to a hardcoded 'general'.
    await expect(canvas.getByText('general panel')).toBeVisible();

    await userEvent.click(canvas.getByText('Code'));
    await expect(canvas.getByText('code panel')).toBeVisible();
    await expect(canvas.queryByText('general panel')).toBeNull();

    await userEvent.click(canvas.getByText('Columns'));
    await expect(canvas.getByText('columns panel')).toBeVisible();
  },
};

/** Counts render in the tab, which is why `columns` shows `4`. */
export const WithCounts: Story = {
  args: {
    tabs: [
      { type: 'general' },
      { type: 'columns', count: 24 },
      { type: 'dimensions', count: 2 },
      { type: 'measures', count: 7 },
    ],
  },
};

/** A single tab hides the tab bar entirely and just renders the body — how a resource
 *  type with nothing to switch between looks. */
export const SingleTabHidesBar: Story = {
  args: { tabs: [{ type: 'general' }] },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    // Body still renders; the tab control does not.
    await expect(canvas.getByText('general panel')).toBeVisible();
    await expect(canvas.queryByTestId('resource-view-tabs')).toBeNull();
  },
};

/** `show={false}` fades the whole block out. The detail page uses it to avoid a flash
 *  of tabs before the asset resolves. */
export const Hidden: Story = {
  args: { show: false },
};

/** A real component rather than an inline `render` closure, because hooks are not
 *  allowed in the latter. */
function ControlledTabs(props: ComponentProps<typeof DetailTabs>) {
  const [tab, setTab] = useState<TabType>('code');
  return (
    <>
      <p className="mb-2 text-sm text-fgDecorative">
        Caller-owned active tab: <code>{tab}</code>
      </p>
      <DetailTabs {...props} activeTab={tab} onTabChange={setTab} />
    </>
  );
}

/** Controlled: passing `activeTab` hands ownership to the caller, which is how the
 *  real page keeps the selected tab in the URL. */
export const Controlled: Story = {
  render: (args) => <ControlledTabs {...args} />,
};

/** Every tab type at once — the check that each has a label in `tabNameMap` and that
 *  the bar scrolls rather than wrapping. */
export const AllTabs: Story = {
  args: {
    tabs: [
      { type: 'general' },
      { type: 'code' },
      { type: 'columns', count: 4 },
      { type: 'performance' },
      { type: 'evaluator' },
      { type: 'arguments', count: 2 },
      { type: 'dimensions', count: 2 },
      { type: 'measures', count: 2 },
      { type: 'queryExports', count: 1 },
      { type: 'tables' },
      { type: 'views' },
      { type: 'relationships' },
      { type: 'config' },
    ],
  },
};

import { useState } from 'react';
import type { Meta, StoryObj } from '@storybook/react-vite';

import { Resizable, type ResizableProps } from './Resizable';

const panel = (
  <div className="h-64 bg-bgNeutralMuted p-4 text-sm text-fgMain">
    Drag the handle on the edge of this panel.
  </div>
);

const meta: Meta<typeof Resizable> = {
  component: Resizable,
  args: { children: panel },
  decorators: [(Story) => <div className="flex">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof Resizable>;

/** Handle on the right edge — dragging right widens. */
export const Default: Story = {};

/** Handle on the left, so the drag maths inverts. This is the form used by a panel
 *  docked to the right of the page. */
export const HandleOnLeft: Story = {
  args: { direction: 'left' },
  decorators: [(Story) => <div className="flex justify-end">{Story()}</div>],
};

export const CustomDefaultWidth: Story = {
  args: { defaultWidth: 520 },
};

/** Tight bounds, so the clamping is easy to feel: it will not go below 240 or above
 *  360 however far you drag. */
export const ConstrainedWidth: Story = {
  args: { defaultWidth: 300, minWidth: 240, maxWidth: 360 },
};

/**
 * With no `maxWidth` the ceiling becomes the viewport less 100px, recomputed on window
 * resize — so the panel can never be dragged (or left) wider than the window.
 */
export const UnboundedMaxWidth: Story = {
  args: { defaultWidth: 400, maxWidth: undefined },
};

/** A real component rather than an inline `render` closure, because hooks are not
 *  allowed in the latter. */
function WidthReporter(props: ResizableProps) {
  const [width, setWidth] = useState<number | null>(null);
  return (
    <div>
      <p className="mb-2 text-sm text-fgDecorative">
        Reported width: {width == null ? '—' : `${Math.round(width)}px`}
      </p>
      <div className="flex">
        <Resizable {...props} onWidthChange={setWidth} />
      </div>
    </div>
  );
}

/** `onWidthChange` fires on every drag frame and on viewport resize; this echoes it so
 *  the callback is observable. */
export const ReportsWidth: Story = {
  render: (args) => <WidthReporter {...args} />,
};

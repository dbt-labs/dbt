import type { Meta, StoryObj } from '@storybook/react-vite';

import { HighlightPills } from './HighlightPills';

const meta: Meta<typeof HighlightPills> = {
  component: HighlightPills,
  args: {
    query: 'customer',
    highlights: {
      column: ['<b>customer</b>_id', '<b>customer</b>_name'],
      tag: ['<b>customer</b>_facing'],
    },
  },
  // Room for the tooltips, which carry the matched snippets.
  decorators: [(Story) => <div className="p-4 pb-48">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof HighlightPills>;

/** One pill per matched field, each counting its snippets. Hover for the snippets. */
export const Default: Story = {};

/** `name` matches are deliberately excluded — the row already shows the name bolded,
 *  so a "Name (1)" pill would be noise. This renders only the Tag pill. */
export const NameMatchesAreExcluded: Story = {
  args: {
    highlights: {
      name: ['<b>customer</b>s'],
      tag: ['<b>customer</b>_facing'],
    },
  },
};

/** Counts cap at "10+" rather than growing without bound. */
export const CountCapsAtTenPlus: Story = {
  args: {
    highlights: {
      column: Array.from({ length: 24 }, (_, i) => `<b>customer</b>_field_${i}`),
    },
  },
};

/** With `getColumnHref`, column snippets become links into the resource's column
 *  list. Without it they are plain text. */
export const ColumnLinks: Story = {
  args: {
    getColumnHref: (columnName) => `#/model/customers?column=${columnName}`,
  },
};

/** Description snippets are truncated at 200 characters inside the tooltip — the only
 *  field that gets that treatment, since it is the only unbounded one. */
export const DescriptionIsTruncated: Story = {
  args: {
    highlights: {
      description: [
        `One row per <b>customer</b>. ${'This description goes on at length. '.repeat(
          12,
        )}`,
      ],
    },
  },
};

export const AllFields: Story = {
  args: {
    highlights: {
      column: ['<b>customer</b>_id'],
      tag: ['<b>customer</b>_facing'],
      fqn: ['jaffle_shop.marts.<b>customer</b>s'],
      description: ['One row per <b>customer</b>.'],
    },
  },
};

/** Every field empty renders nothing — no stray "Matches:" label. */
export const NoHighlightsRendersNothing: Story = {
  args: { highlights: { column: [] } },
};

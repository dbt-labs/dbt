import type { Meta, StoryObj } from '@storybook/react-vite';

import { Badge } from '../../../components/ui/Badge';
import { HealthIssueType } from '../../typings/discoveryEnums';
import { RichSearchResultItem } from './RichSearchResultItem';
import type { SearchResultDisplayData } from './types';

const DATA: SearchResultDisplayData = {
  matchedField: 'column',
  highlight: '<b>customer</b>_id',
  hit: {
    name: 'customers',
    uniqueId: 'model.jaffle_shop.customers',
    resourceType: 'model',
  },
};

const meta: Meta<typeof RichSearchResultItem> = {
  component: RichSearchResultItem,
  args: {
    query: 'customer',
    data: DATA,
    getResourceHref: (uniqueId) => `#/model/${uniqueId}`,
    metadata: {
      projectName: 'jaffle_shop',
      environmentType: 'Production',
      numColumns: 4,
      lastRunLabel: 'Last run: 2h ago',
    },
    highlights: { column: ['<b>customer</b>_id', '<b>customer</b>_name'] },
  },
  decorators: [(Story) => <div className="w-[760px] pb-48">{Story()}</div>],
};

export default meta;
type Story = StoryObj<typeof RichSearchResultItem>;

/** The card variant: name row, a metadata row, then the match pills. */
export const Default: Story = {};

/** Without `metadata` the second row is suppressed entirely — which is what docs-v2
 *  does, having no environment or project to report. */
export const WithoutMetadata: Story = {
  args: { metadata: undefined },
};

/** Without highlights the pill row is suppressed too, leaving just the name row. */
export const WithoutHighlights: Story = {
  args: { highlights: undefined, metadata: undefined },
};

/** `dataPlatform` swaps the leading chip from dbt to a warehouse mark. */
export const WithWarehousePlatform: Story = {
  args: {
    metadata: {
      projectName: 'jaffle_shop',
      numColumns: 12,
      dataPlatform: 'snowflake',
    },
  },
};

/** `extras` appends arbitrary inline pills after the resource-type icon — the slot
 *  used for materialization. */
export const WithExtras: Story = {
  args: {
    metadata: {
      projectName: 'jaffle_shop',
      numColumns: 4,
      extras: <Badge text="incremental" variant="default" size="xs" />,
    },
  },
};

export const WithTrustSignals: Story = {
  args: {
    trustSignals: { healthIssues: [HealthIssueType.FailedTest] },
  },
};

export const WithColumnLinks: Story = {
  args: {
    getColumnHref: (uniqueId, column) => `#/model/${uniqueId}?column=${column}`,
  },
};

export const Skeleton: Story = {
  render: () => <RichSearchResultItem skeleton />,
};

import type { Meta, StoryObj } from '@storybook/react-vite';

import { storyModel, storySource } from '../shared/testing/storyFixtures';
import ResourceDetails from './ResourceDetails';

const meta: Meta<typeof ResourceDetails> = {
  component: ResourceDetails,
  args: {
    detail: storyModel(),
    detailLoading: false,
    detailNotFound: false,
    onSelect: () => {},
    hasColumnLineage: false,
    userState: 'core',
  },
  // The uniqueId in the route is only read for the not-found message, but a route is
  // needed for the param to resolve at all.
  parameters: {
    docsApp: { initialEntries: ['/details/model.jaffle_shop.customers'] },
  },
};

export default meta;
type Story = StoryObj<typeof ResourceDetails>;

/**
 * The route wrapper around `NodeDetail`. It owns only the three loading/absent states
 * below; everything else is delegated, which is why the fetch happens in `App.tsx` and
 * arrives here as props.
 */
export const Default: Story = {};

export const Loading: Story = {
  args: { detail: null, detailLoading: true },
};

/**
 * A resource type the index does not serve a detail projection for. Distinct from
 * "loading" and from a 404: the node exists, the detail view does not — so the copy
 * says exactly that rather than claiming the resource is missing.
 */
export const NotFound: Story = {
  args: { detail: null, detailNotFound: true },
  parameters: {
    docsApp: { initialEntries: ['/details/operation.jaffle_shop.on_run_end'] },
  },
};

/** Neither loading nor not-found, but no detail either — falls back to the loading
 *  copy rather than rendering an empty page. */
export const NoDetailYet: Story = {
  args: { detail: null },
};

export const SourceDetail: Story = {
  args: { detail: storySource() },
  parameters: {
    docsApp: { initialEntries: ['/details/source.jaffle_shop.raw.customers'] },
  },
};

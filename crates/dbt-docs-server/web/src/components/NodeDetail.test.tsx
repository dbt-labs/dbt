import { screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { ResourceType } from '../shared';
import { DETAIL_REGISTRY } from '../shared/data-sources/duckdb/details';
import exposureFx from '../test/fixtures/details/exposure.json';
import groupFx from '../test/fixtures/details/group.json';
import macroFx from '../test/fixtures/details/macro.json';
import metricFx from '../test/fixtures/details/metric.json';
import modelFx from '../test/fixtures/details/model.json';
import savedQueryFx from '../test/fixtures/details/saved_query.json';
import seedFx from '../test/fixtures/details/seed.json';
import semanticModelFx from '../test/fixtures/details/semantic_model.json';
import snapshotFx from '../test/fixtures/details/snapshot.json';
import sourceFx from '../test/fixtures/details/source.json';
import testFx from '../test/fixtures/details/test.json';
import { renderWithProviders } from '../test/renderWithProviders';
import { NodeDetail } from './NodeDetail';

/**
 * Render coverage for the asset-detail page across every resource type the index
 * serves. Each case drives the real production path — a projected row → the
 * per-type mapper → `NodeDetail` — so a mapper that mishandles a row shape
 * surfaces as a render failure here.
 *
 * The fixtures started as captures from a real project, which is why they are
 * unusually broad: hundreds of nodes, full lineage graphs, every optional field
 * populated. That breadth is the point, and it is worth preserving. Their
 * *contents* are synthetic and must stay that way — every identifier, description,
 * owner, and URL has been replaced. This repository is published, so do not
 * refresh these by re-capturing from a real project.
 */

/** One captured set of endpoint responses for a single asset. */
type DetailFixture = {
  detail: { unique_id: string; name: string };
  lineage: { status: number; body: unknown };
  columnLineage: { status: number; body: unknown };
};

const CASES: ReadonlyArray<[ResourceType, DetailFixture]> = [
  ['model', modelFx as DetailFixture],
  ['seed', seedFx as DetailFixture],
  ['snapshot', snapshotFx as DetailFixture],
  ['source', sourceFx as DetailFixture],
  ['exposure', exposureFx as DetailFixture],
  ['metric', metricFx as DetailFixture],
  ['macro', macroFx as DetailFixture],
  ['semantic_model', semanticModelFx as DetailFixture],
  ['test', testFx as DetailFixture],
  ['saved_query', savedQueryFx as DetailFixture],
  ['group', groupFx as DetailFixture],
];

function res(status: number, body: unknown): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    statusText: '',
    json: () => Promise.resolve(body),
  } as Response;
}

/** Route each request to the matching captured endpoint response, replaying its
 *  real status (detail 200, lineage 200/404, column-lineage 412 gated, …). */
function stubFetch(fx: DetailFixture) {
  return vi.fn((url: string | URL) => {
    const u = String(url);
    if (u.includes('/column-lineage')) {
      return Promise.resolve(res(fx.columnLineage.status, fx.columnLineage.body));
    }
    if (u.includes('/lineage')) {
      return Promise.resolve(res(fx.lineage.status, fx.lineage.body));
    }
    return Promise.resolve(res(200, fx.detail));
  });
}

describe('NodeDetail', () => {
  afterEach(() => vi.unstubAllGlobals());

  it.each(CASES)('renders the %s detail page', async (type, fx) => {
    vi.stubGlobal('fetch', stubFetch(fx));

    // Map the fixture through the same per-type mapper the real source uses, rather
    // than round-tripping it through a source. Fewer moving parts, and it covers the
    // mapping that actually ships.
    const asset = DETAIL_REGISTRY[type]!.map(fx.detail as Record<string, unknown>);
    expect(asset).not.toBeNull();

    renderWithProviders(
      <NodeDetail
        asset={asset!}
        onSelect={vi.fn()}
        hasColumnLineage={false}
        userState={null}
      />,
    );

    // Reached the header rather than crashing to a blank screen.
    await waitFor(() =>
      expect(screen.getByRole('heading', { level: 1 })).toHaveTextContent(
        fx.detail.name,
      ),
    );
  });
});

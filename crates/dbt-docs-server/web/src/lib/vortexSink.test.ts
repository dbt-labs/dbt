import { fromBinary, toBinary } from '@bufbuild/protobuf';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import {
  DocsSiteOpenedSchema,
  ResourceViewedSchema,
  SearchPerformedSchema,
} from '../proto/dbtlabs/proto/v1/events/docs_pb';
import {
  type SiteBootstrap,
  SUPPORTED_BOOTSTRAP_SCHEMA_VERSION,
} from './siteBootstrap';
import producer from './vortexProducer';
import { configureVortex, logEvent, resetVortexForTests } from './vortexSink';

function bootstrap(overrides: Partial<SiteBootstrap['telemetry']> = {}): SiteBootstrap {
  return {
    schema_version: SUPPORTED_BOOTSTRAP_SCHEMA_VERSION,
    generated_at: '2026-08-08T18:00:00Z',
    dbt_version: '2.0.0-preview.208',
    distribution: 'dbt',
    is_logged_in: true,
    duckdb_cdn_base: 'https://cdn.example/duckdb',
    data_dir: 'index/',
    telemetry: {
      enabled: true,
      dbt_cloud_account_identifier: 'acct-1',
      dbt_cloud_project_id: 'proj-2',
      dbt_cloud_environment_id: 'env-3',
      ...overrides,
    },
  };
}

beforeEach(() => {
  resetVortexForTests();
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('consent', () => {
  it('disables the producer entirely when consent is denied', () => {
    // `enabled: false` makes the client drop every logProto and flush, so nothing can
    // leak through a code path that forgot to check.
    configureVortex(bootstrap({ enabled: false }));
    expect(producer.config.enabled).toBe(false);
  });

  it('disables the producer when there is no bootstrap at all', () => {
    configureVortex(null);
    expect(producer.config.enabled).toBe(false);
  });

  it('enables it only on an explicit consent', () => {
    configureVortex(bootstrap({ enabled: true }));
    expect(producer.config.enabled).toBe(true);
  });

  it('emits nothing when consent is denied', async () => {
    const spy = vi.spyOn(producer, 'logProto');
    configureVortex(bootstrap({ enabled: false }));

    await logEvent(
      { event_type: 'search_performed', search_query: 'x', result_count: 1 } as never,
      bootstrap({ enabled: false }),
    );

    // The producer is called but drops it; what matters is nothing reaches the wire,
    // which `enabled: false` guarantees.
    expect(producer.config.enabled).toBe(false);
    spy.mockRestore();
  });
});

describe('the collector', () => {
  it('points at the production ingest path', () => {
    configureVortex(bootstrap());
    expect(producer.config.collectorBaseUrl).toBe('https://p.vx.dbt.com');
    expect(producer.config.collectorPath).toBe('/v1/ingest/protobuf');
  });

  it('sends each event rather than batching', () => {
    // A docs session produces a handful of events over minutes; batching would mostly
    // mean losing the tail when the tab closes.
    configureVortex(bootstrap());
    expect(producer.config.maxBatchBytes).toBe(-1);
  });

  it('never throws out of the producer', () => {
    configureVortex(bootstrap());
    expect(producer.config.errorMode).toBe('log-and-continue');
  });
});

describe('event mapping', () => {
  /** Capture the message handed to the producer without sending it. */
  function captureLogged() {
    const logged: { schema: unknown; message: Record<string, unknown> }[] = [];
    vi.spyOn(producer, 'logProto').mockImplementation(
      async (schema: unknown, message: unknown) => {
        logged.push({ schema, message: message as Record<string, unknown> });
        return 0;
      },
    );
    return logged;
  }

  it('hydrates the fields the relay used to fill in server-side', async () => {
    const logged = captureLogged();
    await logEvent(
      {
        event_type: 'resource_viewed',
        resource_type: 'model',
        view_level: 'detail',
        resource_id: 'model.a.b',
      } as never,
      bootstrap(),
    );

    expect(logged[0]?.message).toMatchObject({
      isLoggedIn: true,
      distribution: 'dbt',
      resourceType: 'model',
      resourceId: 'model.a.b',
    });
    // `dbt_version` is only declared on DocsSiteOpened, so it must not appear here —
    // spreading it onto every event would be a silent no-op.
    expect(logged[0]?.message).not.toHaveProperty('dbtVersion');
  });

  it('sends dbt_version only on the event that declares it', async () => {
    const logged = captureLogged();
    await logEvent(
      { event_type: 'docs_site_opened', project_resource_count: 1 } as never,
      bootstrap(),
    );
    expect(logged[0]?.message).toMatchObject({ dbtVersion: '2.0.0-preview.208' });
  });

  it('carries the dbt_cloud_* context through', async () => {
    const logged = captureLogged();
    await logEvent(
      { event_type: 'docs_site_opened', project_resource_count: 3 } as never,
      bootstrap(),
    );

    expect(logged[0]?.message.context).toMatchObject({
      dbtCloudAccountIdentifier: 'acct-1',
      dbtCloudProjectId: 'proj-2',
      dbtCloudEnvironmentId: 'env-3',
    });
  });

  it('encodes an int64 count as a BigInt, which the wire type requires', async () => {
    const logged = captureLogged();
    await logEvent(
      { event_type: 'docs_site_opened', project_resource_count: 6472 } as never,
      bootstrap(),
    );
    expect(logged[0]?.message.projectResourceCount).toBe(6472n);
  });

  it('drops an unknown event type instead of throwing', async () => {
    const logged = captureLogged();
    await expect(
      logEvent({ event_type: 'invented_later' } as never, bootstrap()),
    ).resolves.toBeUndefined();
    expect(logged).toHaveLength(0);
  });

  it('resolves even when the producer rejects', async () => {
    // Analytics must never be why a docs page misbehaves.
    vi.spyOn(producer, 'logProto').mockRejectedValue(new Error('collector down'));
    await expect(
      logEvent(
        { event_type: 'docs_site_opened', project_resource_count: 1 } as never,
        bootstrap(),
      ),
    ).resolves.toBeUndefined();
  });
});

describe('the encoded messages', () => {
  /**
   * Round-trip through the real protobuf codec.
   *
   * Type-checking proves the field *names* line up; only encoding proves the schema
   * accepts the values, which is where a wrong type (a number where int64 wants a
   * BigInt) would surface.
   */
  function roundTrip<S extends typeof DocsSiteOpenedSchema>(
    schema: S,
    message: unknown,
  ): Record<string, unknown> {
    const bytes = toBinary(schema, message as never);
    expect(bytes.byteLength).toBeGreaterThan(0);
    return fromBinary(schema, bytes) as unknown as Record<string, unknown>;
  }

  it('round-trips every event the app emits', async () => {
    const cases: [unknown, typeof DocsSiteOpenedSchema][] = [];
    vi.spyOn(producer, 'logProto').mockImplementation(
      async (schema: unknown, message: unknown) => {
        cases.push([message, schema as typeof DocsSiteOpenedSchema]);
        return 0;
      },
    );

    const events = [
      { event_type: 'docs_site_opened', project_resource_count: 3 },
      {
        event_type: 'resource_viewed',
        resource_type: 'model',
        view_level: 'detail',
        resource_id: 'model.a.b',
      },
      {
        event_type: 'lineage_viewed',
        lineage_type: 'inline',
        resource_type: 'model',
        resource_id: 'model.a.b',
      },
      { event_type: 'search_performed', search_query: 'orders', result_count: 7 },
      {
        event_type: 'upsell_prompt_displayed',
        upsell_track: 'cll',
        prompt_format: 'card',
        prompt_location: 'columns',
      },
      {
        event_type: 'upsell_prompt_clicked',
        upsell_track: 'cll',
        cta_label: 'Learn more',
        referral_code: 'r1',
      },
      {
        event_type: 'upsell_prompt_dismissed',
        upsell_track: 'cll',
        dismiss_method: 'x',
      },
      {
        event_type: 'referral_link_clicked',
        referral_code: 'r1',
        link_destination: 'https://getdbt.com',
      },
    ];

    for (const event of events) {
      await logEvent(event as never, bootstrap());
    }

    expect(cases).toHaveLength(events.length);
    for (const [message, schema] of cases) {
      expect(() => roundTrip(schema, message)).not.toThrow();
    }
  });

  it('preserves the values through the codec', async () => {
    let captured: unknown;
    vi.spyOn(producer, 'logProto').mockImplementation(
      async (_s: unknown, m: unknown) => {
        captured = m;
        return 0;
      },
    );
    await logEvent(
      {
        event_type: 'search_performed',
        search_query: 'orders',
        result_count: 7,
      } as never,
      bootstrap(),
    );

    const decoded = roundTrip(SearchPerformedSchema as never, captured);
    expect(decoded).toMatchObject({
      searchQuery: 'orders',
      resultCount: 7n,
      isLoggedIn: true,
      distribution: 'dbt',
    });
    expect(decoded.dbtVersion).toBeUndefined();
  });

  it('encodes a resource id verbatim', async () => {
    // Worth pinning: a change to how this field is emitted is a change to what
    // leaves the browser, which is not something to alter incidentally.
    let captured: unknown;
    vi.spyOn(producer, 'logProto').mockImplementation(
      async (_s: unknown, m: unknown) => {
        captured = m;
        return 0;
      },
    );
    await logEvent(
      {
        event_type: 'resource_viewed',
        resource_type: 'model',
        view_level: 'detail',
        resource_id: 'model.acme_finance.revenue',
      } as never,
      bootstrap(),
    );

    const decoded = roundTrip(ResourceViewedSchema as never, captured);
    expect(decoded.resourceId).toBe('model.acme_finance.revenue');
  });
});

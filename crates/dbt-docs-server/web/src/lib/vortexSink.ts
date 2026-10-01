/**
 * Sends docs analytics straight from the browser.
 *
 * The server-side relay (`POST /api/v1/analytics/events`) existed because ADR-9
 * judged three things unsafe in the browser: enforcing consent, keeping PII off the
 * wire, and CORS. Two of those no longer hold for a static site:
 *
 * - **Consent** is resolved at export time and inlined into `window.__DBT_DOCS__`.
 *   Only the machine running `dbt docs generate` can read the project and profile,
 *   so that is the only place the question *can* be answered — a static site has no
 *   relay to ask.
 * - **CORS** was verified against the collector rather than assumed, and needed no
 *   infrastructure change. Nothing about the site carries a credential.
 *
 * The third — what the events contain — is a schema question rather than a transport
 * one, and this change does not alter it: the same eight events the relay forwarded,
 * carrying the same fields. See ADR-10.
 *
 * The fields the relay used to hydrate server-side (`distribution`, `dbt_version`,
 * `is_logged_in`, the `dbt_cloud_*` context) now ride in from the same seam that fed
 * it, baked in at export time.
 *
 * ## The `node:fs` build warning is expected
 *
 * `vite build` reports that `node:fs` was externalized, imported by
 * `@dbt-labs/vortex`'s producer. It is a dynamic `await import` inside
 * `_sendBatchDev`, behind `env.PLATFORM === "nodejs"` — a dev-mode file sink. In a
 * browser that branch is never taken; the producer logs
 * "VORTEX_DEV_MODE is unsupported…" and returns. Vite replaces the specifier with an
 * empty stub module that is never evaluated, and the guard survives minification.
 * Verified in the built bundle, not assumed.
 */

import type { DescMessage, MessageInitShape } from '@bufbuild/protobuf';
import { create } from '@bufbuild/protobuf';

import {
  DocsSiteOpenedSchema,
  LineageViewedSchema,
  ReferralLinkClickedSchema,
  ResourceViewedSchema,
  SearchPerformedSchema,
  UpsellPromptClickedSchema,
  UpsellPromptDismissedSchema,
  UpsellPromptDisplayedSchema,
} from '../proto/dbtlabs/proto/v1/events/docs_pb';
import type { AnalyticsEvent } from '../types';
import type { SiteBootstrap } from './siteBootstrap';
import producer from './vortexProducer';

/** Where the collector lives. Protobuf only. */
const COLLECTOR_BASE_URL = 'https://p.vx.dbt.com';
const COLLECTOR_PATH = '/v1/ingest/protobuf';

/**
 * Flush every event as it is logged.
 *
 * `maxBatchBytes: -1` means no buffering. A docs session produces a handful of
 * events spread over minutes, so batching would mostly mean losing the tail when
 * the tab closes — and the exit flush cannot rely on an async send completing.
 */
const NO_BATCHING = -1;

/** Whether the producer has been configured. Configuring twice is harmless but noisy. */
let configured = false;

/**
 * Point the producer at the collector, or disable it.
 *
 * `enabled: false` makes the client silently drop every subsequent `logProto` and
 * `flush`, which is the behavior we want when consent is denied: callers do not have
 * to branch, and nothing can leak through a path that forgot to check.
 */
export function configureVortex(bootstrap: SiteBootstrap | null): void {
  producer.configure({
    enabled: bootstrap?.telemetry.enabled === true,
    collectorBaseUrl: COLLECTOR_BASE_URL,
    collectorPath: COLLECTOR_PATH,
    maxBatchBytes: NO_BATCHING,
    errorMode: 'log-and-continue',
  });
  configured = true;
}

/**
 * Fields on *every* docs event, hydrated from the build rather than the client.
 *
 * `dbt_version` is deliberately not here: only `DocsSiteOpened` declares it, so it
 * is added by that event's own mapping. `create()` would silently ignore it on the
 * others, which is exactly the kind of quiet no-op worth not writing.
 */
interface CommonHydration {
  isLoggedIn: boolean;
  distribution: string;
  context: {
    dbtCloudAccountIdentifier: string;
    dbtCloudProjectId: string;
    dbtCloudEnvironmentId: string;
  };
}

function hydrationFrom(bootstrap: SiteBootstrap): CommonHydration {
  return {
    isLoggedIn: bootstrap.is_logged_in,
    distribution: bootstrap.distribution,
    context: {
      dbtCloudAccountIdentifier: bootstrap.telemetry.dbt_cloud_account_identifier,
      dbtCloudProjectId: bootstrap.telemetry.dbt_cloud_project_id,
      dbtCloudEnvironmentId: bootstrap.telemetry.dbt_cloud_environment_id,
    },
  };
}

/** One event's schema and its payload fields, minus the hydrated ones. */
type Mapped = { schema: DescMessage; fields: Record<string, unknown> } | null;

/**
 * Map an app event onto its proto message.
 *
 * Returns `null` for an event type with no schema rather than throwing: analytics is
 * best-effort, and a new event type reaching an older site should drop silently
 * rather than break a page.
 */
function mapEvent(event: AnalyticsEvent, bootstrap: SiteBootstrap): Mapped {
  switch (event.event_type) {
    case 'docs_site_opened':
      return {
        schema: DocsSiteOpenedSchema,
        fields: {
          // `int64` on the wire, so protobuf-es wants a BigInt.
          projectResourceCount: BigInt(event.project_resource_count),
          // The only event that declares it.
          dbtVersion: bootstrap.dbt_version,
        },
      };
    case 'resource_viewed':
      return {
        schema: ResourceViewedSchema,
        fields: {
          resourceType: event.resource_type,
          viewLevel: event.view_level,
          resourceId: event.resource_id,
        },
      };
    case 'lineage_viewed':
      return {
        schema: LineageViewedSchema,
        fields: {
          lineageType: event.lineage_type,
          resourceType: event.resource_type,
          resourceId: event.resource_id,
        },
      };
    case 'search_performed':
      return {
        schema: SearchPerformedSchema,
        fields: {
          searchQuery: event.search_query,
          // `int64`, like `project_resource_count`. `create()` would coerce a plain
          // number, but the two 64-bit fields in this schema are worth being explicit
          // about rather than relying on that.
          resultCount: BigInt(event.result_count),
        },
      };
    case 'upsell_prompt_displayed':
      return {
        schema: UpsellPromptDisplayedSchema,
        fields: {
          upsellTrack: event.upsell_track,
          promptFormat: event.prompt_format,
          promptLocation: event.prompt_location,
        },
      };
    case 'upsell_prompt_clicked':
      return {
        schema: UpsellPromptClickedSchema,
        fields: {
          upsellTrack: event.upsell_track,
          ctaLabel: event.cta_label,
          referralCode: event.referral_code,
        },
      };
    case 'upsell_prompt_dismissed':
      return {
        schema: UpsellPromptDismissedSchema,
        fields: {
          upsellTrack: event.upsell_track,
          dismissMethod: event.dismiss_method,
        },
      };
    case 'referral_link_clicked':
      return {
        schema: ReferralLinkClickedSchema,
        fields: {
          referralCode: event.referral_code,
          linkDestination: event.link_destination,
        },
      };
    default:
      return null;
  }
}

/**
 * Log one event.
 *
 * Resolves even on failure. Analytics must never be the reason a docs page misbehaves,
 * which is also why the producer runs in `log-and-continue`.
 */
export async function logEvent(
  event: AnalyticsEvent,
  bootstrap: SiteBootstrap,
): Promise<void> {
  if (!configured) configureVortex(bootstrap);

  const mapped = mapEvent(event, bootstrap);
  if (!mapped) return;

  const message = create(mapped.schema, {
    ...hydrationFrom(bootstrap),
    ...mapped.fields,
  } as MessageInitShape<DescMessage>);

  try {
    await producer.logProto(mapped.schema, message);
  } catch {
    // Best-effort; `log-and-continue` should already prevent this.
  }
}

/**
 * Flush anything pending.
 *
 * With batching off there is normally nothing queued, but a `logProto` still in
 * flight when the tab closes benefits from this — and it costs nothing when the
 * queue is empty.
 */
export async function flushVortex(): Promise<void> {
  try {
    await producer.flush();
  } catch {
    // Best-effort.
  }
}

/** Test seam: forget that the producer was configured. */
export function resetVortexForTests(): void {
  configured = false;
}

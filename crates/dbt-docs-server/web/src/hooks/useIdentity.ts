import { useQuery, type UseQueryResult } from '@tanstack/react-query';

import { readSiteBootstrap } from '../lib/siteBootstrap';
import type { Identity } from '../types';

/** Consent denied. Any failure to resolve identity resolves to this — telemetry
 *  never fails open. */
const CONSENT_DENIED: Identity = { is_logged_in: false, analytics_enabled: false };

/**
 * The telemetry consent gate, resolved once per session.
 *
 * There is no endpoint to ask. Consent was decided at export time — only the machine
 * running `dbt docs generate` can read the project and the profile — so it rides into
 * the page with the build scalars in `window.__DBT_DOCS__`.
 *
 * A missing or unreadable bootstrap resolves to {@link CONSENT_DENIED}: consent fails
 * closed, and callers can gate telemetry on `data` without handling an error branch.
 */
export function useIdentity(): UseQueryResult<Identity> {
  return useQuery({
    queryKey: ['identity'],
    queryFn: (): Identity => {
      const site = readSiteBootstrap();
      if (!site) return CONSENT_DENIED;
      return {
        is_logged_in: site.is_logged_in,
        analytics_enabled: site.telemetry.enabled,
      };
    },
    // Consent doesn't change within a session, and the queryFn cannot fail, so
    // retries add nothing.
    retry: false,
    staleTime: Infinity,
  });
}

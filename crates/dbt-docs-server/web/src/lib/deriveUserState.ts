import type { Distribution, UserState } from '../shared';

/**
 * Maps the domain {@link Distribution} onto the {@link UserState} the upsell
 * components consume. Distribution reports two orthogonal facts:
 *   - `isProprietary` — running the proprietary (dbt v2) build vs dbt Core.
 *   - `isLoggedIn` — whether the user has authenticated.
 *
 * Mapping:
 *   - not dbt v2 (dbt Core)             → `core`
 *   - dbt v2, `isLoggedIn=false`        → `proprietary-anon`
 *   - dbt v2, `isLoggedIn=true`         → `proprietary-logged-in`
 *
 * The `via-catalog` state isn't surfaced by the docs-server (docs are
 * embedded in the Catalog app from the outside) and stays out of scope.
 *
 * Returns `null` when `distInfo` is still loading so callers can hold off
 * rendering and avoid a flicker through the Core default on first paint.
 */
export function deriveUserState(distInfo: Distribution | null): UserState | null {
  if (distInfo == null) return null;
  if (!distInfo.isProprietary) return 'core';
  return distInfo.isLoggedIn ? 'proprietary-logged-in' : 'proprietary-anon';
}

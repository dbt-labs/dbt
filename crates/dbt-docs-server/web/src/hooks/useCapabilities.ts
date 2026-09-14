import type { Capabilities, Distribution } from '../shared';

/**
 * Flat capability flags consumed by the upsell pipeline. The shape is
 * deliberately decoupled from the domain {@link Capabilities} /
 * {@link Distribution} so the upsell components don't have to know whether a
 * given signal came from the BE today, is hardcoded in the FE today, or
 * moves between the two tomorrow.
 *
 *   - `hasCll` — column-level lineage is available for this user.
 *     Sourced from `Capabilities.hasColumnLineage`.
 *   - `hasDbtState` — dbt State is already active for this project.
 *     Sourced from `Capabilities.hasDbtState`.
 *   - `isProprietary` — running against the proprietary (dbt v2) distribution
 *     rather than dbt Core.
 *   - `isLoggedIn` — user has authenticated against the distribution.
 */
export interface UpgradeCapabilities {
  hasCll: boolean;
  hasDbtState: boolean;
  isProprietary: boolean;
  isLoggedIn: boolean;
}

/**
 * Derives {@link UpgradeCapabilities} from the domain {@link Capabilities} and
 * {@link Distribution}. Returns `null` until both have resolved so callers can
 * hold off on rendering upsell surfaces and avoid a flicker through the "core"
 * default.
 */
export function deriveUpgradeCapabilities(
  capabilities: Capabilities | null,
  distInfo: Distribution | null,
): UpgradeCapabilities | null {
  if (capabilities == null || distInfo == null) return null;
  return {
    hasCll: capabilities.hasColumnLineage,
    hasDbtState: capabilities.hasDbtState,
    isProprietary: distInfo.isProprietary,
    isLoggedIn: distInfo.isLoggedIn,
  };
}

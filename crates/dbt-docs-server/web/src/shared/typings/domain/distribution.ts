/**
 * Build identity of the metadata backend — *not* a feature capability (see
 * {@link Capabilities}). Separates "what binary" (`isProprietary`) from "is the
 * user authed" (`isLoggedIn`), which the REST `/distribution` endpoint reports
 * as `name` / `is_logged_in`. A GraphQL catalog source can synthesize this
 * (platform → both true); a source that has no notion of distribution omits
 * `fetchDistribution` entirely.
 */
export type Distribution = {
  /** Running against the proprietary (dbt v2) build rather than dbt Core. */
  isProprietary: boolean;
  /** User has authenticated against the distribution. */
  isLoggedIn: boolean;
  /** Build version string, when reported. */
  version?: string;
};

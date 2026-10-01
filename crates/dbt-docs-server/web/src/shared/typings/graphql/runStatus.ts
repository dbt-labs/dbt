import type { RunStatus } from '../discoveryEnums';

/**
 * GraphQL run status enum values (lowercase). Accepts the string-literal
 * union so any consumer GQL client's enum can pass in without cross-module
 * enum nominal-type issues.
 */
export type GqlRunStatusLiteral = `${RunStatus}`;

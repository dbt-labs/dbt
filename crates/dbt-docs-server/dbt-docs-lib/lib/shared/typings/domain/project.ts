/**
 * Identity of the dbt project a metadata source serves — its name, dbt/adapter
 * versions, and git state. Maps the REST `GET /api/v1/project` response; a
 * GraphQL catalog source can synthesize this from its environment/project
 * record. A source with no notion of a single project omits `fetchProject`.
 */
export type Project = {
  name: string;
  projectId?: string;
  description?: string | null;
  dbtVersion?: string;
  adapterType?: string;
  gitSha?: string | null;
  gitBranch?: string | null;
  gitIsDirty?: boolean | null;
};

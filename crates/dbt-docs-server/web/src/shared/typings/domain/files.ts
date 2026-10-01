/**
 * One file-bearing resource in the project, from the REST `GET /api/v1/files`
 * endpoint — the rows the file tree is built from. `resourceType` is a plain
 * string (not {@link ResourceType}) because the file view spans entries with no
 * asset counterpart, e.g. `doc`. `patchPath` is populated only for nodes and
 * macros. A source with no on-disk file view (e.g. a GraphQL catalog) omits
 * `fetchFiles`.
 */
export type FileEntry = {
  uniqueId: string;
  name: string;
  resourceType: string;
  packageName: string;
  originalFilePath: string;
  patchPath?: string | null;
};

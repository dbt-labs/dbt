/**
 * A project's `{% docs __overview__ %}` block — the markdown rendered on the
 * docs landing page.
 *
 * Named `ProjectOverview` rather than `Overview` because `OverviewContainer`
 * already occupies that name in the shared barrel and is unrelated.
 *
 * A source resolves `fetchOverview` to `null` when no package defines a block,
 * which is the signal to render the built-in default rather than an error.
 */
export type ProjectOverview = {
  uniqueId: string;
  /** Null only on an index written before the column was populated. */
  packageName: string | null;
  blockContents: string;
};

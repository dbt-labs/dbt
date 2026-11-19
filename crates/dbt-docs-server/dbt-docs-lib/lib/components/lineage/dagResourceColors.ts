/** Saturated fg/viz token per resource type, for DAG node/badge fills and the
 *  minimap. Base values only (not `Hover`/`Muted`) -- confirmed against
 *  Jess's reference swatch of all 8 resource-type badges as the universal
 *  OSS palette. These are theme-invariant (same hex in both the light and
 *  dark theme blocks in tokens.css). Never use the `--bgDag*`/pale family
 *  here -- too washed out against the swatch reference. */
export const DAG_RESOURCE_COLOR: Record<string, string> = {
  model: 'var(--fgVizModel)',
  source: 'var(--fgVizSource)',
  test: 'var(--fgVizTest)',
  unit_test: 'var(--fgVizTest)',
  seed: 'var(--fgVizSeed)',
  exposure: 'var(--fgVizExposure)',
  metric: 'var(--fgVizMetric)',
  semantic_model: 'var(--fgVizSemanticmodel)',
  snapshot: 'var(--fgVizSnapshot)',
  macro: 'var(--fgVizMacro)',
  analysis: 'var(--fgVizAnalysis)',
  saved_query: 'var(--fgVizSavedquery)',
  function: 'var(--fgVizFunction)',
  column: 'var(--fgVizColumn)',
};

export function dagResourceColor(type: string): string {
  return DAG_RESOURCE_COLOR[type] ?? 'var(--bgDisabled)';
}

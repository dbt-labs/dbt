/**
 * Modeling-layer classification, from `LAYER_CONDITIONS` in `handlers/models.rs`.
 *
 * One source of truth for the projected `CASE` and every `WHERE` filter (the model
 * list and cross-type search), so what a row displays and what a filter matches
 * cannot drift — the same reason the Rust side kept them in one table.
 *
 * Each builder takes the SQL expression for the file path, because the model list
 * reads `n.original_file_path` while search reads the union's `b.original_file_path`.
 */

export const LAYER_NAMES = ['Staging', 'Intermediate', 'Marts'] as const;

/** `[layer, condition]` pairs, with the conditions written against `pathExpr`. */
export function layerConditions(pathExpr: string): [string, string][] {
  const p = `lower(${pathExpr})`;
  return [
    [
      'Staging',
      `${p} LIKE '%/staging/%'
     OR ${p} LIKE '%/stg_%'
     OR ${p} LIKE 'staging/%'`,
    ],
    [
      'Intermediate',
      `${p} LIKE '%/intermediate/%'
     OR ${p} LIKE '%/int_%'
     OR ${p} LIKE 'intermediate/%'`,
    ],
    [
      'Marts',
      `${p} LIKE '%/marts/%'
     OR ${p} LIKE '%/dim_%'
     OR ${p} LIKE '%/fct_%'
     OR ${p} LIKE 'marts/%'`,
    ],
  ];
}

/** `CASE` that projects the layer name, or NULL when no rule matches. */
export function modelingLayerCase(pathExpr: string): string {
  return `CASE
${layerConditions(pathExpr)
  .map(([layer, cond]) => `  WHEN ${cond} THEN '${layer}'`)
  .join('\n')}
  ELSE NULL
END`;
}

/** `(cond) OR (cond)` for the selected layers, or `null` when none are known. */
export function modelingLayerPredicate(
  pathExpr: string,
  selected: string[],
): string | null {
  const conds = layerConditions(pathExpr)
    .filter(([layer]) => selected.includes(layer))
    .map(([, cond]) => `(${cond})`);
  return conds.length ? `(${conds.join(' OR ')})` : null;
}

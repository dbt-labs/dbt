/**
 * Filters a node's raw `config` block down to what's worth showing in the
 * Config tab. dbt fills in defaults for nearly every config key, so an
 * unfiltered dump is mostly noise — this drops null/empty entries
 * (recursively, for nested blocks like `contract`/`docs`) and reports `null`
 * when nothing visible remains, which the caller uses to hide the tab.
 */
function isEmptyValue(value: unknown): boolean {
  if (value == null) return true;
  if (typeof value === 'string') return value.trim() === '';
  if (Array.isArray(value)) return value.length === 0;
  if (typeof value === 'object') return Object.keys(value as object).length === 0;
  return false;
}

export function filterConfig(
  config: Record<string, unknown> | null | undefined,
): Record<string, unknown> | null {
  if (!config) return null;

  const out: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(config)) {
    if (isEmptyValue(value)) continue;
    if (typeof value === 'object' && !Array.isArray(value)) {
      const nested = filterConfig(value as Record<string, unknown>);
      if (nested) out[key] = nested;
    } else {
      out[key] = value;
    }
  }
  return Object.keys(out).length > 0 ? out : null;
}

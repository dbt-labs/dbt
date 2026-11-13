import { twJoin } from 'tailwind-merge';

function formatLabel(key: string): string {
  return key.replace(/_/g, ' ').toUpperCase();
}

function formatValue(value: unknown): string {
  if (Array.isArray(value)) return value.map(String).join(' ');
  if (typeof value === 'boolean') return value ? 'true' : 'false';
  return String(value);
}

function isNestedObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** `docs.node_color` (and similarly named keys) hold a CSS color string —
 *  worth a visual swatch rather than just the raw text. */
function isColorKey(key: string): boolean {
  return key.toLowerCase().endsWith('color');
}

function ConfigRow({
  label,
  value,
  isNested,
}: {
  label: string;
  value: unknown;
  isNested: boolean;
}) {
  const indent = isNested && 'ml-4 border-l border-borderMuted pl-4';

  if (isNestedObject(value)) {
    return (
      <div className={twJoin(indent)}>
        <div className="mb-2 text-xs font-medium uppercase tracking-wide text-fgDecorative">
          {formatLabel(label)}
        </div>
        <div className="space-y-2">
          {Object.entries(value).map(([k, v]) => (
            <ConfigRow key={k} label={k} value={v} isNested />
          ))}
        </div>
      </div>
    );
  }

  if (isColorKey(label) && typeof value === 'string') {
    return (
      <div className={twJoin('flex items-baseline justify-between gap-4', indent)}>
        <span className="text-xs font-medium uppercase tracking-wide text-fgDecorative">
          {formatLabel(label)}
        </span>
        <span
          className="inline-block h-4 w-4 shrink-0 rounded-full border border-borderMuted"
          style={{ backgroundColor: value }}
          role="img"
          aria-label={value}
          title={value}
        />
      </div>
    );
  }

  return (
    <div className={twJoin('flex items-baseline justify-between gap-4', indent)}>
      <span className="text-xs font-medium uppercase tracking-wide text-fgDecorative">
        {formatLabel(label)}
      </span>
      <span className="text-right text-sm text-fgMain">{formatValue(value)}</span>
    </div>
  );
}

export type ConfigDisplayProps = {
  /** Pre-filtered config — pass the output of `filterConfig`, never a raw block. */
  config: Record<string, unknown>;
};

export function ConfigDisplay({ config }: ConfigDisplayProps) {
  // `static_analysis` is a dbt-v2-only concept (Core has no static SQL
  // analyzer) — when it's genuinely absent, show a locked teaser rather than
  // silently omitting the row, so Core users know the capability exists.
  const hasStaticAnalysis = 'static_analysis' in config;

  return (
    <div className="space-y-3">
      {Object.entries(config).map(([key, value]) => (
        <ConfigRow key={key} label={key} value={value} isNested={false} />
      ))}
      {!hasStaticAnalysis && (
        <div className="flex items-baseline justify-between gap-4">
          <span className="text-xs font-medium uppercase tracking-wide text-fgDecorative">
            STATIC ANALYSIS
          </span>
          <span className="text-right text-sm italic text-fgDecorative">
            Enable dbt v2 to view
          </span>
        </div>
      )}
    </div>
  );
}

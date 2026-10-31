import { FC, ReactNode } from 'react';
import { ChevronRight, Key, Workflow } from 'lucide-react';

import { Button } from '../../components/ui/Button';
import { truthy } from '../util/array';
import { Badge, Badges } from './Badge';

export type ColumnCardShellConstraint = { name?: string | null; type?: string | null };

export type ColumnCardShellProps = {
  name: string;
  type?: string | null;
  isPrimaryKey?: boolean;
  constraints?: ColumnCardShellConstraint[];
  /** When true a caret toggle renders to the left of the name. */
  expandable?: boolean;
  expanded?: boolean;
  onToggleExpanded?: () => void;
  /** Caret tooltip strings keyed by current state. */
  toggleTooltip?: { open: string; closed: string };
  /** data-trackingid for pendo. */
  trackingId?: string;
  /** When true, renders a CLL badge button next to the expand caret. */
  showCllBadge?: boolean;
  /** Description / inherited markup rendered inside the main row. */
  description?: ReactNode;
  /** Inline content rendered between the main row and any body rows
   *  (explorer uses this for the "Meta Contains:" highlight line). */
  belowDescription?: ReactNode;
  /** Body rows rendered as additional dividers below the main row
   *  (explorer uses this for `TestStatusRows`). */
  bodyRows?: ReactNode;
  /** Revealed when `expanded` is true. */
  expandedBody?: ReactNode;
};

/** Presentational shell shared by dbt-explorer and dbt-docs-v2. Owns
 *  the row chrome (divide-y border, caret + name + PK icon, right-pinned
 *  type badge, constraints) and slots in description / extra rows /
 *  expanded body. Expansion is fully controlled — caller wires state. */
export const ColumnCardShell: FC<ColumnCardShellProps> = ({
  name,
  type,
  isPrimaryKey,
  constraints,
  expandable,
  expanded,
  onToggleExpanded,
  toggleTooltip,
  trackingId,
  showCllBadge,
  description,
  belowDescription,
  bodyRows,
  expandedBody,
}) => {
  const constraintLabels =
    constraints?.map((c) => c.name ?? c.type).filter(truthy) ?? [];

  return (
    <li
      data-testid={`${name}-card`}
      aria-label={name}
      className="overflow-hidden rounded-lg border border-borderMuted bg-bgMain text-sm"
    >
      <div className="px-4 py-2">
        <div className="flex items-center text-fgMain">
          <span className="font-label not-sr-only mx-1 flex flex-1 items-center gap-1.5">
            {name}
            {type && <Badge aria-label="data type">{type.toUpperCase()}</Badge>}
            {isPrimaryKey && (
              <>
                <Key data-testid={`pk-icon-${name}`} className="ml-2 mr-1 size-3" />
                PK
              </>
            )}
          </span>
          {constraintLabels.length > 0 && <Badges content={constraintLabels} />}
          {showCllBadge && expandable && (
            <button
              onClick={onToggleExpanded}
              className="ml-1 flex items-center gap-1 rounded-md bg-bgBrandMuted px-2 py-0.5 text-xs font-semibold text-fgBrand"
            >
              <Workflow className="size-3" />
              CLL
            </button>
          )}
          {expandable && (
            <Button
              variant="ghost"
              onClick={onToggleExpanded}
              className="flex-2 not-sr-only pb-0.5 align-middle"
              testId={`toggle-column-card-${name}`}
              icon={
                <ChevronRight className={`size-3 ${expanded ? 'rotate-90' : ''}`} />
              }
              size="icon-xs"
              tooltip={expanded ? toggleTooltip?.open : toggleTooltip?.closed}
            />
          )}
        </div>
        {description && <div className="w-full">{description}</div>}
      </div>
      {belowDescription}
      {bodyRows}
      {expandable && expanded && expandedBody}
    </li>
  );
};

import { ChevronDown, Filter } from 'lucide-react';

import { useLineageStore } from '../../stores/lineageStore';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from '../ui/DropdownMenu';

/** "Default", "Materialization", and "Model layer" render a real badge on
 *  each node card now (see lib/lensBadges). "Last run", "Last test", and
 *  "Query history" are hidden until there's an actual data source for them:
 *  that data isn't in the lineage payload at all (execution results aren't
 *  fetched here), and query history is Discovery-API/Cloud-only regardless.
 *  Their `lensBadgeFor` cases and store values still exist -- only the menu
 *  entries are removed -- so wiring them back in later is a one-line add
 *  here, not a data-layer change. */
const LENSES = [
  {
    label: 'Default',
    description: 'By resource type (i.e. model, source, test, etc.)',
  },
  {
    label: 'Materialization',
    description: 'How the model gets built: table, view, etc.',
  },
  { label: 'Model layer', description: 'Staging, intermediate, marts, etc.' },
];

export function DagLensesDropdown() {
  const lens = useLineageStore((s) => s.activeLens);
  const setLens = useLineageStore((s) => s.setActiveLens);

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          className="flex h-9 items-center gap-1.5 rounded-md border border-borderMain bg-bgMain px-3 text-sm text-fgMain hover:bg-bgMainHover"
        >
          <Filter className="size-3.5" />
          Lenses
          <ChevronDown className="size-3.5 text-fgDecorative" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-72">
        <div className="px-2 pb-1.5 pt-1 text-xs text-fgAlt">Filter view by lenses</div>
        <DropdownMenuRadioGroup value={lens} onValueChange={setLens}>
          {LENSES.map(({ label, description }) => (
            <DropdownMenuRadioItem
              key={label}
              value={label}
              className="items-start py-2"
            >
              <span className="flex flex-col gap-0.5">
                <span className="text-sm text-fgMain">{label}</span>
                <span className="text-xs text-fgAlt">{description}</span>
              </span>
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

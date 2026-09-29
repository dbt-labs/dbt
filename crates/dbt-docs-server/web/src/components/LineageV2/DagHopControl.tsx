import type { ReactNode } from 'react';
import { useNavigate } from 'react-router-dom';
import { ChevronDown } from 'lucide-react';

import { cn } from '../../lib/utils';
import { paths } from '../../routes';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '../ui/DropdownMenu';

const HOP_PRESETS = [1, 3, 6, Infinity];

function formatHop(direction: 'upstream' | 'downstream', hops: number): string {
  const n = Number.isFinite(hops) ? String(hops) : 'max';
  return direction === 'upstream' ? `${n}+` : `+${n}`;
}

export interface DagHopControlProps {
  direction: 'upstream' | 'downstream';
  hops: number;
  onChange: (hops: number) => void;
  className?: string;
}

/** One side of the hop bar (upstream or downstream depth). Presets only for now --
 *  the Figma states also show clicking directly into the number to type a custom
 *  value inline; that keystroke-editing interaction is deferred to its own pass. */
export function DagHopControl({
  direction,
  hops,
  onChange,
  className,
}: DagHopControlProps) {
  const navigate = useNavigate();

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          className={cn(
            'flex h-9 items-center gap-1 rounded-md border border-borderMain bg-bgMain px-3 text-sm text-fgMain hover:bg-bgMainHover',
            className,
          )}
        >
          {formatHop(direction, hops)}
          <ChevronDown className="size-3.5 text-fgDecorative" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align={direction === 'upstream' ? 'start' : 'end'}>
        <DropdownMenuRadioGroup
          value={String(hops)}
          onValueChange={(value) => onChange(Number(value))}
        >
          {HOP_PRESETS.map((preset) => (
            <DropdownMenuRadioItem key={preset} value={String(preset)}>
              {formatHop(direction, preset)}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
        <DropdownMenuSeparator />
        <DropdownMenuItemLink onSelect={() => navigate(paths.home())}>
          Go to Catalog
        </DropdownMenuItemLink>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** Plain, non-radio menu item -- "Go to Catalog" isn't one of the hop options,
 *  it's an escape hatch out of the graph entirely. */
function DropdownMenuItemLink({
  onSelect,
  children,
}: {
  onSelect: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onSelect}
      className="flex w-full cursor-pointer select-none items-center rounded px-2 py-1.5 text-left text-sm text-fgBrand outline-none hover:bg-bgMainHover"
    >
      {children}
    </button>
  );
}

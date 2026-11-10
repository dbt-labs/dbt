import { X } from 'lucide-react';

import { cn } from '../../lib/utils';

export interface PillData {
  id: string;
  value: string;
}

export interface PillProps extends PillData {
  /** Parent should respond by not rendering this pill. */
  onClickRemove?: (pill: PillData) => void;
  className?: string;
}

export function Pill({ id, value, onClickRemove, className }: PillProps) {
  return (
    <span
      className={cn(
        'inline-flex items-center gap-1 rounded-full bg-bgMainActive px-2 py-1 text-sm text-fgMain',
        className,
      )}
    >
      {value}
      {onClickRemove && (
        <button
          type="button"
          aria-label={`Remove ${value}`}
          onClick={() => onClickRemove({ id, value })}
          className="text-fgDecorative hover:text-fgMain"
        >
          <X className="size-3" />
        </button>
      )}
    </span>
  );
}

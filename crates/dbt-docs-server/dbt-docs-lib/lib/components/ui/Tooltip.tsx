import { type ReactNode, useRef, useState } from 'react';
import * as RadixTooltip from '@radix-ui/react-tooltip';

export type TooltipPlacement =
  | 'top'
  | 'top-start'
  | 'top-end'
  | 'bottom'
  | 'bottom-start'
  | 'bottom-end'
  | 'left'
  | 'left-start'
  | 'left-end'
  | 'right'
  | 'right-start'
  | 'right-end';

export interface TooltipProps {
  /** The tooltip trigger. Pass a function to receive a ref for truncation detection. */
  children: ReactNode | ((ref: (node: HTMLElement | null) => void) => ReactNode);
  /** The content to display in the tooltip */
  content: ReactNode;
  /** Custom placement for the tooltip */
  placement?: TooltipPlacement;
  /** Only show the tooltip once the element passed the children-function ref overflows */
  displayOnlyWhenTruncated?: boolean;
  /** Accepted for drop-in parity with the old sourdough Tooltip; asChild renders the same either way */
  childrenAreInteractive?: boolean;
  /** Classnames for the wrapper span around the trigger (not the tooltip bubble) */
  className?: string;
}

function splitPlacement(placement: TooltipPlacement) {
  const [side, align] = placement.split('-') as [
    'top' | 'bottom' | 'left' | 'right',
    'start' | 'end' | undefined,
  ];
  return { side, align: align ?? 'center' } as const;
}

export function Tooltip({
  children,
  content,
  placement = 'top',
  displayOnlyWhenTruncated = false,
  className,
}: TooltipProps) {
  const [open, setOpen] = useState(false);
  const elRef = useRef<HTMLElement | null>(null);
  const { side, align } = splitPlacement(placement);

  const setRef = (node: HTMLElement | null) => {
    elRef.current = node;
  };

  const handleOpenChange = (next: boolean) => {
    if (next && displayOnlyWhenTruncated) {
      const el = elRef.current;
      const isTruncated =
        !!el && (el.scrollWidth > el.clientWidth || el.scrollHeight > el.clientHeight);
      if (!isTruncated) return;
    }
    setOpen(next);
  };

  // setRef only ever assigns elRef.current for a later open-intent check; it's never
  // read here, so this doesn't run afoul of the "no ref reads during render" rule.
  // eslint-disable-next-line react-hooks/refs
  const trigger = typeof children === 'function' ? children(setRef) : children;

  return (
    <RadixTooltip.Provider delayDuration={200}>
      <RadixTooltip.Root open={open} onOpenChange={handleOpenChange}>
        <RadixTooltip.Trigger asChild>
          <span className={className}>{trigger}</span>
        </RadixTooltip.Trigger>
        <RadixTooltip.Portal>
          <RadixTooltip.Content
            side={side}
            align={align}
            sideOffset={4}
            // `break-words`: content is capped at `max-w-xs`, so longer content
            // may need to break mid-token to prevent overflow
            className="z-50 max-w-xs break-words rounded bg-fgMain px-2 py-1 text-xs text-bgMain shadow-md"
          >
            {content}
            <RadixTooltip.Arrow className="fill-fgMain" />
          </RadixTooltip.Content>
        </RadixTooltip.Portal>
      </RadixTooltip.Root>
    </RadixTooltip.Provider>
  );
}

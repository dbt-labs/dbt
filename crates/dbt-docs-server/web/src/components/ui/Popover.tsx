import { type ReactElement, type ReactNode } from 'react';
import * as HoverCardPrimitive from '@radix-ui/react-hover-card';

export interface PopoverProps {
  labelledBy: string;
  content: ReactNode;
  zIndex?: number;
  children: ReactElement;
}

export function Popover({ labelledBy, content, zIndex, children }: PopoverProps) {
  return (
    <HoverCardPrimitive.Root>
      <HoverCardPrimitive.Trigger asChild>{children}</HoverCardPrimitive.Trigger>
      <HoverCardPrimitive.Portal>
        <HoverCardPrimitive.Content
          aria-label={labelledBy}
          style={{ zIndex }}
          className="rounded-md border border-borderMain bg-bgMain p-3 text-sm text-fgMain shadow-md"
        >
          {content}
        </HoverCardPrimitive.Content>
      </HoverCardPrimitive.Portal>
    </HoverCardPrimitive.Root>
  );
}

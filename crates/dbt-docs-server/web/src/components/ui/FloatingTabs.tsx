import { type ReactNode } from 'react';
import * as TabsPrimitive from '@radix-ui/react-tabs';

import { cn } from '../../lib/utils';

export interface FloatingTabsProps {
  children: ReactNode;
  value: string;
  onValueChange: (value: string) => void;
  testId?: string;
  className?: string;
}

export function FloatingTabs({
  children,
  value,
  onValueChange,
  testId,
  className,
}: FloatingTabsProps) {
  return (
    <TabsPrimitive.Root
      value={value}
      onValueChange={onValueChange}
      className={className}
      data-testid={testId}
    >
      <TabsPrimitive.List className="flex gap-4 border-b border-borderMain">
        {children}
      </TabsPrimitive.List>
    </TabsPrimitive.Root>
  );
}

export interface FloatingTabProps {
  id: string;
  text?: ReactNode;
  children?: ReactNode;
  count?: number;
  trackingId?: string;
  testId?: string;
}

function FloatingTab({
  id,
  text,
  children,
  count,
  trackingId,
  testId,
}: FloatingTabProps) {
  return (
    <TabsPrimitive.Trigger
      value={id}
      data-testid={testId}
      data-trackingid={trackingId}
      className={cn(
        "relative whitespace-nowrap px-1 py-2 text-sm font-medium text-fgDecorative transition-colors after:absolute after:inset-x-0 after:-bottom-px after:h-0.5 after:bg-transparent after:content-[''] hover:text-fgMain data-[state=active]:text-fgMain data-[state=active]:after:bg-fgMain",
      )}
    >
      <span className="flex items-center gap-1.5">
        {text ?? children}
        {count !== undefined && (
          <span className="rounded-full bg-bgBadgeNeutralMuted px-1.5 text-xs text-fgMain">
            {count}
          </span>
        )}
      </span>
    </TabsPrimitive.Trigger>
  );
}

FloatingTabs.Tab = FloatingTab;
export { FloatingTab };

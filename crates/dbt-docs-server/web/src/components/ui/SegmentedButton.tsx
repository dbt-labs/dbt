import type { ReactNode } from 'react';
import * as ToggleGroupPrimitive from '@radix-ui/react-toggle-group';
import { cva, type VariantProps } from 'class-variance-authority';

import { cn } from '../../lib/utils';

export type Segment = {
  label: string;
  value: string;
  startIcon?: ReactNode;
  endIcon?: ReactNode;
};

const rootVariants = cva(
  'inline-flex items-center gap-0.5 rounded-md border border-borderMain p-0.5',
  {
    variants: {
      variant: {
        default: '',
        stretch: 'w-full',
      },
    },
    defaultVariants: { variant: 'default' },
  },
);

const segmentVariants = cva(
  'inline-flex items-center justify-center gap-1.5 whitespace-nowrap rounded font-medium text-fgDecorative transition-colors hover:text-fgMain data-[state=on]:bg-bgMainActive data-[state=on]:text-fgMain',
  {
    variants: {
      size: {
        sm: 'px-2 py-1 text-sm',
        md: 'px-3 py-1.5 text-sm',
      },
      variant: {
        default: '',
        stretch: 'flex-1',
      },
    },
    defaultVariants: { size: 'sm', variant: 'default' },
  },
);

export interface SegmentedButtonProps extends VariantProps<typeof rootVariants> {
  segments: Segment[];
  onSelect: (value: string) => void;
  selectedValue?: string;
  size?: VariantProps<typeof segmentVariants>['size'];
  className?: string;
  /** Extra classes on each segment item, e.g. to match a taller toolbar's height. */
  itemClassName?: string;
  testId?: string;
}

export function SegmentedButton({
  segments,
  onSelect,
  selectedValue,
  size,
  variant,
  className,
  itemClassName,
  testId,
}: SegmentedButtonProps) {
  return (
    <ToggleGroupPrimitive.Root
      type="single"
      value={selectedValue}
      onValueChange={(value) => {
        if (value) onSelect(value);
      }}
      className={cn(rootVariants({ variant }), className)}
      data-testid={testId}
    >
      {segments.map((segment) => (
        <ToggleGroupPrimitive.Item
          key={segment.value}
          value={segment.value}
          className={cn(segmentVariants({ size, variant }), itemClassName)}
        >
          {segment.startIcon}
          {segment.label}
          {segment.endIcon}
        </ToggleGroupPrimitive.Item>
      ))}
    </ToggleGroupPrimitive.Root>
  );
}

import { type HTMLAttributes, type ReactNode } from 'react';
import { cva, type VariantProps } from 'class-variance-authority';

import { cn } from '../../lib/utils';

const badgeVariants = cva(
  'inline-flex items-center gap-1 whitespace-nowrap rounded-full font-medium',
  {
    variants: {
      variant: {
        default: 'bg-bgBadgeIndigoMuted text-fgMain',
        secondary: 'bg-bgBadgeNeutralMuted text-fgMain',
        destructive: 'bg-bgBadgeRedMuted text-fgDanger',
        outline: 'border border-borderMuted bg-transparent text-fgMain',
      },
      size: {
        xs: 'px-1 py-0.5 text-xs',
        sm: 'px-1.5 py-0.5 text-xs',
        lg: 'px-2 py-1 text-sm',
      },
    },
    defaultVariants: {
      variant: 'secondary',
      size: 'sm',
    },
  },
);

export type BadgeVariant = NonNullable<VariantProps<typeof badgeVariants>['variant']>;

export interface BadgeProps
  extends
    Omit<HTMLAttributes<HTMLSpanElement>, 'className'>,
    VariantProps<typeof badgeVariants> {
  text: string;
  /** Optional icon rendered left of the badge text. */
  icon?: ReactNode;
  className?: string;
}

export function Badge({ text, variant, size, icon, className, ...props }: BadgeProps) {
  return (
    <span className={cn(badgeVariants({ variant, size }), className)} {...props}>
      {icon}
      {text}
    </span>
  );
}

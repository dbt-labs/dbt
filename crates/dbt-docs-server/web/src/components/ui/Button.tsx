import { type MouseEvent, type ReactNode } from 'react';
import { cva, type VariantProps } from 'class-variance-authority';

import { cn } from '../../lib/utils';
import { Tooltip } from './Tooltip';

const buttonVariants = cva(
  'inline-flex items-center justify-center whitespace-nowrap rounded-md font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-bgBrand disabled:pointer-events-none disabled:opacity-50',
  {
    variants: {
      variant: {
        default: 'bg-bgBrand text-white hover:bg-bgBrandHover',
        outline: 'border border-borderMain bg-bgMain text-fgMain hover:bg-bgMainHover',
        ghost: 'text-fgMain hover:bg-bgMainHover',
      },
      size: {
        xs: 'gap-1 px-2 py-1 text-xs',
        sm: 'gap-1.5 px-3 py-1.5 text-sm',
        /** Icon-only, no text — square padding at three sizes instead of a text button's horizontal padding. */
        'icon-xs': 'p-1',
        'icon-sm': 'p-1.5',
        'icon-lg': 'p-2',
      },
    },
    defaultVariants: {
      variant: 'outline',
      size: 'sm',
    },
  },
);

export type ButtonVariant = NonNullable<VariantProps<typeof buttonVariants>['variant']>;
type ButtonVariantSize = VariantProps<typeof buttonVariants>['size'];

/** Matches sourdough's Sizes enum structurally (same string values) so callers
 *  still passing it are still assignable -- TypeScript checks shape, not
 *  origin, for a plain string-literal union. */
type SourdoughSize = 'xs' | 'sm' | 'md' | 'lg' | 'xl' | '2xl' | '3xl' | '4xl';

export interface ButtonProps extends Omit<VariantProps<typeof buttonVariants>, 'size'> {
  size?: ButtonVariantSize | SourdoughSize;
  text?: ReactNode;
  icon?: ReactNode;
  ariaLabel?: string;
  tooltip?: string;
  onClick?(event: MouseEvent<HTMLButtonElement>): void;
  className?: string;
  testId?: string;
  disabled?: boolean;
}

export function Button({
  variant,
  size,
  text,
  icon,
  ariaLabel,
  tooltip,
  onClick,
  className,
  testId,
  disabled,
}: ButtonProps) {
  const button = (
    <button
      type="button"
      aria-label={ariaLabel}
      onClick={onClick}
      disabled={disabled}
      data-testid={testId}
      className={cn(
        buttonVariants({ variant, size: size as ButtonVariantSize }),
        className,
      )}
    >
      {icon}
      {text}
    </button>
  );

  if (!tooltip) return button;

  return <Tooltip content={tooltip}>{button}</Tooltip>;
}

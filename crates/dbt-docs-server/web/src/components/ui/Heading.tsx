import { createElement, type ReactNode } from 'react';
import { cva, type VariantProps } from 'class-variance-authority';

import { cn } from '../../lib/utils';

const headingVariants = cva('font-semibold text-fgMain', {
  variants: {
    size: {
      '1': 'text-4xl',
      '2': 'text-3xl',
      '3': 'text-2xl',
      '4': 'text-xl',
      '5': 'text-lg',
      '6': 'text-base',
    },
  },
  defaultVariants: {
    size: '3',
  },
});

type HeadingComponent = 'h1' | 'h2' | 'h3' | 'h4' | 'h5' | 'h6';

export interface HeadingProps extends VariantProps<typeof headingVariants> {
  children: ReactNode;
  /** Component tag override — e.g. font size of h6 but rendered as a <p>. */
  component?: HeadingComponent;
  testId?: string;
  className?: string;
}

export function Heading({
  children,
  component = 'h2',
  size,
  testId,
  className,
}: HeadingProps) {
  return createElement(
    component,
    { className: cn(headingVariants({ size }), className), 'data-testid': testId },
    children,
  );
}

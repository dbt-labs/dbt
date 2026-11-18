import { type ReactNode } from 'react';
import { twMerge } from 'tailwind-merge';

export interface CardProps {
  children?: ReactNode;
  className?: string;
  /** Reduces body padding for dense layouts. */
  isCompact?: boolean;
}

export function Card({ children, className, isCompact }: CardProps) {
  return (
    <div
      className={twMerge(
        'rounded-lg border border-borderMuted bg-bgMain shadow-rest',
        isCompact ? 'p-2' : 'p-4',
        className,
      )}
    >
      {children}
    </div>
  );
}

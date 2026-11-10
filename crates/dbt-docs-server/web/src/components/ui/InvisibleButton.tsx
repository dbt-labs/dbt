import { type ButtonHTMLAttributes } from 'react';

import { cn } from '../../lib/utils';

export interface InvisibleButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  testId?: string;
}

export function InvisibleButton({ className, testId, ...props }: InvisibleButtonProps) {
  return (
    <button
      type="button"
      className={cn('appearance-none border-0 bg-transparent p-0 text-left', className)}
      data-testid={testId}
      {...props}
    />
  );
}

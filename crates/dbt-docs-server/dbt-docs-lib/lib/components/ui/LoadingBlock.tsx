import { twMerge } from 'tailwind-merge';

export interface LoadingBlockProps {
  /** Pixel width, or -1 to fill the available width. */
  width?: number;
  height?: number;
  className?: string;
}

export function LoadingBlock({ width, height, className }: LoadingBlockProps) {
  return (
    <div
      className={twMerge('animate-pulse rounded bg-bgMainActive', className)}
      style={{
        width: width === undefined || width === -1 ? '100%' : width,
        height: height ?? 16,
      }}
    />
  );
}

import { FC, ReactNode, useCallback, useEffect, useState } from 'react';
import { twJoin } from 'tailwind-merge';

export interface ResizableProps {
  /** The content to make resizable */
  children: ReactNode;
  /** Default width in pixels */
  defaultWidth?: number;
  /** Minimum width in pixels */
  minWidth?: number;
  /** Id for the container */
  id?: string;
  /** Maximum width in pixels - set to undefined for unlimited width */
  maxWidth?: number;
  /** Direction of resize handle - defaults to 'right' */
  direction?: 'left' | 'right';
  /** Additional className for the container */
  className?: string;
  /** Additional className for the resize handle */
  handleClassName?: string;
  /** Callback when width changes */
  onWidthChange?: (width: number) => void;
}

export const Resizable: FC<ResizableProps> = ({
  children,
  defaultWidth = 300,
  minWidth = 200,
  id,
  maxWidth,
  direction = 'right',
  className,
  handleClassName,
  onWidthChange,
}) => {
  const [isResizing, setIsResizing] = useState(false);
  const [currentWidth, setCurrentWidth] = useState(defaultWidth);
  const [resizeStartX, setResizeStartX] = useState(0);
  const [resizeStartWidth, setResizeStartWidth] = useState(0);

  const getViewportConstraints = useCallback(() => {
    const viewportMaxWidth = window.innerWidth - 100;

    return {
      effectiveMaxWidth: maxWidth !== undefined ? maxWidth : viewportMaxWidth,
      effectiveMinWidth: minWidth,
      effectiveWidth: defaultWidth,
    };
  }, [maxWidth, minWidth, defaultWidth]);

  const startResizing = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault();
      e.stopPropagation();
      setIsResizing(true);
      setResizeStartX(e.clientX);
      setResizeStartWidth(currentWidth);
    },
    [currentWidth],
  );

  const stopResizing = useCallback(() => {
    setIsResizing(false);
  }, []);

  const resize = useCallback(
    (e: MouseEvent) => {
      if (isResizing) {
        const { effectiveMaxWidth, effectiveMinWidth } = getViewportConstraints();
        const deltaX = e.clientX - resizeStartX;
        let newWidth: number;

        if (direction === 'right') {
          newWidth = resizeStartWidth + deltaX;
        } else {
          newWidth = resizeStartWidth - deltaX;
        }

        const constrainedWidth = Math.max(
          effectiveMinWidth,
          Math.min(effectiveMaxWidth, newWidth),
        );

        setCurrentWidth(constrainedWidth);
        onWidthChange?.(constrainedWidth);
      }
    },
    [
      isResizing,
      getViewportConstraints,
      direction,
      onWidthChange,
      resizeStartX,
      resizeStartWidth,
    ],
  );

  useEffect(() => {
    const handleResize = () => {
      const newConstraints = getViewportConstraints();
      setCurrentWidth((current) => {
        const newWidth = Math.max(
          newConstraints.effectiveMinWidth,
          Math.min(newConstraints.effectiveMaxWidth, current),
        );

        onWidthChange?.(newWidth);
        return newWidth;
      });
    };

    window.addEventListener('resize', handleResize);
    return () => window.removeEventListener('resize', handleResize);
  }, [getViewportConstraints, onWidthChange]);

  useEffect(() => {
    if (isResizing) {
      window.addEventListener('mousemove', resize);
      window.addEventListener('mouseup', stopResizing);
      document.body.style.cursor = 'ew-resize';
      document.body.style.userSelect = 'none';

      return () => {
        window.removeEventListener('mousemove', resize);
        window.removeEventListener('mouseup', stopResizing);
        document.body.style.cursor = '';
        document.body.style.userSelect = '';
      };
    }
  }, [isResizing, resize, stopResizing]);

  useEffect(() => {
    const { effectiveWidth } = getViewportConstraints();
    setCurrentWidth(effectiveWidth);
    onWidthChange?.(effectiveWidth);
  }, [defaultWidth, getViewportConstraints, onWidthChange]);

  const handlePosition = direction === 'right' ? 'right-0' : 'left-0';
  const handleBorder = direction === 'right' ? 'border-r' : 'border-l';

  return (
    <div
      id={id}
      className={twJoin('relative', className)}
      style={{
        width: `${currentWidth}px`,
        transition: isResizing ? 'none' : undefined,
      }}
    >
      {children}

      <div
        className={twJoin(
          'absolute top-0 z-10 h-full w-1 cursor-ew-resize border-borderMuted hover:border-0 hover:bg-bgBrandHover active:border-0 active:bg-bgBrandActive',
          handlePosition,
          handleBorder,
          handleClassName,
        )}
        onMouseDown={startResizing}
        onClick={(e) => e.stopPropagation()}
      />
    </div>
  );
};

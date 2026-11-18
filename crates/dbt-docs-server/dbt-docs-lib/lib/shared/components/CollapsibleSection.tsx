import { useState } from 'react';
import { ChevronRight } from 'lucide-react';
import { twJoin } from 'tailwind-merge';

type CollapsibleSectionProps = React.DetailedHTMLProps<
  React.HTMLAttributes<HTMLDivElement>,
  HTMLDivElement
> & {
  children: React.ReactNode;
  toExpand: React.ReactNode;
  /** Initial state */
  isOpen?: boolean;
  closeAltText: string;
  expandAltText: string;
  disable?: boolean;
  shouldIndent?: boolean;
  onToggle?: (isOpen: boolean) => void;
};

export const CollapsibleSection = ({
  children,
  toExpand,
  isOpen = false,
  closeAltText,
  expandAltText,
  disable,
  shouldIndent = true,
  onToggle = () => {},
  ...divProps
}: CollapsibleSectionProps) => {
  const [isActuallyOpen, setIsOpen] = useState(!disable && isOpen);
  shouldIndent ||= !disable;

  return (
    <>
      <div {...divProps} className={twJoin('relative', divProps.className)}>
        {!disable && (
          <button
            className="absolute inset-0 cursor-pointer opacity-0"
            onClick={() => {
              setIsOpen((state) => !state);
              onToggle(!isActuallyOpen);
            }}
          >
            <div className="sr-only">
              {isActuallyOpen ? closeAltText : expandAltText}
            </div>
          </button>
        )}
        <div className="z-5 pointer-events-none flex w-full gap-2">
          <div
            className={twJoin(
              'flex-0 flex items-center',
              disable && shouldIndent && 'mr-4',
            )}
          >
            {!disable && (
              <ChevronRight
                className={twJoin(
                  isActuallyOpen ? 'rotate-90' : '',
                  'pointer-events-none size-4 align-middle transition-transform',
                )}
              />
            )}
          </div>
          <div className="font-label-lg flex flex-1 items-center">{children}</div>
        </div>
      </div>
      {isActuallyOpen && toExpand}
    </>
  );
};

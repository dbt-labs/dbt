import { FC } from 'react';

import { Tooltip } from '../../components/ui/Tooltip';

type HeadingProps = {
  children?: React.ReactNode;
  /** Additional content to render next to the heading*/
  additional?: {
    left?: React.ReactNode;
    right?: React.ReactNode;
  };
  /** Whether to apply truncation on the content. You may want to disable this if the children can render options */
  shouldTruncate?: boolean;
  className?: string;
};

export const PageHeading: FC<HeadingProps> = ({
  className,
  shouldTruncate = true,
  ...props
}) => {
  return (
    <div className={className}>
      {props.additional?.left}
      <Tooltip
        displayOnlyWhenTruncated
        content={props.children}
        className="w-full"
        childrenAreInteractive={false}
      >
        {(ref) => (
          <div ref={ref} className={shouldTruncate ? 'truncate' : ''}>
            <h1 className="text-3xl text-fgMain">{props.children}</h1>
          </div>
        )}
      </Tooltip>
      {props.additional?.right}
    </div>
  );
};

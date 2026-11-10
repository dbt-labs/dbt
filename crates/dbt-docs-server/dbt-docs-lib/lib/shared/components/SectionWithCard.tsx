import { FC, ReactNode } from 'react';

import { Card, type CardProps } from '../../components/ui/Card';
import { Heading } from '../../components/ui/Heading';

type SectionProps = {
  heading?: string;
  headingClassname?: string;
  withHeading?: ReactNode;
  withCard?: boolean;
} & CardProps;

export const DetailsSection: FC<SectionProps> = ({
  heading,
  children,
  headingClassname,
  withHeading,
  withCard = true,
  className: inputClassName,
  ...cardProps
}) => {
  const className = `${inputClassName} overflow-hidden`;
  return children ? (
    <div className="my-10 space-y-4">
      {heading && (
        <div className="flex items-baseline">
          <span className={`flex-0 ${headingClassname ?? ''}`}>
            <Heading size="6" component="h3">
              {heading}
            </Heading>
          </span>
          {withHeading}
        </div>
      )}
      {withCard ? (
        <Card {...cardProps} className={className}>
          {children}
        </Card>
      ) : (
        children
      )}
    </div>
  ) : null;
};

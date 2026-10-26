import { FC } from 'react';
import { twMerge } from 'tailwind-merge';

type AnnotationBadgeType = 'text';

interface AnnotationBadgeProps extends React.ComponentPropsWithoutRef<'span'> {
  text?: string;
  type?: AnnotationBadgeType;
}

export const AnnotationBadge: FC<AnnotationBadgeProps> = ({
  text = 'Beta',
  type = 'text',
  ...spanProps
}) => {
  return (
    <span
      {...spanProps}
      className={twMerge(
        'ml-2 align-text-top text-xs font-medium uppercase text-fgBrandAlt',
        spanProps.className,
      )}
    >
      {text}
    </span>
  );
};

import { FC } from 'react';
import { twMerge } from 'tailwind-merge';

export const Badge: FC<React.ComponentPropsWithoutRef<'span'>> = ({
  children,
  className,
  ...rest
}) => {
  return (
    <span
      {...rest}
      className={twMerge(
        'whitespace-nowrap rounded-sm bg-bgBadgeNeutralMuted px-1 py-0.5 text-xs text-fgMain',
        className,
      )}
    >
      {children}
    </span>
  );
};

interface BadgesProps {
  content: string[];
}

export const Badges: FC<BadgesProps> = ({ content }) => {
  return content.length ? (
    <ul data-testid="test-badges" aria-label="Tests" className={`max-w-[300px]`}>
      {content.map((content, index) => (
        <li key={`${content}${index}`} className="m-0.5 inline-block">
          <Badge>{content}</Badge>
        </li>
      ))}
    </ul>
  ) : null;
};

import { FC } from 'react';
import ReactMarkdown from 'react-markdown';
import rehypeExternalLinks from 'rehype-external-links';
import rehypeRaw from 'rehype-raw';
import remarkGfm from 'remark-gfm';

import { Badge } from './Badge';

type SemanticAspectCardProps = {
  name: string | null;
  description: string | null;
  type: string | null;
};

export const SemanticAspectCard: FC<SemanticAspectCardProps> = ({
  name,
  type,
  description,
}) => {
  if (!name) return null;

  return (
    <li
      data-testid={`${name}-semantic-card`}
      key={name}
      aria-label={name}
      className="overflow-hidden rounded-lg border border-borderMuted bg-bgMain  text-sm"
    >
      <div className="px-4 py-2">
        <div className="flex">
          <span className="not-sr-only mx-2 flex-1 leading-[21px]">{name}</span>
          <span className="flex-0">
            <Badge aria-label="data type">{type?.toUpperCase()}</Badge>
          </span>
        </div>
        {description && (
          <ReactMarkdown
            className="prose prose-sm dark:prose-invert mt-2 w-full min-w-full text-fgDecorative"
            remarkPlugins={[remarkGfm]}
            rehypePlugins={[rehypeRaw, [rehypeExternalLinks, { target: '_blank' }]]}
          >
            {description}
          </ReactMarkdown>
        )}
      </div>
    </li>
  );
};

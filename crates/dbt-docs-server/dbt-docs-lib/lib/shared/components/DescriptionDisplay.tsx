import { FC } from 'react';
import ReactMarkdown from 'react-markdown';
import rehypeExternalLinks from 'rehype-external-links';
import rehypeRaw from 'rehype-raw';
import remarkGfm from 'remark-gfm';
import { twMerge } from 'tailwind-merge';

type DescriptionDisplayProps = {
  description: string | null | undefined;
  className?: string;
};

const BASE_CLASS = 'prose prose-sm text-fgMain dark:prose-invert text-sm';

export const DescriptionDisplay: FC<DescriptionDisplayProps> = ({
  description,
  className,
}) => {
  if (!description) {
    return (
      <p className={twMerge(BASE_CLASS, 'italic', className)}>
        This resource does not have a description
      </p>
    );
  }

  return (
    <ReactMarkdown
      className={twMerge(BASE_CLASS, className)}
      remarkPlugins={[remarkGfm]}
      rehypePlugins={[rehypeRaw, [rehypeExternalLinks, { target: '_blank' }]]}
    >
      {description}
    </ReactMarkdown>
  );
};

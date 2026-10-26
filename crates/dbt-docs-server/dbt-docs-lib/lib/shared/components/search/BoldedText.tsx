import { Fragment, useMemo } from 'react';
import { twMerge } from 'tailwind-merge';

interface BoldedTextParams extends React.ComponentPropsWithoutRef<'span'> {
  text: string;
  shouldBeBold: string;
  boldProps?: React.ComponentPropsWithoutRef<'b'>;
  ref?: React.Ref<HTMLHeadingElement>;
}

function escapeRegExp(string: string) {
  return string.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

export function BoldedText({
  text,
  shouldBeBold,
  boldProps,
  ref,
  ...spanProps
}: BoldedTextParams) {
  if (!shouldBeBold)
    return (
      <span {...spanProps} className="truncate">
        {text}
      </span>
    );

  shouldBeBold = escapeRegExp(shouldBeBold).trim().replaceAll(/[. ]+/g, '|');
  const textArray = text.split(RegExp(shouldBeBold, 'ig'));
  const match = text.match(RegExp(shouldBeBold, 'ig'));

  return (
    <span {...spanProps}>
      {textArray.map((item, index) => (
        <Fragment key={index}>
          {item}
          {index !== textArray.length - 1 && match && (
            <b
              {...boldProps}
              className={twMerge(boldProps?.className, 'font-semibold')}
            >
              {match[index]}
            </b>
          )}
        </Fragment>
      ))}
    </span>
  );
}

/** Matches bolded tags emitted by the search backend, e.g. `<b>foo</b>`. */
export const BOLD_TAG_REGEX = /<\/? *[bB]>/gm;

interface SanitizeBoldTextParams extends React.ComponentPropsWithoutRef<'span'> {
  text: string | null;
  query: string;
  boldProps?: React.ComponentPropsWithoutRef<'b'>;
}

export function BoldSearchHighlight({ text, query, ...props }: SanitizeBoldTextParams) {
  if (!text) return null;
  const tagsRemoved = text?.replace(BOLD_TAG_REGEX, '');
  if (!tagsRemoved?.includes(query)) {
    return <SanitizeBoldText {...props} text={text} query={query} />;
  }
  const textWithHighlights = tagsRemoved.replaceAll(query, `<b>${query}</b>`);
  return <SanitizeBoldText {...props} text={textWithHighlights} query={query} />;
}

export function SanitizeBoldText({
  text,
  query: _query,
  boldProps,
  ...props
}: SanitizeBoldTextParams) {
  const textWithBoldInserts = useMemo(() => {
    if (text == null) return null;
    const textParts = text.split(BOLD_TAG_REGEX);
    return textParts.map((part, index) => {
      const isBold = index % 2 === 1;
      if (isBold) {
        return (
          <b key={index} {...boldProps}>
            {part}
          </b>
        );
      }
      return part;
    });
  }, [boldProps, text]);
  if (!textWithBoldInserts) return null;

  return <span {...props}>{textWithBoldInserts}</span>;
}

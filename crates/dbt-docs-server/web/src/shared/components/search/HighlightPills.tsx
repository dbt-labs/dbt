import { FC } from 'react';

import { Badge } from '../../../components/ui/Badge';
import { Link } from '../../../components/ui/Link';
import { Tooltip } from '../../../components/ui/Tooltip';
import { toTitleCase } from '../../util/string';
import { BOLD_TAG_REGEX, SanitizeBoldText } from './BoldedText';
import { MatchedField } from './types';

export type HighlightsByField = Partial<Record<MatchedField, string[]>>;

export interface HighlightPillsProps {
  highlights: HighlightsByField;
  query: string;
  /**
   * Link builder for column-matched highlights. When omitted, column highlights
   * render as plain text inside the tooltip.
   */
  getColumnHref?: (columnName: string) => string;
}

const FIELD_LABEL: Record<MatchedField, string> = {
  name: 'Name',
  column: 'Column',
  tag: 'Tag',
  fqn: 'Relation',
  description: 'Description',
};

const truncateHighlight = (field: MatchedField, text: string): string => {
  const MAX_LENGTH = 200;
  if (field !== 'description') return text;
  return text.length > MAX_LENGTH ? `${text.slice(0, MAX_LENGTH)}...` : text;
};

const ColumnLinksTooltip: FC<{
  highlightTexts: string[];
  query: string;
  getColumnHref: (columnName: string) => string;
}> = ({ highlightTexts, query, getColumnHref }) => (
  <div className="max-w-md whitespace-pre-wrap">
    {highlightTexts.map((text, index) => {
      const columnName = text.replace(BOLD_TAG_REGEX, '');
      return (
        <div key={`${index}-${columnName}`}>
          <Link to={getColumnHref(columnName)} isInternal>
            <SanitizeBoldText text={text} query={query} />
          </Link>
        </div>
      );
    })}
  </div>
);

/**
 * Renders a "Matches: Column (10+), Tag (3), …" pill row. Each pill is a
 * sourdough Badge with a tooltip showing the matched snippets.
 *
 * Backends that surface only one matched field per hit can still use this by
 * passing a single-entry `highlights` object, e.g.
 * `{ column: ['<b>orders</b>_total'] }`.
 */
export const HighlightPills: FC<HighlightPillsProps> = ({
  highlights,
  query,
  getColumnHref,
}) => {
  const entries = (Object.entries(highlights) as [MatchedField, string[]][]).filter(
    ([field, texts]) => field !== 'name' && texts.length > 0,
  );

  if (entries.length === 0) return null;

  const getBadgeText = (field: MatchedField, texts: string[]) => {
    const count = texts.length >= 11 ? '10+' : texts.length.toString();
    return `${FIELD_LABEL[field] ?? toTitleCase(field)} (${count})`;
  };

  return (
    <div className="flex flex-wrap items-center gap-2 pt-2">
      <span className="text-xs text-fgDecorative">Matches:</span>
      {entries.map(([field, texts]) => (
        <Tooltip
          key={field}
          content={
            field === 'column' && getColumnHref ? (
              <ColumnLinksTooltip
                highlightTexts={texts}
                query={query}
                getColumnHref={getColumnHref}
              />
            ) : (
              <div className="max-w-md whitespace-pre-wrap">
                <SanitizeBoldText
                  text={truncateHighlight(field, texts.join('\n'))}
                  query={query}
                />
              </div>
            )
          }
          placement="bottom-start"
        >
          <Badge text={getBadgeText(field, texts)} variant="secondary" size="xs" />
        </Tooltip>
      ))}
    </div>
  );
};

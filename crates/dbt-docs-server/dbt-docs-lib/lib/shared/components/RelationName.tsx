import { FC, useCallback } from 'react';
import toast from 'react-hot-toast';
import { Copy } from 'lucide-react';
import { twJoin } from 'tailwind-merge';

import { Button } from '../../components/ui/Button';
import { Tooltip } from '../../components/ui/Tooltip';

export type RelationLike = {
  database?: string | null;
  schema?: string | null;
  alias?: string | null;
  identifier?: string | null;
};

export const buildRelationName = ({
  database,
  schema,
  alias,
  identifier,
}: RelationLike) => {
  const targetIdentifier = alias || identifier;
  if (database && schema && targetIdentifier) {
    return `${database}.${schema}.${targetIdentifier}`;
  }
  return null;
};

type RelationNameParams = {
  relation: RelationLike;
  copy?: boolean;
};

export const RelationName: FC<RelationNameParams> = ({ relation, copy = true }) => {
  const relationName = buildRelationName(relation);

  const onClick = useCallback(() => {
    if (!relationName) return null;

    navigator.clipboard.writeText(relationName);
    toast.success('Copied relation name to clipboard', {
      id: 'copy-relation-name',
    });
  }, [relationName]);

  if (!relationName) {
    return null;
  }

  return (
    <div
      className={twJoin(
        'flex w-full items-center gap-2 overflow-hidden whitespace-nowrap',
        copy && 'cursor-pointer',
      )}
      onClick={copy ? onClick : undefined}
    >
      <Tooltip displayOnlyWhenTruncated content={relationName}>
        {(ref) => (
          <span ref={ref} className="block truncate">
            {relationName}
          </span>
        )}
      </Tooltip>
      {copy && (
        <Button
          variant="ghost"
          icon={<Copy className="size-3" />}
          size="icon-sm"
          testId="copy-relation-name"
          tooltip="Copy to Clipboard"
        />
      )}
    </div>
  );
};

import { useCallback, useState } from 'react';
import { Box, Check, Copy, FileText, Workflow } from 'lucide-react';

import { Markdown } from '../components/Overview/Markdown';
import { Button } from '../components/ui/Button';
import { Card } from '../components/ui/Card';
import { Code } from '../components/ui/Code';
import { RESOURCE_TYPE_ICON, RESOURCE_TYPE_LABEL } from '../lib/resourceType';
import { useAssetCounts, useProjectOverview } from '../shared';

/**
 * The project overview — the docs landing page, at parity with dbt Docs v1
 * for authored content, plus a richer unconfigured default.
 *
 * Renders the winning `{% docs __overview__ %}` block when a package defines
 * one, and this dashboard otherwise. See `Overview.tsx`'s prior revision
 * (git history) for why the fallback used to be a bundled `overview.md`: the
 * same reasoning about surviving a missing/unreadable index still applies,
 * it just isn't the fallback's *content* anymore.
 */

const WHATS_HERE = [
  {
    icon: Box,
    title: 'Assets',
    description: 'Models, sources, tests, and more, grouped by type and package',
  },
  {
    icon: FileText,
    title: 'Files',
    description: "Mirrors your project's real directory structure",
  },
  {
    icon: Workflow,
    title: 'Lineage',
    description: "Use a resource's lineage to see immediate parents and children.",
  },
];

const TRY_RUNNING = [
  {
    command: 'dbt docs generate',
    description: 'Rebuilds the data these docs are served from',
  },
  {
    command: 'dbt docs serve',
    description: 'Starts this docs site locally',
  },
  {
    command: 'dbt debug',
    description: 'Checks your connection and profile setup.',
  },
];

const EXPLORE_TYPES = [
  'model',
  'source',
  'test',
  'seed',
  'snapshot',
  'analysis',
] as const;

function CommandPill({
  command,
  description,
}: {
  command: string;
  description: string;
}) {
  const [copied, setCopied] = useState(false);

  const handleCopy = useCallback(() => {
    navigator.clipboard.writeText(command);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  }, [command]);

  return (
    <Card className="w-fit">
      <div className="flex items-center gap-2 whitespace-nowrap">
        <span aria-hidden="true" className="text-fgDecorative">
          ↳
        </span>
        <Code className="bg-transparent px-0 py-0">{command}</Code>
        <Button
          variant="ghost"
          size="icon-xs"
          ariaLabel={copied ? 'Copied to clipboard' : `Copy ${command}`}
          icon={copied ? <Check className="size-3" /> : <Copy className="size-3" />}
          onClick={handleCopy}
        />
      </div>
      <p className="m-0 mt-2 max-w-[200px] text-sm text-fgDecorative">{description}</p>
    </Card>
  );
}

export default function Overview() {
  const { data: assetCounts } = useAssetCounts();
  const { data: overviewData, isPending, isError } = useProjectOverview();

  // An unreadable dbt.docs must not blank the landing page: the built-in
  // dashboard is a correct answer, not a degraded one.
  const authored =
    !isError && overviewData?.blockContents.trim() ? overviewData.blockContents : null;

  // Deliberately a spinner rather than the dashboard while pending —
  // flashing the built-in default and then swapping to the user's is worse
  // than a brief wait.
  if (isPending) return <div className="main-inner muted">Loading…</div>;

  // A project that authors its own overview doc block gets exactly that,
  // full stop — the dashboard below is only the unconfigured default.
  if (authored) {
    return (
      <div className="flex max-w-[768px] flex-col gap-3 px-8 pb-12 pt-6">
        <Markdown>{authored}</Markdown>
      </div>
    );
  }

  return (
    <div className="flex max-w-[900px] flex-col gap-8 px-8 pb-12 pt-6">
      <div className="flex flex-col gap-1">
        <h1 className="m-0 font-sansHeading text-3xl font-semibold text-fgMain">
          Welcome!
        </h1>
        <p className="m-0 font-sans text-base text-fgMain">
          Welcome to the auto-generated documentation for your dbt project!
        </p>
      </div>

      <div className="flex flex-col gap-3">
        <h2 className="m-0 font-sansHeading text-lg font-semibold text-fgMain">
          What&apos;s here
        </h2>
        <div className="flex flex-wrap gap-3">
          {WHATS_HERE.map(({ icon: Icon, title, description }) => (
            <Card key={title} className="w-[220px]">
              <div className="flex flex-col gap-3">
                <Icon className="size-4 text-fgDecorative" />
                <span className="font-sansHeading text-base font-semibold text-fgMain">
                  {title}
                </span>
                <p className="m-0 text-sm text-fgDecorative">{description}</p>
              </div>
            </Card>
          ))}
        </div>
      </div>

      <div className="flex flex-col gap-3">
        <h2 className="m-0 font-sansHeading text-lg font-semibold text-fgMain">
          Try running
        </h2>
        <div className="flex flex-wrap gap-3">
          {TRY_RUNNING.map(({ command, description }) => (
            <CommandPill key={command} command={command} description={description} />
          ))}
        </div>
      </div>

      <div className="flex flex-col gap-3">
        <h2 className="m-0 font-sansHeading text-lg font-semibold text-fgMain">
          Explore
        </h2>
        <div className="flex flex-wrap gap-3">
          {EXPLORE_TYPES.map((type) => {
            const Icon = RESOURCE_TYPE_ICON[type];
            return (
              <Card key={type} className="w-[200px]" isCompact>
                <div className="flex flex-col gap-1 px-2 py-1">
                  <span className="text-sm text-fgDecorative">
                    {RESOURCE_TYPE_LABEL[type]}
                  </span>
                  <div className="flex items-center gap-2">
                    <Icon className="size-4 text-fgDecorative" />
                    <span className="font-sansHeading text-base font-semibold text-fgMain">
                      {(assetCounts?.[type] ?? 0).toLocaleString()}
                    </span>
                  </div>
                </div>
              </Card>
            );
          })}
        </div>
      </div>

      <div className="flex flex-col gap-3">
        <h2 className="m-0 font-sansHeading text-lg font-semibold text-fgMain">
          More information
        </h2>
        <ul className="m-0 list-disc space-y-1 pl-5 font-sans text-base text-fgMain">
          <li>
            <a
              className="text-fgBrand hover:underline"
              href="https://docs.getdbt.com/docs/introduction"
              target="_blank"
              rel="noreferrer"
            >
              What is dbt
            </a>
            ?
          </li>
          <li>
            Read about{' '}
            <a
              className="text-fgBrand hover:underline"
              href="https://docs.getdbt.com/docs/build/view-documentation?version=2.0#dbt-docs-v2"
              target="_blank"
              rel="noreferrer"
            >
              dbt Docs v2
            </a>
          </li>
          <li>
            Join the{' '}
            <a
              className="text-fgBrand hover:underline"
              href="https://www.getdbt.com/community/"
              target="_blank"
              rel="noreferrer"
            >
              dbt Community
            </a>{' '}
            for questions and discussion
          </li>
        </ul>
      </div>
    </div>
  );
}

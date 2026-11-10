import { CopyCommandSnippet } from '../shared';
import { Code } from './ui/Code';

export function NoColumnMetadataFallback() {
  return (
    <div className="flex flex-col gap-2 p-6">
      <h4 className="m-0 text-base font-semibold leading-6 text-fgMain">
        No column metadata.
      </h4>
      <p className="m-0 flex flex-wrap items-center gap-1 text-sm leading-5 text-fgDecorative">
        <span>Add a</span>
        <Code>schemas.yml</Code>
        <span>
          entry, or run the command below to populate this from the warehouse.
        </span>
      </p>
      {/* `--write-index` is hidden and no longer what the site reads;
          `docs generate` writes the information schema itself. What is still
          needed is static analysis, for the inferred column types. */}
      <CopyCommandSnippet command="dbt build --static-analysis strict && dbt docs generate" />
    </div>
  );
}

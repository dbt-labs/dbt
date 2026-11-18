import { CopyCommandSnippet } from '../shared';

interface NoLineageFallbackProps {
  modelName: string;
}

export function NoLineageFallback({ modelName }: NoLineageFallbackProps) {
  const command = `dbt build --select +${modelName}+ && dbt docs generate`;
  return (
    <div className="flex flex-col gap-2 p-6">
      <h4 className="m-0 text-base font-semibold leading-6 text-fgMain">
        No lineage connections found.
      </h4>
      <p className="m-0 text-sm leading-5 text-fgDecorative">
        Don&apos;t see these connections? Run your project and refresh — or regenerate
        with:
      </p>
      <CopyCommandSnippet command={command} />
    </div>
  );
}

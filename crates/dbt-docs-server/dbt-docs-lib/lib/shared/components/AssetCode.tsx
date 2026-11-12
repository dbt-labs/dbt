import { CodePreview } from './CodePreview';
import { DetailsSection } from './SectionWithCard';

export type AssetCodeProps = {
  rawCode: string | null | undefined;
  compiledCode?: string | null;
};

export function AssetCode({ rawCode, compiledCode }: AssetCodeProps) {
  if (!rawCode) return null;
  return (
    <DetailsSection className="mt-4">
      <div className="mt-4 px-10 pb-6">
        <CodePreview source={rawCode} compiled={compiledCode || undefined} />
      </div>
    </DetailsSection>
  );
}

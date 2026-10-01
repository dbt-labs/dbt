// Small colored pill displaying the resource_type. Background color is
// driven by the per-type CSS classes in app.css. Used in the sidebar and
// node detail header.

interface Props {
  resourceType: string;
  size?: 'sm' | 'md';
}

export function NodeIcon({ resourceType, size = 'sm' }: Props) {
  return (
    <span
      className={`type-pill ${resourceType} ${size === 'sm' ? 'sm' : ''}`}
      title={resourceType}
    >
      {LABEL[resourceType] ?? resourceType}
    </span>
  );
}

// Compact display name. Most resource types are already short enough;
// these are the ones with snake_case names that look better collapsed.
const LABEL: Record<string, string> = {
  semantic_model: 'semantic',
  saved_query: 'saved query',
};

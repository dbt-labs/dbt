import { Button } from '../../components/ui/Button';
import { Link } from '../../components/ui/Link';

interface SelectorLinkProps {
  selector: string;
  label: string;
}

export function SelectorLink({ selector, label }: SelectorLinkProps) {
  return (
    <Link isInternal to={`?select=${encodeURIComponent(selector)}`}>
      <Button variant="outline" className="mt-6" text={label} />
    </Link>
  );
}

import { Button } from '../../components/ui/Button';
import { Link } from '../../components/ui/Link';

interface Props {
  selector: string;
  label: string;
}

export function SelectorLink({ selector, label }: Props) {
  return (
    <Link isInternal to={`?select=${encodeURIComponent(selector)}`}>
      <Button variant="outline" className="mt-6" text={label} />
    </Link>
  );
}

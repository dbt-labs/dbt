import type { ComponentProps } from 'react';
import { useParams } from 'react-router-dom';

import { NodeDetail } from '../components/NodeDetail';
import type { Asset } from '../shared';

type WrapperProps = Omit<ComponentProps<typeof NodeDetail>, 'asset'> & {
  detail: Asset | null;
  detailLoading: boolean;
  detailNotFound: boolean;
};

export default function ResourceDetails({
  detail,
  detailLoading,
  detailNotFound,
  ...rest
}: WrapperProps) {
  const { dbtUniqueId } = useParams<{ dbtUniqueId: string }>();
  if (detailLoading) return <div className="main-inner muted">Loading…</div>;
  if (detailNotFound) {
    return (
      <div className="main-inner muted">
        Detail view for <code>{dbtUniqueId}</code> isn&apos;t available yet — this
        resource type isn&apos;t served by the index API.
      </div>
    );
  }
  if (!detail) return <div className="main-inner muted">Loading…</div>;
  return <NodeDetail asset={detail} {...rest} />;
}

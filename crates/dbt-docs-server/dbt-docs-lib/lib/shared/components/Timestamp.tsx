import { Tooltip } from '../../components/ui/Tooltip';
import { formatAbsoluteLocalDate, formatDateCommon } from '../util/dateUtils';

interface TimestampDisplayProps {
  timestamp: string | undefined;
  timestampUtc: string | undefined;
  prependedText?: string;
}

/**
 * A component that displays a timestamp with a tooltip showing the UTC time.
 * Should be used in tandem with formatAbsoluteLocalDate & formatDateCommon.
 */
export const TimestampDisplay: React.FC<TimestampDisplayProps> = ({
  timestamp,
  timestampUtc,
  prependedText,
}) => (
  <>
    {timestamp && (
      <span className="text-sm">
        <Tooltip content={timestampUtc} className="ml-1">
          <span className="text-sm text-fgDecorative">
            {prependedText}
            {timestamp}
          </span>
        </Tooltip>
      </span>
    )}
  </>
);

interface TimestampContainerProps {
  date: Date | undefined;
}

/**
 * A wrapper around TimestampDisplay that takes a Date object and formats it for display.
 */
export const TimestampContainer: React.FC<TimestampContainerProps> = ({ date }) => {
  const timestamp = date && formatAbsoluteLocalDate(date);
  const timestampUtc = date && formatDateCommon(date).utc;
  return <TimestampDisplay timestamp={timestamp} timestampUtc={timestampUtc} />;
};

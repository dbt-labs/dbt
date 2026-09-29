import { createElement, useMemo } from 'react';
import {
  BadgeAlert,
  BadgeCheck,
  BadgeMinus,
  BadgeX,
  type LucideIcon,
} from 'lucide-react';

import { Tooltip } from '../../components/ui/Tooltip';
import { truthy } from '../util/array';
import { CollapsibleSection } from './CollapsibleSection';

export type SharedTestResult = {
  name: string;
  uniqueId: string;
  status: string | null;
};

enum TestResultStatus {
  unknown = 0,
  pass = 1,
  warn = 2,
  error = 3,
}

const testResultStatusIconMap: Record<number, LucideIcon> = {
  0: BadgeMinus,
  1: BadgeCheck,
  2: BadgeAlert,
  3: BadgeX,
};

// used to sort test results by severity
export const testStatusOrder: Record<string, number> = {
  error: 0,
  fail: 1,
  warn: 2,
  pass: 3,
  skipped: 4,
  reused: 5,
};

type TestResultsSectionProps = {
  resourceType: string;
  tests: SharedTestResult[] | undefined;
  toExpand?: React.ReactNode;
};

export const TestResultsSection = ({
  resourceType,
  tests,
  toExpand,
}: TestResultsSectionProps) => {
  const { sortedTests, testResultStatus, tooltipMessage } = useMemo(() => {
    if (!tests) {
      return {
        sortedTests: undefined,
        testResultStatus: TestResultStatus.unknown,
        tooltipMessage: `The status of this ${resourceType}'s tests could not be determined`,
      };
    }

    let warnCount = 0;
    let errorCount = 0;

    const sortedTests = tests.filter(truthy).map((test) => {
      const testStatus = test.status;
      if (testStatus === 'warn') {
        warnCount++;
      } else if (testStatus === 'error' || testStatus === 'fail') {
        errorCount++;
      }
      return test;
    });

    let testResultStatus = TestResultStatus.unknown;
    let tooltipMessage = `The status of this ${resourceType}'s tests could not be determined`;
    if (sortedTests.length > 0 && warnCount === 0 && errorCount === 0) {
      testResultStatus = TestResultStatus.pass;
      tooltipMessage = `This ${resourceType}'s tests are all passing`;
    } else if (errorCount > 0) {
      testResultStatus = TestResultStatus.error;
      tooltipMessage = `One or more tests of this ${resourceType} are failing`;
    } else if (sortedTests.length === 0) {
      testResultStatus = TestResultStatus.warn;
      tooltipMessage = `This ${resourceType} has no tests configured`;
    } else if (warnCount > 0) {
      testResultStatus = TestResultStatus.warn;
      tooltipMessage = `One or more tests of this ${resourceType} are warning`;
    }

    // sort by test status severity, then name alphabetically if tie
    sortedTests.sort((a, b) => {
      const aOrder = testStatusOrder[a.status ?? ''] ?? 99;
      const bOrder = testStatusOrder[b.status ?? ''] ?? 99;
      const statusComparison = aOrder - bOrder;
      if (statusComparison !== 0) {
        return statusComparison;
      }
      return a.name.localeCompare(b.name);
    });

    return { sortedTests, testResultStatus, tooltipMessage };
  }, [resourceType, tests]);

  return (
    <>
      {sortedTests && (
        <CollapsibleSection
          closeAltText={`Hide test result details`}
          expandAltText={`Show test result details`}
          toExpand={toExpand}
          disable={sortedTests.length === 0 || toExpand == null}
          shouldIndent={sortedTests.length > 0}
          className="-mb-[1px] mt-1 flex w-full overflow-hidden border-b border-borderMuted p-4 text-fgMain"
        >
          <span className="flex" data-testid={`test-result-status-${testResultStatus}`}>
            <span className="flex items-center">
              <Tooltip
                content={tooltipMessage}
                className="pointer-events-auto flex items-center"
              >
                {createElement(testResultStatusIconMap[testResultStatus], {
                  className: 'size-4 pointer-events-auto align-middle',
                })}
                <div className="sr-only">{tooltipMessage}</div>
              </Tooltip>
            </span>
            <span>
              <p className="font-label-lg mx-2 align-middle">Test results</p>
            </span>
          </span>
        </CollapsibleSection>
      )}
    </>
  );
};

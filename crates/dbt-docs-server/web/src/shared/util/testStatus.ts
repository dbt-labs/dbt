/** A test's raw last-run outcome, including `reused` (an execution detail,
 *  not a pass/fail outcome) -- distinct from {@link TestStatusValue}, which
 *  is the already-normalized display status with no `reused` case. Mirrors
 *  dbt-dag's `TestStatus`. */
export type TestStatus = 'pass' | 'error' | 'fail' | 'warn' | 'skipped' | 'reused';

/**
 * Returns the display status for a test, mapping REUSED to the actual test
 * outcome (PASS or FAIL). Test status is semantically pass/fail only — reuse
 * is an execution detail, not a test outcome.
 *
 * When lastRunStatus is 'reused' and lastKnownResult is available, we return
 * lastKnownResult (the correct prior outcome — may be warn/fail/error).
 * When lastRunStatus is 'reused' and no lastKnownResult is available (e.g.
 * the lineage API's TestLineageNode does not expose lastKnownResult), we fall
 * back to 'pass' as a best-effort: in practice the majority of reused tests
 * have passed, but callers with access to lastKnownResult should always supply
 * it to get the accurate outcome.
 */
export function getTestDisplayStatus(
  lastRunStatus: TestStatus | null | undefined,
  lastKnownResult?: TestStatus | null,
): TestStatus | null {
  if (!lastRunStatus) return null;
  if (lastRunStatus !== 'reused') return lastRunStatus;
  return lastKnownResult ?? 'pass';
}

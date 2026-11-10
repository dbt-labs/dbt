import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { Markdown } from './Markdown';

/**
 * These assertions look like styling tests but are correctness tests.
 *
 * Tailwind's preflight is on and there is no `@tailwindcss/typography` plugin, so
 * any element without an entry in the component map renders genuinely flat —
 * headings inherit body size and weight, lists lose their markers. dbt Core's own
 * default overview is written entirely in `###`/`####`, so a project migrating
 * from v1 is precisely the case that regresses when an entry goes missing.
 */
describe('<Markdown />', () => {
  it('renders nothing for blank input', () => {
    const { container } = render(<Markdown>{'   \n  '}</Markdown>);
    expect(container).toBeEmptyDOMElement();
  });

  it('styles headings at every level, not just h1 and h2', () => {
    render(<Markdown>{'# One\n\n## Two\n\n### Three\n\n#### Four'}</Markdown>);
    for (const [level, name] of [
      [1, 'One'],
      [2, 'Two'],
      [3, 'Three'],
      [4, 'Four'],
    ] as const) {
      const heading = screen.getByRole('heading', { level, name });
      // A heading with no size class is indistinguishable from a paragraph.
      expect(heading.className).toMatch(/text-(xs|sm|base|lg|xl|2xl|3xl)/);
      expect(heading.className).toContain('font-semibold');
    }
  });

  it('gives lists their markers back', () => {
    // preflight sets `list-style: none`, and a flex container suppresses markers
    // even when `list-disc` is present.
    const { container } = render(
      <Markdown>{'- one\n- two\n\n1. first\n2. second'}</Markdown>,
    );
    const ul = container.querySelector('ul')!;
    const ol = container.querySelector('ol')!;
    expect(ul.className).toContain('list-disc');
    expect(ul.className).not.toContain('flex');
    expect(ol.className).toContain('list-decimal');
  });

  it('renders a fenced block as pre > code', () => {
    const { container } = render(<Markdown>{'```sql\nselect 1\n```'}</Markdown>);
    const code = container.querySelector('pre > code');
    expect(code).not.toBeNull();
    expect(code!.textContent).toContain('select 1');
  });

  it('renders GFM tables', () => {
    const { container } = render(
      <Markdown>{'| a | b |\n|---|---|\n| 1 | 2 |'}</Markdown>,
    );
    expect(container.querySelector('table')).not.toBeNull();
    expect(screen.getByRole('columnheader', { name: 'a' })).toBeInTheDocument();
    expect(screen.getByRole('cell', { name: '1' })).toBeInTheDocument();
  });

  it('renders blockquotes as blockquotes', () => {
    const { container } = render(<Markdown>{'> quoted'}</Markdown>);
    expect(container.querySelector('blockquote')).not.toBeNull();
  });

  it('renders raw HTML rather than escaping it', () => {
    // A deliberate divergence from dbt Docs v1, which used `marked` at
    // `sanitize: true`. Pinned so that adding a sanitizer is a deliberate choice.
    const { container } = render(<Markdown>{'<em>raw</em>'}</Markdown>);
    expect(container.querySelector('em')?.textContent).toBe('raw');
  });

  it('opens external links in a new tab', () => {
    render(<Markdown>{'[dbt](https://docs.getdbt.com)'}</Markdown>);
    const link = screen.getByRole('link', { name: 'dbt' });
    expect(link).toHaveAttribute('target', '_blank');
    expect(link).toHaveAttribute('rel', expect.stringContaining('noreferrer'));
  });
});

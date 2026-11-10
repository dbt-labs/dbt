import type { ComponentPropsWithoutRef } from 'react';
import ReactMarkdown from 'react-markdown';
import rehypeExternalLinks from 'rehype-external-links';
import rehypeRaw from 'rehype-raw';
import remarkGfm from 'remark-gfm';

// Type scale pulled from Figma (Catalog file, node 19522:6538): Heading/1 =
// text-3xl/font-sansHeading/semibold, Heading/4 (section titles) =
// text-lg/font-sansHeading/semibold, Body/Base = text-base/font-sans/regular.
//
// Every element a project overview can produce needs an entry here. Tailwind's
// preflight is on (`app.css`) and there is no @tailwindcss/typography plugin in
// the preset chain, so anything left unmapped renders genuinely flat: headings
// inherit body size and weight, and `ol`/`ul` get `list-style: none`. A project
// authoring its own `{% docs __overview__ %}` block can reach any heading level,
// so the h3/h4 entries below stay load-bearing even though the unconfigured
// default (`Overview.tsx`) no longer renders through this component.
//
// `children` is destructured and passed explicitly rather than spread. It reads
// as noise, but jsx-a11y cannot see content arriving through a spread and the
// lint gate runs at --max-warnings 0.
//
// `DescriptionDisplay` and `SemanticAspectCard` duplicate this plugin stack with
// `prose` classes that are inert for the same missing-plugin reason. Converging
// the three is worth doing, but it would restyle every description on every
// detail page, so it is deliberately not part of this change.
const markdownComponents = {
  h1: ({ children, ...props }: ComponentPropsWithoutRef<'h1'>) => (
    <h1 className="m-0 font-sansHeading text-3xl font-semibold text-fgMain" {...props}>
      {children}
    </h1>
  ),
  h2: ({ children, ...props }: ComponentPropsWithoutRef<'h2'>) => (
    <h2
      className="m-0 mt-6 font-sansHeading text-lg font-semibold text-fgMain"
      {...props}
    >
      {children}
    </h2>
  ),
  h3: ({ children, ...props }: ComponentPropsWithoutRef<'h3'>) => (
    <h3
      className="m-0 mt-5 font-sansHeading text-base font-semibold text-fgMain"
      {...props}
    >
      {children}
    </h3>
  ),
  h4: ({ children, ...props }: ComponentPropsWithoutRef<'h4'>) => (
    <h4
      className="m-0 mt-4 font-sansHeading text-sm font-semibold text-fgMain"
      {...props}
    >
      {children}
    </h4>
  ),
  h5: ({ children, ...props }: ComponentPropsWithoutRef<'h5'>) => (
    <h5
      className="text-fgMuted m-0 mt-4 font-sansHeading text-sm font-semibold"
      {...props}
    >
      {children}
    </h5>
  ),
  h6: ({ children, ...props }: ComponentPropsWithoutRef<'h6'>) => (
    <h6
      className="text-fgMuted m-0 mt-4 font-sansHeading text-xs font-semibold"
      {...props}
    >
      {children}
    </h6>
  ),
  hr: () => <hr className="my-2 border-t border-borderMuted" />,
  p: (props: ComponentPropsWithoutRef<'p'>) => (
    <p className="m-0 font-sans text-base font-regular text-fgMain" {...props} />
  ),
  // `list-disc`/`list-decimal` are load-bearing: preflight strips markers. So is
  // the absence of `flex` — a flex container suppresses list markers entirely.
  ul: (props: ComponentPropsWithoutRef<'ul'>) => (
    <ul
      className="m-0 list-disc space-y-1 pl-5 font-sans text-base font-regular text-fgMain"
      {...props}
    />
  ),
  ol: (props: ComponentPropsWithoutRef<'ol'>) => (
    <ol
      className="m-0 list-decimal space-y-1 pl-5 font-sans text-base font-regular text-fgMain"
      {...props}
    />
  ),
  li: (props: ComponentPropsWithoutRef<'li'>) => <li className="pl-1" {...props} />,
  a: ({ children, ...props }: ComponentPropsWithoutRef<'a'>) => (
    <a className="text-fgBrand hover:underline" {...props}>
      {children}
    </a>
  ),
  blockquote: (props: ComponentPropsWithoutRef<'blockquote'>) => (
    <blockquote
      className="text-fgMuted m-0 border-l-2 border-borderMuted pl-4 font-sans text-base"
      {...props}
    />
  ),
  // react-markdown renders a fenced block as `pre > code`; the nested `code` must
  // not repeat the background, hence `bg-transparent p-0` on the inner element.
  pre: (props: ComponentPropsWithoutRef<'pre'>) => (
    <pre
      className="bg-bgMuted m-0 overflow-x-auto rounded p-3 font-mono text-sm text-fgMain [&>code]:bg-transparent [&>code]:p-0"
      {...props}
    />
  ),
  code: (props: ComponentPropsWithoutRef<'code'>) => (
    <code
      className="bg-bgMuted rounded px-1 py-0.5 font-mono text-sm text-fgMain"
      {...props}
    />
  ),
  table: (props: ComponentPropsWithoutRef<'table'>) => (
    <table
      className="m-0 w-full border-collapse font-sans text-sm text-fgMain"
      {...props}
    />
  ),
  th: (props: ComponentPropsWithoutRef<'th'>) => (
    <th
      className="border border-borderMuted px-3 py-1.5 text-left font-semibold"
      {...props}
    />
  ),
  td: (props: ComponentPropsWithoutRef<'td'>) => (
    <td className="border border-borderMuted px-3 py-1.5" {...props} />
  ),
  // `alt` defaults to empty so a decorative image is announced as such rather
  // than read out as a filename; an author's `![alt](src)` still wins.
  img: ({ alt = '', ...props }: ComponentPropsWithoutRef<'img'>) => (
    <img alt={alt} className="max-w-full" {...props} />
  ),
};

/**
 * Render project-authored markdown.
 *
 * `rehypeRaw` is a deliberate divergence from dbt Docs v1, which rendered with
 * `marked` at `sanitize: true` and so displayed raw HTML as escaped text. Keeping
 * it matches what this app already does for node descriptions, and the bundled
 * default relies on HTML comments being invisible rather than printed. The trust
 * boundary is the same either way: anyone who can author a `{% docs %}` block can
 * already run arbitrary Jinja at parse time.
 */
export function Markdown({ children }: { children: string }) {
  if (!children.trim()) return null;
  return (
    <ReactMarkdown
      components={markdownComponents}
      rehypePlugins={[
        rehypeRaw,
        [rehypeExternalLinks, { target: '_blank', rel: ['noreferrer'] }],
      ]}
      remarkPlugins={[remarkGfm]}
    >
      {children}
    </ReactMarkdown>
  );
}

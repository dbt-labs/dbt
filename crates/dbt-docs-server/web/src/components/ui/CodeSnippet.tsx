import { useEffect, useState } from 'react';
import { Check, Copy } from 'lucide-react';
import type { HighlighterCore, LanguageInput } from 'shiki/core';

import { cn } from '../../lib/utils';

export type CodeSnippetLanguage = 'sql' | 'python';

/** `import('shiki')` would bundle every grammar and theme into the binary. */
const LANGS: Record<CodeSnippetLanguage, LanguageInput> = {
  sql: () => import('shiki/langs/sql.mjs'),
  python: () => import('shiki/langs/python.mjs'),
};

const THEME_NAME = 'dbt-docs';

/** Shiki's own CSS-variable theme factory only distinguishes ~6 token
 *  buckets by default. Our design system's syntax-highlighting spec (Figma:
 *  Classes/Comments/Constants/Functions/Keywords/Numbers/Operators/
 *  Punctuations/Strings/Variables) calls out 10 — number, class, and
 *  operator get their own custom scope rules below so all 10 map to their
 *  real fgSyntax* tokens instead of being lumped into a neighboring bucket. */
const SYNTAX_STYLE = {
  '--shiki-foreground': 'var(--fgMain)',
  '--shiki-background': 'transparent',
  '--shiki-token-constant': 'var(--fgSyntaxConstants)',
  '--shiki-token-string': 'var(--fgSyntaxStrings)',
  '--shiki-token-comment': 'var(--fgSyntaxComments)',
  '--shiki-token-keyword': 'var(--fgSyntaxKeywords)',
  '--shiki-token-parameter': 'var(--fgSyntaxVariables)',
  '--shiki-token-function': 'var(--fgSyntaxFunctions)',
  '--shiki-token-string-expression': 'var(--fgSyntaxStrings)',
  '--shiki-token-punctuation': 'var(--fgSyntaxPunctuation)',
  '--shiki-token-link': 'var(--fgSyntaxStrings)',
  '--shiki-token-number': 'var(--fgSyntaxNumbers)',
  '--shiki-token-class': 'var(--fgSyntaxClasses)',
  '--shiki-token-operator': 'var(--fgSyntaxOperators)',
} as React.CSSProperties;

let highlighterPromise: Promise<HighlighterCore> | null = null;

function getHighlighter() {
  if (!highlighterPromise) {
    // Avoids the Oniguruma WASM chunk.
    highlighterPromise = Promise.all([
      import('shiki/core'),
      import('shiki/engine/javascript'),
    ]).then(
      ([
        { createHighlighterCore, createCssVariablesTheme },
        { createJavaScriptRegexEngine },
      ]) => {
        const theme = createCssVariablesTheme({ name: THEME_NAME });
        (theme.tokenColors ??= []).push(
          {
            scope: ['constant.numeric'],
            settings: { foreground: 'var(--shiki-token-number)' },
          },
          {
            scope: ['entity.name.class', 'entity.name.type', 'support.class'],
            settings: { foreground: 'var(--shiki-token-class)' },
          },
          {
            scope: ['keyword.operator'],
            settings: { foreground: 'var(--shiki-token-operator)' },
          },
        );
        return createHighlighterCore({
          themes: [theme],
          langs: Object.values(LANGS),
          engine: createJavaScriptRegexEngine({ forgiving: true }),
        });
      },
    );
  }
  return highlighterPromise;
}

export interface CodeSnippetProps {
  code: string;
  language?: CodeSnippetLanguage;
  includeCopyButton?: boolean;
  className?: string;
}

export function CodeSnippet({
  code,
  language,
  includeCopyButton,
  className,
}: CodeSnippetProps) {
  const [html, setHtml] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    let cancelled = false;
    if (!language) {
      setHtml(null);
      return;
    }
    getHighlighter().then((highlighter) => {
      if (cancelled) return;
      setHtml(highlighter.codeToHtml(code, { lang: language, theme: THEME_NAME }));
    });
    return () => {
      cancelled = true;
    };
  }, [code, language]);

  const handleCopy = () => {
    void navigator.clipboard.writeText(code);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  return (
    <div
      className={cn(
        'relative overflow-x-auto rounded-md border border-borderMain bg-bgMain p-3 text-sm [&_pre]:!bg-transparent',
        className,
      )}
      style={{
        ...SYNTAX_STYLE,
        fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
      }}
    >
      {includeCopyButton && (
        <button
          type="button"
          onClick={handleCopy}
          aria-label="Copy code"
          data-testid="code-snippet-copy-button"
          className="absolute right-2 top-2 rounded p-1 text-fgDecorative hover:bg-bgMainHover hover:text-fgMain"
        >
          {copied ? <Check className="size-3" /> : <Copy className="size-3" />}
        </button>
      )}
      {html ? (
        <div dangerouslySetInnerHTML={{ __html: html }} />
      ) : (
        <pre className="whitespace-pre-wrap text-fgMain">{code}</pre>
      )}
    </div>
  );
}

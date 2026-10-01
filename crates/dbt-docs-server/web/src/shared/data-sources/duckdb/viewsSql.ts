/**
 * The `views.sql` the information schema ships beside its parquet.
 *
 * The browser used to author its own `CREATE OR REPLACE VIEW` per artifact,
 * which made this app a second definition of the view surface — the same drift
 * problem fs#13788 fixed on the Rust side, across a language boundary. Instead
 * the site fetches the generated document and executes it, so the SQL the page
 * runs against is the SQL that was generated for the parquet sitting next to it.
 *
 * The document is parsed rather than executed wholesale, because registration
 * is per-artifact and on demand: executing it top to bottom would fetch all
 * ~38 files before the first query. Parsing keeps the laziness and still leaves
 * one definition — the statements are the generator's, not ours.
 */

/** A `CREATE OR REPLACE VIEW <ns>.<name> AS SELECT * FROM read_parquet('<file>')`. */
export interface BaseView {
  kind: 'base';
  /** Schema-qualified view name, e.g. `dbt.models`. */
  name: string;
  /** The artifact this view reads, e.g. `dbt.models.parquet`. */
  file: string;
  sql: string;
}

/** Any other `CREATE OR REPLACE VIEW` — one defined over other views. */
export interface DerivedView {
  kind: 'derived';
  name: string;
  /** Views this statement names, which must be registered before it runs. */
  dependsOn: string[];
  sql: string;
}

export type ViewDef = BaseView | DerivedView;

export interface ViewsDocument {
  /** `CREATE SCHEMA` statements, run once when the connection opens. */
  schemas: string[];
  /** Every view the document defines, keyed by schema-qualified name. */
  views: Map<string, ViewDef>;
}

/**
 * Split a `;`-separated DDL script into statements, dropping `--` comments.
 *
 * A port of `view_defs::split_ddl_statements`, including its one subtlety: `''`
 * is SQL's escaped quote inside a string literal, not two delimiters, so the
 * pair is consumed without toggling. Without that an escaped quote closes the
 * string early and exposes a later `--` as a real comment marker.
 */
export function splitStatements(ddl: string): string[] {
  const stripped = ddl
    .split('\n')
    .map((line) => {
      let inString = false;
      for (let i = 0; i < line.length; i += 1) {
        const ch = line[i];
        if (ch === "'") {
          if (line[i + 1] === "'") {
            i += 1;
            continue;
          }
          inString = !inString;
        } else if (ch === '-' && !inString && line[i + 1] === '-') {
          return line.slice(0, i);
        }
      }
      return line;
    })
    .join('\n');

  return stripped
    .split(';')
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

/** `CREATE OR REPLACE VIEW <name> AS …`, capturing the qualified name. */
const VIEW_NAME =
  /^CREATE\s+(?:OR\s+REPLACE\s+)?VIEW\s+([A-Za-z_][\w$]*\.[A-Za-z_][\w$]*)\s+AS\b/i;
/** A base view's whole body: nothing but a read of one parquet file. */
const BASE_BODY = /\bAS\s+SELECT\s+\*\s+FROM\s+read_parquet\(\s*'([^']+)'\s*\)\s*$/i;
/** `CREATE SCHEMA [IF NOT EXISTS] <name>`. */
const CREATE_SCHEMA = /^CREATE\s+SCHEMA\b/i;

/**
 * Parse the generated document into schema statements and view definitions.
 *
 * A statement that is neither a `CREATE SCHEMA` nor a `CREATE … VIEW` is a
 * shape this parser does not know, and is rejected rather than skipped: the
 * document is generated, so an unrecognized statement means the generator and
 * this parser have diverged, and silently dropping it would surface later as a
 * missing relation with no explanation.
 */
export function parseViewsSql(ddl: string): ViewsDocument {
  const schemas: string[] = [];
  const views = new Map<string, ViewDef>();
  const pending: { name: string; sql: string }[] = [];

  for (const sql of splitStatements(ddl)) {
    if (CREATE_SCHEMA.test(sql)) {
      schemas.push(sql);
      continue;
    }
    const named = VIEW_NAME.exec(sql);
    if (!named) {
      throw new Error(`views.sql: unrecognized statement: ${sql.slice(0, 120)}`);
    }
    const name = named[1]!;
    const base = BASE_BODY.exec(sql);
    if (base) {
      views.set(name, { kind: 'base', name, file: base[1]!, sql });
    } else {
      // Dependencies can only be resolved once every name is known, since a
      // derived view may be defined before one it reads.
      pending.push({ name, sql });
    }
  }

  // Resolve each derived view's dependencies by looking for the other views'
  // names in its body. Word-boundary matching on the qualified name, so
  // `dbt.run_results` does not read as a dependency of a statement that only
  // mentions `dbt.run_results_latest`.
  for (const { name, sql } of pending) {
    const dependsOn = [...views.keys(), ...pending.map((p) => p.name)].filter(
      (other) => other !== name && mentions(sql, other),
    );
    views.set(name, { kind: 'derived', name, dependsOn, sql });
  }

  return { schemas, views };
}

/**
 * Whether `sql` names the relation `name`, and not merely a longer name.
 *
 * Checked by scanning rather than with a lookbehind, which is well supported
 * but needlessly narrows the browsers this app runs in. The bounds matter both
 * ways: `dbt_rt.run_results` must not match inside `dbt_rt.run_results_latest`
 * (trailing `_`), and `dbt.models` must not match inside `xdbt.models`.
 */
function mentions(sql: string, name: string): boolean {
  const isNameChar = (ch: string | undefined) => ch !== undefined && /[\w$.]/.test(ch);
  let from = 0;
  for (;;) {
    const at = sql.indexOf(name, from);
    if (at === -1) return false;
    const before = at === 0 ? undefined : sql[at - 1];
    const after = sql[at + name.length];
    if (!isNameChar(before) && !isNameChar(after)) return true;
    from = at + 1;
  }
}

/**
 * Fetch and parse `views.sql` from the site's data directory.
 *
 * A missing or unparseable document is fatal, not degraded: without it the app
 * has no view definitions at all, and guessing them is exactly what this
 * replaced.
 */
export async function fetchViewsSql(dataBaseUrl: string): Promise<ViewsDocument> {
  const url = new URL('views.sql', dataBaseUrl).href;
  const res = await fetch(url);
  if (!res.ok) {
    throw new Error(
      `${res.status} ${res.statusText} fetching ${url} — the site's data directory ` +
        'has no views.sql, so it was not written by `dbt docs generate`',
    );
  }
  return parseViewsSql(await res.text());
}

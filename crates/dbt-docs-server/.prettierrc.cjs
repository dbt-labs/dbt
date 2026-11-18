// @ts-check

// Inlined from `@dbt-labs/prettier-config` (dbt-ui) so this project needs one
// fewer private package. Keep in sync with upstream if the shared config changes.

/** @type {import("prettier").Options} */
module.exports = {
  plugins: ['prettier-plugin-tailwindcss'],
  printWidth: 88,
  singleQuote: true,
  trailingComma: 'all',
  tailwindFunctions: ['cx'],
  // The plugin searches for a Tailwind config only in this file's directory and above,
  // so name web's explicitly. Both packages sort their classes by it.
  tailwindConfig: './web/tailwind.config.cjs',
};

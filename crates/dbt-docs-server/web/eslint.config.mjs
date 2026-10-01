import { defineConfig, globalIgnores } from 'eslint/config';
import jsxA11y from 'eslint-plugin-jsx-a11y';
import reactPlugin from 'eslint-plugin-react';
import hooksPlugin from 'eslint-plugin-react-hooks';
import storybook from 'eslint-plugin-storybook';
import tsParser from '@typescript-eslint/parser';
import prettier from 'eslint-plugin-prettier/recommended';
import simpleImportSort from 'eslint-plugin-simple-import-sort';
import tsEslint from 'typescript-eslint';

const baseConfig = defineConfig([
  tsEslint.configs.base,
  {
    languageOptions: {
      parser: tsParser,
    },
    rules: {
      // NOTE: Auto-fixable rules (using `eslint --fix`) should use the `warn`
      // level so they are less disruptive during development and don't show up in
      // the webpack error overlay.
      //
      'prettier/prettier': 'warn',
      'dot-notation': 'warn',
      'no-else-return': 'warn',
      'no-extra-bind': 'warn',
      'no-extra-boolean-cast': 'warn',
      'no-extra-label': 'warn',
      'no-floating-decimal': 'warn',
      'no-lonely-if': 'warn',
      'no-implicit-coercion': ['warn', { boolean: false }],
      'no-regex-spaces': 'warn',
      'no-unneeded-ternary': 'warn',
      'no-useless-computed-key': 'warn',
      'no-useless-rename': 'warn',
      'no-var': 'warn',
      'one-var': ['warn', 'never'],
      'operator-assignment': 'warn',
      'prefer-const': 'warn',
      'prefer-exponentiation-operator': 'warn',
      'prefer-object-spread': 'warn',
      'prefer-template': 'warn',
      'spaced-comment': ['warn', 'always', { markers: ['/'] }],
      yoda: 'warn',
      '@typescript-eslint/no-unused-vars': [
        'warn',
        {
          args: 'none',
          caughtErrors: 'none',
          argsIgnorePattern: '^_',
          varsIgnorePattern: '^_',
        },
      ],

      // Actual (non-autofixable) errors from here on
      //
      // e.g.: 'react/jsx-key': 'error',
      'no-console': 'warn',
    },
  },
  {
    // Adapted version of eslint-plugin-simple-import's custom rules:
    // https://github.com/lydell/eslint-plugin-simple-import-sort/blob/0e3719edbe52f11afe1859bde721e37ffe82dd79/examples/.eslintrc.js#L69-L93
    files: ['src/**/*.[jt]s?(x)'],
    plugins: {
      'simple-import-sort': simpleImportSort,
    },
    rules: {
      'simple-import-sort/exports': 'warn',
      'simple-import-sort/imports': [
        'error',
        {
          groups: [
            // Third party packages. `react` related packages come first.
            ['^react', '^@?\\w'],
            // dbt Labs external packages (sourdough, etc).
            ['^(@dbt-labs)(/.*|$)'],
            // Internal imports (alias followed by relative)
            [
              '^(@|@root|@components|@hooks|@util|@clients|@pages|@context|@common|@integrations)(/.*|$)',
              '^\\.',
            ],
            // Side effect imports.
            ['^\\u0000'],
            // Style imports.
            ['.*\\.css$'],
          ],
        },
      ],
    },
  },
  prettier,
  globalIgnores([
    // dependencies
    'node_modules/',
    'package-lock.json',
    'pnpm-lock.yaml',

    // testing
    'coverage/',
    '.nyc_output/',
    'cypress/screenshots/',
    'cypress/videos/',

    // artifacts
    '.next/',
    'out/',
    'build/',
    'dist/',
    'storybook-static/',

    // misc
    '.DS_Store',
    '*.pem',

    // logs
    '*.log',
    'npm-debug.log*',
    'yarn-debug.log*',
    'yarn-error.log*',
    '.pnpm-debug.log*',

    // local env files
    '.env.local',
    '.env.development.local',
    '.env.test.local',
    '.env.production.local',

    // turbo
    '.turbo/',
  ]),
]);

const reactConfig = defineConfig([
  ...baseConfig,
  ...storybook.configs['flat/recommended'],
  {
    plugins: {
      react: reactPlugin,
      'react-hooks': hooksPlugin,
      'jsx-a11y': jsxA11y,
    },
    languageOptions: {
      parserOptions: {
        ecmaFeatures: {
          jsx: true,
        },
      },
    },
    settings: {
      react: {
        version: 'detect',
      },
    },
    rules: {
      // Non-fixable rules
      'react/jsx-key': 'error',

      // Autofixable rules
      'react/forbid-foreign-prop-types': ['warn', { allowInPropTypes: true }],
      'react/jsx-no-comment-textnodes': 'warn',
      'react/jsx-no-duplicate-props': 'warn',
      'react/jsx-no-target-blank': 'warn',
      'react/jsx-no-undef': 'error',
      'react/jsx-pascal-case': ['warn', { allowAllCaps: true, ignore: [] }],
      'react/no-danger-with-children': 'warn',
      'react/no-direct-mutation-state': 'warn',
      'react/no-is-mounted': 'warn',
      'react/no-typos': 'error',
      'react/require-render-return': 'error',
      'react/style-prop-object': 'warn',

      // https://github.com/evcohen/eslint-plugin-jsx-a11y/tree/master/docs/rules
      'jsx-a11y/alt-text': 'warn',
      'jsx-a11y/anchor-has-content': 'warn',
      'jsx-a11y/anchor-is-valid': ['warn', { aspects: ['noHref', 'invalidHref'] }],
      'jsx-a11y/aria-activedescendant-has-tabindex': 'warn',
      'jsx-a11y/aria-props': 'warn',
      'jsx-a11y/aria-proptypes': 'warn',
      'jsx-a11y/aria-role': ['warn', { ignoreNonDOM: true }],
      'jsx-a11y/aria-unsupported-elements': 'warn',
      'jsx-a11y/heading-has-content': 'warn',
      'jsx-a11y/iframe-has-title': 'warn',
      'jsx-a11y/img-redundant-alt': 'warn',
      'jsx-a11y/no-access-key': 'warn',
      'jsx-a11y/no-distracting-elements': 'warn',
      'jsx-a11y/no-redundant-roles': 'warn',
      'jsx-a11y/role-has-required-aria-props': 'warn',
      'jsx-a11y/role-supports-aria-props': 'warn',
      'jsx-a11y/scope': 'warn',

      // https://github.com/facebook/react/tree/main/packages/eslint-plugin-react-hooks
      'react-hooks/rules-of-hooks': 'error',

      ...reactPlugin.configs['jsx-runtime'].rules,

      // React Compiler rules
      'react-hooks/config': 'error',
      'react-hooks/error-boundaries': 'error',
      'react-hooks/gating': 'error',
      'react-hooks/globals': 'error',
      'react-hooks/immutability': 'error',
      'react-hooks/preserve-manual-memoization': 'error',
      'react-hooks/set-state-in-render': 'error',
      'react-hooks/use-memo': 'error',
      'react-hooks/incompatible-library': 'warn',
      'react-hooks/purity': 'error',
      'react-hooks/unsupported-syntax': 'error',
      'react-hooks/refs': 'error',
      'react-hooks/static-components': 'error',
      'react-hooks/component-hook-factories': 'error',

      // This rule seems to be too unstable atm and known to be overly strict
      // will enable in the future if it stabilizes
      // https://github.com/facebook/react/issues?q=is%3Aissue%20state%3Aopen%20react-hooks%2Fset-state-in-effect
      'react-hooks/set-state-in-effect': 'off',
    },
  },
  {
    files: ['**/*.stories.*'],
    rules: {
      'import/no-anonymous-default-export': 'off',
    },
  },
  {
    // *.spec.* files are for cypress, not jest
    files: ['**/*.spec.*'],
    rules: {
      '@typescript-eslint/no-unused-expressions': 'off',
      'jest/valid-expect': 'off',
      'jest/valid-expect-in-promise': 'off',
      'testing-library/await-async-utils': 'off',
    },
  },
]);

export default [...baseConfig, ...reactConfig];

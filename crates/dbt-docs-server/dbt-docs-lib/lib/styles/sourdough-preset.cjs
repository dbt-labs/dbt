// @ts-check

const colors = require('tailwindcss/colors');
const defaultTheme = require('tailwindcss/defaultTheme');
const { tokens: bigaTokens } = require('./tokens.js');

const DBT_ORANGE = 'var(--logoBit)';

// Vendored copy of @dbt-labs/sourdough's tailwind.config.cjs (fontFamily,
// fontWeight, fontSize, the font-* utility-class plugin, color palette,
// box-shadows, min/max width, animations, and the forms plugin) -- see the
// OSS Design Capability plan. Re-copy from
// @dbt-labs/sourdough/tailwind.config if sourdough's config changes.
//
// No `content` scan of the live @dbt-labs/sourdough package: confirmed
// (2026-09-02) that every sourdough-only classname it used to pull in
// (bg-borderBrand, cursor-col-resize, ring-focusDanger, shadow-brand-hover,
// touch-none) has zero references anywhere in this app's own src/.

/** @type {import('tailwindcss').Config} */
module.exports = {
  content: [],
  darkMode: 'class', // or 'media' or 'class'
  theme: {
    screens: {
      sm: '800px',
      md: '960px',
      lg: '1800px',
    },
    extend: {
      fontFamily: {
        sans: ['Inter', ...defaultTheme.fontFamily.sans],
        sansHeading: ['Inter Tight', ...defaultTheme.fontFamily.sans],
        mono: ['IBM Plex Mono', ...defaultTheme.fontFamily.mono],
      },
      fontWeight: {
        regular: '400',
        medium: '500',
        semibold: '600',
        bold: '700',
      },
      fontSize: {
        xs: ['0.75rem', '1rem'],
        sm: ['0.8125rem', '1rem'],
        base: ['0.875rem', '1.25rem'],
        lg: ['1rem', '1.5rem'],
        xl: ['1.25rem', '1.75rem'],
        '2xl': ['1.5rem', '2rem'],
        '3xl': ['1.75rem', '2.5rem'],
        '4xl': ['2.25rem', '2.75rem'],
      },
      colors: {
        ...defaultTheme.colors,
        gray: {
          50: '#F9FAFB',
          100: '#F3F4F6',
          200: '#E5E7EB',
          300: '#D1D5DB',
          400: '#9CA3AF',
          500: '#6B7280',
          600: '#4B5563',
          700: '#374151',
          800: '#1F2937',
          900: '#111827',
          950: '#0F1623',
        },
        black: '#111827', // black override
        active: {
          50: '#F3FCFB',
          100: '#DDF8F6',
          200: '#ACF1ED',
          300: '#6BEAE4',
          400: '#19DCD5',
          500: '#12C9C6',
          600: '#0DB0B0',
          700: '#089396',
          800: '#047377',
          900: '#055657',
          950: '#043939',
        },
        accent: {
          50: '#fff3f1',
          100: '#ffe4df',
          200: '#ffcfc5',
          300: '#ffae9d',
          400: '#ff7f64',
          500: '#ff694a',
          600: '#ed3a15',
          700: '#c82d0d',
          800: '#a5290f',
          900: '#882814',
          950: '#4b1004',
        },
        teal: {
          50: '#F3FCFB',
          100: '#DDF8F6',
          200: '#ACF1ED',
          300: '#6BEAE4',
          400: '#19DCD5',
          500: '#12C9C6',
          600: '#0DB0B0',
          700: '#089396',
          800: '#047377',
          900: '#055657',
          950: '#043939',
        },
        red: {
          50: '#FEF2F2',
          100: '#FEE2E2',
          200: '#FECACA',
          300: '#FCA5A5',
          400: '#F87171',
          500: '#EF4444',
          600: '#DC2626',
          700: '#B91C1C',
          800: '#991B1B',
          900: '#7F1D1D',
          950: '#4F2229',
        },
        orange: {
          50: '#fff3f1',
          100: '#ffe4df',
          200: '#ffcfc5',
          300: '#ffae9d',
          400: '#ff7f64',
          500: '#ff694a',
          600: '#ed3a15',
          700: '#c82d0d',
          800: '#a5290f',
          900: '#882814',
          950: '#4b1004',
        },
        amber: {
          50: '#FFFBEB',
          100: '#FEF3C7',
          200: '#FDE68A',
          300: '#FCD34D',
          400: '#FBBF24',
          500: '#F59E0B',
          600: '#D97706',
          700: '#B45309',
          800: '#92400E',
          900: '#78350F',
          950: '#4B2F23',
        },
        emerald: {
          50: '#ECFDF5',
          100: '#D1FAE5',
          200: '#A7F3D0',
          300: '#6EE7B7',
          400: '#34D399',
          500: '#10B981',
          600: '#059669',
          700: '#047857',
          800: '#065F46',
          900: '#064E3B',
          950: '#123B38',
        },
        'light-blue': {
          50: '#F0F9FF,',
          100: '#E0F2FE',
          200: '#BAE6FD',
          300: '#7DD3FC',
          400: '#38BDF8',
          500: '#0EA5E9',
          600: '#0284C7',
          700: '#0369A1',
          800: '#075985',
          900: '#0C4A6E',
          950: '#082F49',
        },
        indigo: {
          50: '#EEF2FF',
          100: '#E0E7FF',
          200: '#C7D2FE',
          300: '#A5B4FC',
          400: '#647ACB',
          500: '#6366F1',
          600: '#4F46E5',
          700: '#4338CA',
          800: '#3730A3',
          900: '#312E81',
          950: '#1E1B4B',
        },
        // Lineage node colors
        'analysis-purple': {
          50: '#f9f5ff',
          100: '#f2e8ff',
          200: '#e7d6fe',
          300: '#d7bafd',
          400: '#ba86fa',
          500: '#a057f5',
          600: '#8a36e7',
          700: '#7525cb',
          800: '#6423a6',
          900: '#521e85',
          950: '#360863',
        },
        'column-gray': {
          50: '#f5f7f8',
          100: '#edeff2',
          200: '#dfe2e6',
          300: '#d1d5db',
          400: '#b5b9c4',
          500: '#a1a5b3',
          600: '#8b8fa0',
          700: '#777a8b',
          800: '#626471',
          900: '#52555d',
          950: '#303136',
        },
        'exposure-orange': {
          50: '#fef5f2',
          100: '#ffe8e1',
          200: '#ffd5c8',
          300: '#ffa588',
          400: '#fd8f6c',
          500: '#f56b3e',
          600: '#e24f20',
          700: '#be4017',
          800: '#9d3817',
          900: '#82331a',
          950: '#471708',
        },
        'macro-red': {
          50: 'fef2f3',
          100: '#fee2e5',
          200: '#FDBAC2',
          300: '#fba6b0',
          400: '#f77282',
          500: '#ed465a',
          600: '#da283d',
          700: '#b71e30',
          800: '#981c2b',
          900: '#7e1e29',
          950: '#440b12',
        },
        'metric-yellow': {
          50: '#fffbeb',
          100: '#fef3c7',
          200: '#FDE68A',
          300: '#fcd94d',
          400: '#fbd024',
          500: '#f5c60b',
          600: '#d9af06',
          700: '#b49209',
          800: '#92780e',
          900: '#78630f',
          950: '#453803',
        },
        'model-blue': {
          50: '#f0faff',
          100: '#e0f4fe',
          200: '#BAE6FD',
          300: '#7dd0fc',
          400: '#38b6f8',
          500: '#0e9ee9',
          600: '#0283c7',
          700: '#036ba1',
          800: '#075a85',
          900: '#0c4c6e',
          950: '#083349',
        },
        'project-violet': {
          50: '#eef2ff',
          100: '#e0e8ff',
          200: '#BAC9FD',
          300: '#a6b7fb',
          400: '#8290f7',
          500: '#656aef',
          600: '#4d48e3',
          700: '#413ac8',
          800: '#3531a2',
          900: '#302f80',
          950: '#1d1c4a',
        },
        'seed-green': {
          50: '#effeef',
          100: '#daffd9',
          200: '#BBFDBA',
          300: '#7df97c',
          400: '#3cec3c',
          500: '#12d513',
          600: '#08b108',
          700: '#0b8a0c',
          800: '#0e6d0f',
          900: '#0e5911',
          950: '#013203',
        },
        'semantic-pink': {
          50: '#fff1f2',
          100: '#ffe3e5',
          200: '#ffccd2',
          300: '#ff9eaa',
          400: '#fe6e83',
          500: '#f83b5b',
          600: '#e51946',
          700: '#c20e3a',
          800: '#a20f38',
          900: '#8b1036',
          950: '#4e0318',
        },
        'snapshot-fuchsia': {
          50: '#fef1fb',
          100: '#fde6f9',
          200: '#FDBAF2',
          300: '#fea3ec',
          400: '#fb6bdd',
          500: '#f53fca',
          600: '#e51dac',
          700: '#c80e8d',
          800: '#a50f74',
          900: '#891262',
          950: '#540339',
        },
        'source-teal': {
          50: '#f1faf9',
          100: '#daf3f2',
          200: '#A6E0E0',
          300: '#8ad5d6',
          400: '#53babd',
          500: '#389ea2',
          600: '#318189',
          700: '#2d6971',
          800: '#2c575e',
          900: '#294a50',
          950: '#173035',
        },
        'test-green': {
          50: '#ecfdf5',
          100: '#d1fae7',
          200: '#A7F3D0',
          300: '#6ee7af',
          400: '#34d38a',
          500: '#10b96b',
          600: '#059653',
          700: '#047843',
          800: '#065f36',
          900: '#064e2d',
          950: '#022c19',
        },
        'dbt-orange': DBT_ORANGE,
        current: 'currentColor',
        toggle: {
          'dark-ring': '#19dcd5',
          checked: {
            dark: '#089396',
            'dark-hover': '#0db0b0',
            disabled: '#f3f4f6',
            hover: '#055657',
          },
        },
        // Currently uses "dark mode" as the default
        globalnav: {
          background: colors.gray[900], // #111827
          border: colors.gray[700], // #374151
          item: colors.gray[200], // #e5e7eb
          'item-active': '#d9f6f4',
          'item-background-active': '#195050',
          'item-background-hover': colors.gray[700], // #374151
          'item-hover': colors.white,
          'item-tag': DBT_ORANGE,
          'item-tag-active': '#0cc',
          'menu-header-background': colors.gray[800], // #1f2937
        },
        ...bigaTokens,
      },
      minHeight: {
        input: '40px', // Our min height for readonly inputs
        textarea: '58px', // Min height for textareas
      },
      minWidth: {
        page: '800px', // Our mimimum supported width for general pages
        'wide-page': '1080px', // Our minimum supported width for settings pages
      },
      maxWidth: {
        page: '2080px', // Our maximum supported width for general pages
        'narrow-page': '1146px', // Our maximum supported width for settings pages
      },
      boxShadow: {
        top: '0 -1px 1px 0 var(--elevationBase), 0 -2px 2px 0 var(--elevationBase)',
        rest: '0 0 1px 0 var(--elevationBase), 0 1px 1px 0 var(--elevationBase)',
        hover:
          '0 0 1px 0 var(--elevationBase), 0 1px 2px 0 var(--elevationBase), 0 2px 4px 0 var(--elevationBase), 0 4px 8px 0 var(--elevationBase)',
        'brand-rest':
          '0 0 1px 0 var(--elevationBase), 0 1px 1px 0 var(--elevationBase)',
        'brand-hover':
          '0 0 1px 0 var(--elevationBase), 0 2px 2px 0 var(--elevationBase), 0 2px 4px 0 var(--elevationBase)',
        'success-rest':
          '0 0 1px 0 var(--elevationSuccess), 0 1px 1px 0 var(--elevationSuccess)',
        'success-hover':
          '0 0 1px 0 var(--elevationSuccess), 0 2px 2px 0 var(--elevationSuccess), 0 2px 4px 0 var(--elevationSuccess)',
        'warning-rest':
          '0 0 1px 0 var(--elevationWarning), 0 1px 1px 0 var(--elevationWarning)',
        'warning-hover':
          '0 0 1px 0 var(--elevationWarning), 0 2px 2px 0 var(--elevationWarning), 0 2px 4px 0 var(--elevationWarning)',
        'danger-rest':
          '0 0 1px 0 var(--elevationDanger), 0 1px 1px 0 var(--elevationDanger)',
        'danger-hover':
          '0 0 1px 0 var(--elevationDanger), 0 2px 2px 0 var(--elevationDanger), 0 2px 4px 0 var(--elevationDanger)',
        'info-rest': '0 0 1px 0 var(--elevationInfo), 0 1px 1px 0 var(--elevationInfo)',
        'info-hover':
          '0 0 1px 0 var(--elevationInfo), 0 2px 2px 0 var(--elevationInfo), 0 2px 4px 0 var(--elevationInfo)',
        'node-rest': '0 0 2px 0 var(--elevationNode), 0 2px 2px 0 var(--elevationNode)',
        'node-hover':
          '0 0 2px 0 var(--elevationNode), 0 2px 2px 0 var(--elevationNode), 0 4px 4px 0 var(--elevationNode), 0 8px 8px 0 var(--elevationNode)',
        'node-active':
          '0 2px 2px 0 var(--elevationNode), 0 6px 6px 0 var(--elevationNode), 0 8px 8px 0 var(--elevationNode), 0 12px 12px 0 var(--elevationNode)',
      },
      animation: {
        'slow-spin': 'spin 3s linear infinite',
        'medium-spin': 'spin 1.5s linear infinite',
        'fast-spin': 'spin .5s linear infinite',
        ping: 'ping 2s linear infinite',
        'ping-once': 'ping 2s ease-in-out 1',
      },
    },
  },
  variants: {
    extend: {
      backgroundColor: ['active', 'group-active'],
      color: ['active', 'group-active'],
    },
  },
  plugins: [
    require('@tailwindcss/forms'),
    function ({ addComponents, theme }) {
      addComponents({
        '.font-display': {
          fontSize: '2.25rem',
          lineHeight: '2.75rem',
          fontWeight: '500',
          color: 'var(--fgMain)',
          fontFamily: theme('fontFamily.sansHeading'),
        },
        '.font-heading-1': {
          fontSize: '1.75rem',
          lineHeight: '2rem',
          fontWeight: '500',
          color: 'var(--fgMain)',
          fontFamily: theme('fontFamily.sansHeading'),
        },
        '.font-heading-2': {
          fontSize: '1.5rem',
          lineHeight: '1.75rem',
          fontWeight: '500',
          color: 'var(--fgMain)',
          fontFamily: theme('fontFamily.sansHeading'),
        },
        '.font-heading-3': {
          fontSize: '1.25rem',
          lineHeight: '1.5rem',
          fontWeight: '500',
          color: 'var(--fgMain)',
          fontFamily: theme('fontFamily.sansHeading'),
        },
        '.font-heading-4': {
          fontSize: '1rem',
          lineHeight: '1.5rem',
          fontWeight: '500',
          color: 'var(--fgMain)',
          fontFamily: theme('fontFamily.sansHeading'),
        },
        '.font-heading-5': {
          fontSize: '0.875rem',
          lineHeight: '1.25rem',
          fontWeight: '500',
          color: 'var(--fgMain)',
          fontFamily: theme('fontFamily.sansHeading'),
        },
        '.font-heading-6': {
          fontSize: '0.8125rem',
          lineHeight: '1rem',
          fontWeight: '500',
          color: 'var(--fgMain)',
          fontFamily: theme('fontFamily.sansHeading'),
        },
        '.font-body': {
          fontSize: '0.875rem',
          lineHeight: '1.25rem',
          fontFamily: theme('fontFamily.sans'),
        },
        '.font-body-sm': {
          fontSize: '0.8125rem',
          lineHeight: '1.25rem',
          fontFamily: theme('fontFamily.sans'),
        },
        '.font-body-lg': {
          fontSize: '1rem',
          lineHeight: '1.5rem',
          fontFamily: theme('fontFamily.sans'),
        },
        '.font-caption': {
          fontSize: '0.75rem',
          lineHeight: '1rem',
          fontFamily: theme('fontFamily.sans'),
          color: 'var(--fgDecorative)',
        },
        '.font-label': {
          fontSize: '0.8125rem',
          lineHeight: '1rem',
          fontWeight: '500',
          fontFamily: theme('fontFamily.sans'),
        },
        '.font-label-lg': {
          fontSize: '0.875rem',
          lineHeight: '1.25rem',
          fontWeight: '500',
          fontFamily: theme('fontFamily.sans'),
        },
        '.font-code': {
          fontSize: '0.8125rem',
          lineHeight: '1.5',
          fontFamily: theme('fontFamily.mono'),
        },
      });
    },
  ],
};

import js from '@eslint/js'
import globals from 'globals'
import sonarjs from 'eslint-plugin-sonarjs'
import tseslint from 'typescript-eslint'

export default tseslint.config(
  { ignores: ['dist/**', 'node_modules/**', 'out/**', 'release/**', 'bundle/**'] },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    languageOptions: { globals: globals.node },
    rules: {
      'no-eval': 'error',
      'no-implied-eval': 'error',
      'no-new-func': 'error',
      '@typescript-eslint/no-explicit-any': 'error',
      '@typescript-eslint/ban-ts-comment': ['error', { 'ts-ignore': true, 'ts-nocheck': true, 'ts-expect-error': true }],
      '@typescript-eslint/no-unused-vars': ['error', { argsIgnorePattern: '^_', caughtErrors: 'none' }],
    },
  },
  // The same size and complexity limits as the server (AGENTS.md): production files
  // up to 400 lines, tests up to 800, functions up to 80 lines (the raw span, blanks and
  // comments count) and cognitive complexity 15. Refactor instead of raising a limit.
  {
    files: ['src/**/*.ts', 'assets/**/*.js', 'scripts/**/*.ts'],
    plugins: { sonarjs },
    rules: {
      'max-lines': ['error', { max: 400, skipBlankLines: false, skipComments: false }],
      'max-lines-per-function': ['error', { max: 80, skipBlankLines: false, skipComments: false, IIFEs: true }],
      'sonarjs/cognitive-complexity': ['error', 15],
    },
  },
  {
    files: ['test/**/*.ts'],
    plugins: { sonarjs },
    rules: {
      'max-lines': ['error', { max: 800, skipBlankLines: false, skipComments: false }],
      'max-lines-per-function': ['error', { max: 80, skipBlankLines: false, skipComments: false, IIFEs: true }],
      'sonarjs/cognitive-complexity': ['error', 15],
    },
  },
  // The page-facing scripts run in a browser.
  { files: ['assets/**/*.js'], languageOptions: { globals: globals.browser, sourceType: 'script' } },
)

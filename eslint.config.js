// ESLint flat config (ESLint 9+). Lints the TypeScript/React frontend; the Rust
// shell is handled by rustfmt + clippy. `eslint-config-prettier` is last so it
// disables any stylistic rules that would fight Prettier.
import js from "@eslint/js";
import globals from "globals";
import tseslint from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";
import reactRefresh from "eslint-plugin-react-refresh";
import prettier from "eslint-config-prettier";

export default tseslint.config(
  {
    ignores: ["dist", "src-tauri", "node_modules", "docs/internal"],
  },
  {
    files: ["src/**/*.{ts,tsx}"],
    extends: [js.configs.recommended, ...tseslint.configs.recommended],
    languageOptions: {
      ecmaVersion: 2020,
      globals: globals.browser,
    },
    plugins: {
      "react-hooks": reactHooks,
      "react-refresh": reactRefresh,
    },
    rules: {
      ...reactHooks.configs.recommended.rules,
      "react-refresh/only-export-components": ["warn", { allowConstantExport: true }],
      // CLAUDE.md mandate: TypeScript strict, no `any`.
      "@typescript-eslint/no-explicit-any": "error",
    },
  },
  {
    // Build tooling at the repo root runs under Node, not the browser.
    files: ["*.{ts,js}"],
    extends: [js.configs.recommended, ...tseslint.configs.recommended],
    languageOptions: {
      globals: globals.node,
    },
  },
  prettier,
);

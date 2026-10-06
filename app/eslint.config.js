// Minimal ESLint configuration (S1-07). It catches real defects (unused code,
// hook-rule violations, unsafe TypeScript patterns) without enforcing a style:
// formatting is not linted here.
import js from "@eslint/js";
import reactHooks from "eslint-plugin-react-hooks";
import globals from "globals";
import tseslint from "typescript-eslint";

export default tseslint.config(
  // Generated/third-party output, never linted. `src/components/ui/` holds
  // shadcn/ui primitives added by its CLI (see app/README.md).
  { ignores: ["dist/", "src-tauri/", "test-results/", "playwright-report/", "src/components/ui/"] },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    files: ["src/**/*.{ts,tsx}", "e2e/**/*.ts"],
    languageOptions: { globals: globals.browser },
    plugins: { "react-hooks": reactHooks },
    rules: {
      // Only the two classic hook rules; the plugin's React Compiler rules
      // are not adopted (the app does not use the compiler).
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "error",
    },
  },
  {
    files: ["e2e-native/**/*.ts", "*.config.{ts,js}"],
    languageOptions: { globals: globals.node },
  },
);

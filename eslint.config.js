import eslint from "@eslint/js";
import vue from "eslint-plugin-vue";
import tseslint from "typescript-eslint";
import vueParser from "vue-eslint-parser";

export default tseslint.config(
  {
    ignores: [
      "dist/**",
      "coverage/**",
      "target/**",
      "src-tauri/target/**",
      "src/core-api/generated/**",
      "src-tauri/gen/**",
      "vendor/tauri-plugin-updater/**",
    ],
  },
  eslint.configs.recommended,
  ...tseslint.configs.recommended,
  ...vue.configs["flat/recommended"],
  {
    files: ["**/*.ts", "**/*.vue"],
    rules: {
      "no-undef": "off",
    },
  },
  {
    files: ["**/*.vue"],
    languageOptions: {
      parser: vueParser,
      parserOptions: {
        parser: tseslint.parser,
        ecmaVersion: "latest",
        sourceType: "module",
      },
    },
    rules: {
      "vue/multi-word-component-names": "off",
    },
  },
  {
    files: ["**/*.test.ts"],
    rules: {
      // Test fixtures intentionally colocate several tiny stub components so
      // one behavior scenario remains self-contained.
      "vue/one-component-per-file": "off",
    },
  },
);

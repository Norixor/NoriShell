import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    environment: "node",
    include: ["examples/theme-plugins/theme-plugins.test.ts"],
  },
});

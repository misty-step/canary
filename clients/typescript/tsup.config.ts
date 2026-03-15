import { defineConfig } from "tsup";

export default defineConfig({
  entry: ["src/index.ts", "src/nextjs.ts"],
  format: ["esm", "cjs"],
  dts: true,
  clean: true,
  splitting: false,
});

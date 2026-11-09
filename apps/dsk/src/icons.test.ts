import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";

test("only ui/Icon.tsx imports the icon library, so swapping an icon is one line", () => {
  const src = new URL(".", import.meta.url).pathname;
  const importers = readdirSync(src, { recursive: true, encoding: "utf8" }).filter(
    (file) =>
      /\.tsx?$/.test(file) &&
      !file.endsWith(".test.ts") &&
      readFileSync(join(src, file), "utf8").includes('from "lucide-react"'),
  );
  assert.deepEqual(importers, [join("ui", "Icon.tsx")]);
});

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { parsePresentation, validateScene } from "../src/protocol.ts";

const fixture = (name: string) =>
  JSON.parse(
    readFileSync(
      new URL(
        `../../../crates/intersect-views/tests/fixtures/${name}`,
        import.meta.url,
      ),
      "utf8",
    ),
  );
test("shared Rust scene fixture is accepted", () =>
  validateScene(fixture("scene-v1.json")));
for (const rejection of fixture("scene-v1-rejections.json")) {
  test(rejection.name, () => {
    const scene = fixture("scene-v1.json");
    const path = rejection.pointer.slice(1).split("/");
    const key = path.pop()!;
    let target = scene;
    for (const part of path) target = target[part];
    target[key] = rejection.value;
    assert.throws(() => validateScene(scene));
  });
}
test("presentation round trips border ports without semantic fields", () => {
  const value = {
    schemaVersion: 1,
    views: {
      view: {
        positions: { port: { x: 0, y: 45, width: 12, height: 12 } },
        viewport: { x: 5, y: 10, zoom: 0.7 },
      },
    },
  };
  assert.deepEqual(parsePresentation(JSON.stringify(value)), value);
  assert.throws(() =>
    parsePresentation(JSON.stringify({ ...value, schemaVersion: 2 })),
  );
});

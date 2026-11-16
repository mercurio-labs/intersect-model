/** Structural checks only, shared fixtures with the Rust wire contract. */
import type { Scene, SceneResponse } from "./protocol.ts";
type ObjectValue = Record<string, unknown>;
function object(
  value: unknown,
  required: string[],
  optional: string[] = [],
): ObjectValue {
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw new Error("Expected a contract object");
  const result = value as ObjectValue,
    allowed = new Set([...required, ...optional]);
  if (
    required.some((key) => !Object.hasOwn(result, key)) ||
    Object.keys(result).some((key) => !allowed.has(key))
  )
    throw new Error("Missing or unknown contract field");
  return result;
}
function text(value: unknown, nonempty = false): asserts value is string {
  if (typeof value !== "string" || (nonempty && !value))
    throw new Error("Expected a contract string");
}
function list(value: unknown): unknown[] {
  if (!Array.isArray(value)) throw new Error("Expected a contract array");
  return value;
}
function integer(value: unknown): asserts value is number {
  if (!Number.isSafeInteger(value) || (value as number) < 0)
    throw new Error("Invalid portable JSON integer");
}
function binding(value: unknown) {
  const b = object(value, [
    "key",
    "elementId",
    "context",
    "uri",
    "range",
    "readOnly",
  ]);
  for (const key of ["key", "elementId", "uri"]) text(b[key], true);
  for (const context of list(b.context)) text(context, true);
  if (typeof b.readOnly !== "boolean")
    throw new Error("Invalid read-only flag");
  const range = object(b.range, ["start", "end"]),
    start = object(range.start, ["line", "character"]),
    end = object(range.end, ["line", "character"]);
  for (const p of [start, end]) {
    integer(p.line);
    integer(p.character);
  }
  if (
    (end.line as number) < (start.line as number) ||
    (end.line === start.line &&
      (end.character as number) < (start.character as number))
  )
    throw new Error("Invalid UTF-16 source range");
}
function field(value: unknown) {
  const f = object(value, ["label", "binding"]);
  text(f.label);
  binding(f.binding);
}
export function validateScene(value: unknown): asserts value is Scene {
  const scene = object(
    value,
    [
      "schemaVersion",
      "revision",
      "view",
      "nodes",
      "edges",
      "palette",
      "problems",
    ],
    ["layout"],
  );
  if (scene.schemaVersion !== 1) throw new Error("Unsupported scene version");
  integer(scene.revision);
  if (scene.layout !== undefined) {
    const layout = object(scene.layout, ["direction"]);
    if (!["RIGHT", "LEFT", "UP", "DOWN"].includes(layout.direction as string))
      throw new Error("Unsupported layout direction");
  }
  const view = object(scene.view, [
    "key",
    "name",
    "kind",
    "binding",
    "owners",
    "details",
    "problems",
  ]);
  for (const key of ["key", "name", "kind"]) text(view[key], true);
  binding(view.binding);
  const owners = new Set<string>();
  for (const item of list(view.owners)) {
    const owner = object(item, ["key", "label"]);
    text(owner.key, true);
    text(owner.label, true);
    if (owners.has(owner.key)) throw new Error("Duplicate view owner");
    owners.add(owner.key);
  }
  for (const detail of list(view.details)) field(detail);
  for (const problem of [...list(view.problems), ...list(scene.problems)])
    text(problem);
  const palette = object(scene.palette, [
    "schemaVersion",
    "id",
    "label",
    "groups",
  ]);
  if (palette.schemaVersion !== 1)
    throw new Error("Unsupported palette version");
  text(palette.id, true);
  text(palette.label, true);
  const groups = list(palette.groups),
    paletteIds = new Set<string>();
  let tools = 0;
  const addPalette = (id: unknown) => {
    text(id, true);
    if (paletteIds.has(id)) throw new Error("Duplicate palette identity");
    paletteIds.add(id);
  };
  if (groups.length > 32) throw new Error("Palette group budget exceeded");
  for (const item of groups) {
    const group = object(item, ["id", "label", "entries"]);
    addPalette(group.id);
    text(group.label, true);
    for (const entry of list(group.entries)) {
      if (++tools > 256) throw new Error("Palette tool budget exceeded");
      const tool = object(
        entry,
        ["id", "label", "icon", "gesture", "command"],
        ["parameters", "inputs"],
      );
      addPalette(tool.id);
      text(tool.label, true);
      text(tool.icon);
      text(tool.command, true);
      if (!["select", "place", "action"].includes(tool.gesture as string))
        throw new Error("Unsupported palette gesture");
      if (
        tool.parameters !== undefined &&
        (!tool.parameters ||
          typeof tool.parameters !== "object" ||
          Array.isArray(tool.parameters))
      )
        throw new Error("Invalid tool parameters");
      const inputs = tool.inputs === undefined ? [] : list(tool.inputs),
        ids = new Set<string>();
      if (inputs.length > 16) throw new Error("Palette input budget exceeded");
      for (const item of inputs) {
        const input = object(item, ["id", "label"], ["default"]);
        text(input.id, true);
        text(input.label, true);
        if (input.default !== undefined) text(input.default);
        if (ids.has(input.id)) throw new Error("Duplicate tool input");
        ids.add(input.id);
      }
    }
  }
  const ids = new Set<string>(),
    endpoints = new Set<string>();
  let nodes = 0,
    ports = 0,
    rows = 0;
  const add = (id: unknown, endpoint: boolean) => {
    text(id, true);
    if (ids.has(id)) throw new Error("Duplicate scene identity");
    ids.add(id);
    if (endpoint) endpoints.add(id);
  };
  function node(value: unknown, depth: number) {
    if (++nodes > 500 || depth > 16)
      throw new Error("Scene node or nesting budget exceeded");
    const n = object(value, [
      "id",
      "notation",
      "title",
      "subtitle",
      "binding",
      "compartments",
      "children",
      "ports",
    ]);
    add(n.id, true);
    binding(n.binding);
    for (const key of ["notation", "title", "subtitle"]) text(n[key]);
    for (const item of list(n.ports)) {
      if (++ports > 1000) throw new Error("Scene port budget exceeded");
      const p = object(item, ["id", "label", "side", "binding"]);
      add(p.id, true);
      text(p.label);
      binding(p.binding);
      if (!["NORTH", "SOUTH", "WEST", "EAST"].includes(p.side as string))
        throw new Error("Unsupported port side");
    }
    for (const item of list(n.compartments)) {
      const c = object(item, ["label", "rows"]);
      text(c.label);
      for (const row of list(c.rows)) {
        if (++rows > 5000) throw new Error("Scene row budget exceeded");
        field(row);
      }
    }
    for (const child of list(n.children)) node(child, depth + 1);
  }
  for (const root of list(scene.nodes)) node(root, 0);
  const edges = list(scene.edges);
  if (edges.length > 2000) throw new Error("Scene edge budget exceeded");
  for (const item of edges) {
    const edge = object(item, [
      "id",
      "notation",
      "label",
      "source",
      "target",
      "binding",
    ]);
    add(edge.id, false);
    text(edge.notation);
    text(edge.label);
    binding(edge.binding);
    if (
      !endpoints.has(edge.source as string) ||
      !endpoints.has(edge.target as string)
    )
      throw new Error("Unbound scene edge");
  }
}
export function validateSceneResponse(
  value: unknown,
): asserts value is SceneResponse {
  const response = object(value, ["scene", "capabilities"]);
  validateScene(response.scene);
  const tools = new Set(
      response.scene.palette.groups.flatMap((g) => g.entries.map((t) => t.id)),
    ),
    seen = new Set<string>();
  for (const item of list(response.capabilities)) {
    const capability = object(item, ["toolId", "enabled", "reason"]);
    text(capability.toolId, true);
    text(capability.reason);
    if (
      typeof capability.enabled !== "boolean" ||
      (!capability.enabled && !capability.reason) ||
      !tools.has(capability.toolId) ||
      seen.has(capability.toolId)
    )
      throw new Error("Invalid tool capability");
    seen.add(capability.toolId);
  }
  if (tools.size !== seen.size) throw new Error("Missing tool capabilities");
}

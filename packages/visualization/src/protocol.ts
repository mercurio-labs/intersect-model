/** Intersect visualization client. package.
 * Language-neutral protocol mirrored from intersect-views. No SysML imports. */
export interface Position {
  line: number;
  character: number;
}
export interface Range {
  start: Position;
  end: Position;
}
export interface Binding {
  key: string;
  elementId: string;
  context: string[];
  uri: string;
  range: Range;
  readOnly: boolean;
}
export interface Owner {
  key: string;
  label: string;
}
export interface View {
  key: string;
  name: string;
  kind: string;
  binding: Binding;
  owners: Owner[];
  details: Field[];
  problems: string[];
}
export interface Field {
  label: string;
  binding: Binding;
}
export interface Compartment {
  label: string;
  rows: Field[];
}
export interface Port {
  id: string;
  label: string;
  side: string;
  binding: Binding;
}
export interface Node {
  id: string;
  notation: string;
  title: string;
  subtitle: string;
  binding: Binding;
  compartments: Compartment[];
  children: Node[];
  ports: Port[];
}
export interface Edge {
  id: string;
  notation: string;
  label: string;
  source: string;
  target: string;
  binding: Binding;
}
export interface Tool {
  id: string;
  label: string;
  icon: string;
  gesture: "select" | "place" | "action";
  command: string;
  parameters: Record<string, unknown>;
  inputs: { id: string; label: string; default: string }[];
}
export interface Palette {
  schemaVersion: 1;
  id: string;
  label: string;
  groups: { id: string; label: string; entries: Tool[] }[];
}
export interface Scene {
  schemaVersion: 1;
  revision: number;
  layout?: { direction: "RIGHT" | "LEFT" | "UP" | "DOWN" };
  view: View;
  nodes: Node[];
  edges: Edge[];
  palette: Palette;
  problems: string[];
}
export interface SceneResponse {
  scene: Scene;
  capabilities: { toolId: string; enabled: boolean; reason: string }[];
}
export interface SourcePatch {
  uri: string;
  range: Range;
  original: string;
  replacement: string;
  beforeHash: string;
  afterHash: string;
}
export interface PreparedEdit {
  schemaVersion: 1;
  token: string;
  baseRevision: number;
  nextRevision: number;
  patches: SourcePatch[];
  created: string;
}
export interface Point {
  x: number;
  y: number;
}
export interface Geometry extends Point {
  width: number;
  height: number;
}
export interface Presentation {
  schemaVersion: 1;
  views: Record<
    string,
    {
      positions: Record<string, Geometry>;
      viewport: { x: number; y: number; zoom: number };
    }
  >;
}
export interface Layout {
  revision: number;
  viewKey: string;
  nodes: Record<string, Geometry>;
  ports: Record<string, Point>;
  edges: Record<string, Point[]>;
}

/** Rendering/style definitions are independent from palette definitions. */
export interface NotationProfile {
  schemaVersion: 1;
  id: string;
  shapes: Record<
    string,
    {
      shape: "rectangle" | "rounded";
      fill: string;
      stroke: string;
      radius?: number;
    }
  >;
}
export const defaultNotation: NotationProfile = {
  schemaVersion: 1,
  id: "intersect.standard-structure",
  shapes: {
    definition: { shape: "rectangle", fill: "definition", stroke: "neutral" },
    usage: { shape: "rectangle", fill: "usage", stroke: "neutral" },
    structure: {
      shape: "rounded",
      fill: "usage",
      stroke: "neutral",
      radius: 5,
    },
    package: { shape: "rectangle", fill: "package", stroke: "neutral" },
  },
};

export function parsePresentation(text: string): Presentation {
  const p = JSON.parse(text) as Presentation;
  if (
    !p ||
    p.schemaVersion !== 1 ||
    !p.views ||
    typeof p.views !== "object" ||
    Array.isArray(p.views) ||
    Object.keys(p.views).length > 128
  )
    throw new Error("Unsupported presentation document.");
  for (const view of Object.values(p.views)) {
    if (
      !view ||
      !view.positions ||
      !view.viewport ||
      typeof view.positions !== "object" ||
      Array.isArray(view.positions) ||
      typeof view.viewport !== "object" ||
      Array.isArray(view.viewport) ||
      Object.keys(view.positions).length > 5000
    )
      throw new Error("Invalid saved presentation.");
    for (const g of Object.values(view.positions))
      if (
        !g ||
        ![g.x, g.y, g.width, g.height].every(Number.isFinite) ||
        g.width < 1 ||
        g.height < 1 ||
        [g.x, g.y].some((n) => Math.abs(n) > 1_000_000)
      )
        throw new Error("Invalid saved diagram position.");
    if (
      ![view.viewport.x, view.viewport.y, view.viewport.zoom].every(
        Number.isFinite,
      ) ||
      view.viewport.zoom < 0.05 ||
      view.viewport.zoom > 4
    )
      throw new Error("Invalid saved viewport.");
  }
  return p;
}
export { validateScene, validateSceneResponse } from "./validation.ts";

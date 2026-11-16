/// <reference lib="webworker" />
/** Generic layout adapter. All semantic selection happens upstream. */
import ELK from "elkjs/lib/elk.bundled.js";
import type { ElkNode, ElkExtendedEdge } from "elkjs";
import type { Scene, Node, Layout, Geometry, Presentation } from "./protocol";
const elk = new ELK();
const scope = self as unknown as DedicatedWorkerGlobalScope;
scope.onmessage = async ({
  data,
}: MessageEvent<{ id: number; scene: Scene; presentation: Presentation }>) => {
  const { id, scene, presentation } = data;
  try {
    const saved = presentation.views[scene.view.key]?.positions ?? {};
    const edgeList: ElkExtendedEdge[] = scene.edges.map((e) => ({
      id: e.id,
      sources: [e.source],
      targets: [e.target],
      labels: e.label
        ? [{ text: e.label, width: e.label.length * 6 + 10, height: 16 }]
        : [],
    }));
    const toElk = (node: Node): ElkNode => {
      const rows = node.compartments.reduce(
        (sum, c) => sum + 20 + c.rows.length * 20,
        0,
      );
      const longest = Math.max(
        node.title.length,
        ...node.compartments.flatMap((c) => c.rows.map((r) => r.label.length)),
        18,
      );
      return {
        id: node.id,
        width: Math.min(340, Math.max(205, longest * 7 + 28)),
        height: 56 + rows + (node.ports.length ? 28 : 0),
        children: node.children.map(toElk),
        ports: node.ports.map((p, i) => ({
          id: p.id,
          width: 12,
          height: 12,
          layoutOptions: {
            "elk.port.side": p.side,
            "elk.port.index": String(i),
          },
        })),
        layoutOptions: {
          "elk.algorithm": "layered",
          "elk.direction": "RIGHT",
          "elk.padding": `[top=${64 + rows},left=28,bottom=38,right=28]`,
          "elk.portConstraints": "FIXED_SIDE",
          "elk.spacing.nodeNode": "55",
        },
      };
    };
    const graph = await elk.layout({
      id: "root",
      children: scene.nodes.map(toElk),
      edges: edgeList,
      layoutOptions: {
        "elk.algorithm": "layered",
        "elk.direction": scene.layout?.direction ?? "RIGHT",
        "elk.hierarchyHandling": "INCLUDE_CHILDREN",
        "elk.edgeRouting": "ORTHOGONAL",
        "elk.spacing.nodeNode": "65",
        "elk.layered.spacing.nodeNodeBetweenLayers": "85",
        "elk.padding": "[top=40,left=50,bottom=40,right=50]",
      },
    });
    const result: Layout = {
      revision: scene.revision,
      viewKey: scene.view.key,
      nodes: {},
      ports: {},
      edges: {},
    };
    const collect = (node: ElkNode, parentX: number, parentY: number) => {
      const g: Geometry = {
        x: node.x ?? 0,
        y: node.y ?? 0,
        width: node.width ?? 200,
        height: node.height ?? 70,
      };
      const previous = saved[node.id];
      if (previous) {
        g.x = previous.x;
        g.y = previous.y;
        g.width = Math.max(g.width, previous.width);
        g.height = Math.max(g.height, previous.height);
      }
      // Persisted positions are local to visual parents, not to the semantic model.
      result.nodes[node.id] = g;
      for (const port of node.ports ?? [])
        result.ports[port.id] = {
          x: g.x + parentX + (port.x ?? 0) + 6,
          y: g.y + parentY + (port.y ?? 0) + 6,
        };
      for (const child of node.children ?? [])
        collect(child, parentX + g.x, parentY + g.y);
    };
    for (const node of graph.children ?? []) collect(node, 0, 0);
    // Edges are rerouted by the renderer from final endpoints when user positions
    // differ from automatic layout. No stale automatic routes are reused.
    for (const edge of graph.edges ?? []) {
      const section = edge.sections?.[0];
      if (section)
        result.edges[edge.id] = [
          section.startPoint,
          ...(section.bendPoints ?? []),
          section.endPoint,
        ];
    }
    scope.postMessage({ id, result });
  } catch (error) {
    scope.postMessage({
      id,
      error: error instanceof Error ? error.message : String(error),
    });
  }
};

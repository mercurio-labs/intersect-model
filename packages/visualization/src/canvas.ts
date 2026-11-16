import "./canvas.css";
import { validateSceneResponse } from "./protocol";
/** Intersect visualization client.
 * SVG/ELK adapter. It consumes presentation DTOs and emits intents, never source. */
import LayoutWorker from "./layout.worker?worker";
import {
  defaultNotation,
  type Binding,
  type Geometry,
  type Layout,
  type Node,
  type Palette,
  type Point,
  type Presentation,
  type SceneResponse,
  type Tool,
  type View,
} from "./protocol";

const esc = (s: string) =>
  s.replace(
    /[&<>"']/g,
    (c) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        c
      ]!,
  );
const glyph: Record<string, string> = {
  pointer: "↖",
  maximize: "⛶",
  layout: "▦",
  box: "◇",
  port: "▣",
  connection: "⤳",
};
interface Callbacks {
  view(key: string): void;
  select(binding: Binding, reveal: boolean): void;
  command(
    tool: Tool,
    owner: string,
    args: Record<string, string>,
  ): Promise<void>;
  presentation(value: Presentation): void;
  save(): void;
  palette(path: string): void;
  undo(): void;
  redo?(): void;
}
export class DiagramCanvas {
  private worker = new LayoutWorker();
  private response?: SceneResponse;
  private layout?: Layout;
  private presentation: Presentation = { schemaVersion: 1, views: {} };
  private sequence = 0;
  private stale = true;
  private busy = false;
  private tool?: Tool;
  private selected?: string;
  private bindings = new Map<string, Binding>();
  private viewList: View[] = [];
  private graph: SVGSVGElement;
  private message: HTMLElement;
  private controls: HTMLElement;
  private paletteElement: HTMLElement;
  private viewport = { x: 0, y: 0, zoom: 1 };
  private absolute = new Map<string, Geometry>();
  private portOwners = new Map<string, string>();
  private drag?: { id?: string; start: Point; origin: Point; moved: boolean };
  private resize: ResizeObserver;
  private clickBinding?: Binding;
  private needsFit = true;
  private connection?: {
    source: string;
    start: Point;
    current: Point;
    pointer: number;
  };
  private placing?: {
    before: Set<string>;
    point: Point;
    owner?: string;
    border: boolean;
  };
  private portDrag?: { id: string; owner: string; moved: boolean };
  private resizeDrag?: {
    id: string;
    start: Point;
    width: number;
    height: number;
  };

  constructor(
    private root: HTMLElement,
    private callbacks: Callbacks,
  ) {
    root.classList.add("diagram-workbench");
    root.innerHTML = `<div class="diagram-heading"><span class="diagram-badge">VIEW</span><select aria-label="Diagram view"></select><span class="diagram-kind"></span><button class="diagram-button" data-reveal-view title="Reveal view declaration">↗ Source</button><button class="diagram-button" data-save-layout>Save layout</button></div><div class="diagram-status" role="status"></div><div class="diagram-body"><aside class="diagram-palette" aria-label="Diagram palette"></aside><div class="diagram-viewport" tabindex="0" aria-label="Diagram canvas"><svg role="img" aria-label="Model diagram"><defs><pattern id="diagram-grid" width="20" height="20" patternUnits="userSpaceOnUse"><circle cx="1" cy="1" r=".7" class="grid-dot"/></pattern><marker id="general-arrow" viewBox="0 0 12 12" refX="11" refY="6" markerWidth="12" markerHeight="12" orient="auto-start-reverse"><path d="M1 1 L11 6 L1 11 Z" class="general-arrow"/></marker></defs><rect width="100%" height="100%" fill="url(#diagram-grid)"/><g class="diagram-world"></g></svg><div class="diagram-hint">Drag symbols to arrange · drag background to pan · scroll to zoom</div><div class="diagram-zoom"><button aria-label="Zoom out">−</button><span>100%</span><button aria-label="Zoom in">+</button><button aria-label="Fit diagram">⛶</button></div></div></div><form class="diagram-command" hidden></form>`;
    this.graph = root.querySelector("svg")!;
    const details = document.createElement("div");
    details.className = "diagram-details";
    details.hidden = true;
    root.querySelector(".diagram-heading")!.after(details);
    details.addEventListener("click", (e) => {
      const index = (e.target as Element).closest<HTMLButtonElement>(
        "[data-detail]",
      )?.dataset.detail;
      if (index !== undefined && this.response)
        this.callbacks.select(
          this.response.scene.view.details[Number(index)].binding,
          true,
        );
    });
    this.message = root.querySelector(".diagram-status")!;
    this.controls = root.querySelector(".diagram-command")!;
    this.paletteElement = root.querySelector(".diagram-palette")!;
    root
      .querySelector("select")!
      .addEventListener("change", (e) =>
        callbacks.view((e.target as HTMLSelectElement).value),
      );
    root.querySelector("[data-reveal-view]")!.addEventListener("click", () => {
      if (this.response)
        callbacks.select(this.response.scene.view.binding, true);
    });
    root
      .querySelector("[data-save-layout]")!
      .addEventListener("click", () => callbacks.save());
    this.paletteElement.addEventListener("change", (e) => {
      const el = e.target as HTMLSelectElement;
      if (el.matches("[data-palette-file]")) callbacks.palette(el.value);
    });
    this.paletteElement.addEventListener("click", (e) => {
      const id = (e.target as Element).closest<HTMLButtonElement>("[data-tool]")
        ?.dataset.tool;
      if (id) this.activate(id);
    });
    this.paletteElement.addEventListener("dragstart", (e) => {
      const button = (e.target as Element).closest<HTMLButtonElement>(
        "[data-tool]",
      );
      const tool = this.findTool(button?.dataset.tool);
      if (
        !button ||
        button.disabled ||
        !tool ||
        tool.gesture !== "place" ||
        this.placement(tool) === "connection"
      ) {
        e.preventDefault();
        return;
      }
      e.dataTransfer?.setData("application/x-intersect-tool", tool.id);
      e.dataTransfer?.setData("text/plain", tool.label);
      if (e.dataTransfer) e.dataTransfer.effectAllowed = "copy";
    });
    this.graph.addEventListener("dragover", (e) => {
      if (
        !e.dataTransfer?.types.includes("application/x-intersect-tool") ||
        this.stale ||
        this.busy
      )
        return;
      e.preventDefault();
      if (e.dataTransfer) e.dataTransfer.dropEffect = "copy";
      this.dropFeedback(e.clientX, e.clientY);
    });
    this.graph.addEventListener("dragleave", () => this.clearFeedback());
    this.graph.addEventListener("drop", (e) => {
      e.preventDefault();
      this.clearFeedback();
      const tool = this.findTool(
        e.dataTransfer?.getData("application/x-intersect-tool"),
      );
      if (
        !tool ||
        this.stale ||
        this.busy ||
        !this.response?.capabilities.some(
          (c) => c.toolId === tool.id && c.enabled,
        )
      )
        return;
      this.tool = tool;
      this.renderTools();
      void this.place(e.clientX, e.clientY);
    });
    this.worker.onmessage = ({ data }) => {
      if (data.id !== this.sequence) return;
      if (data.error) {
        this.status(`Layout failed: ${data.error}`);
        return;
      }
      this.layout = data.result as Layout;
      this.draw();
      this.finishPlacement();
      if (!this.presentation.views[this.response!.scene.view.key])
        this.fit(false);
    };
    this.worker.onerror = () =>
      this.status("The layout worker failed. Reopen this view to retry.");
    this.graph.addEventListener("pointerdown", (e) => this.pointerDown(e));
    this.graph.addEventListener("pointermove", (e) => this.pointerMove(e));
    this.graph.addEventListener("pointerup", (e) => this.pointerUp(e));
    this.graph.addEventListener("pointercancel", () => {
      this.cancelGesture();
    });
    this.graph.addEventListener("dblclick", (e) => {
      const b = this.bindingAt(e.target) ?? this.clickBinding;
      if (b) this.callbacks.select(b, true);
    });
    this.graph.addEventListener(
      "wheel",
      (e) => {
        e.preventDefault();
        const r = this.graph.getBoundingClientRect();
        this.zoom(Math.exp(-e.deltaY * 0.0015), {
          x: e.clientX - r.left,
          y: e.clientY - r.top,
        });
      },
      { passive: false },
    );
    root
      .querySelector('[aria-label="Zoom out"]')!
      .addEventListener("click", () => this.zoom(0.8));
    root
      .querySelector('[aria-label="Zoom in"]')!
      .addEventListener("click", () => this.zoom(1.25));
    root
      .querySelector('[aria-label="Fit diagram"]')!
      .addEventListener("click", () => this.fit());
    root
      .querySelector(".diagram-viewport")!
      .addEventListener("keydown", (e) => {
        const event = e as KeyboardEvent;
        if (event.key === "Escape") {
          this.cancelGesture();
          this.tool = undefined;
          this.renderTools();
        }
        if (
          (event.ctrlKey || event.metaKey) &&
          ["z", "y"].includes(event.key.toLowerCase())
        ) {
          event.preventDefault();
          if (event.key.toLowerCase() === "y" || event.shiftKey)
            callbacks.redo?.();
          else callbacks.undo();
        }

        if (event.key === "Enter" && this.selected) {
          const b = [...this.bindings.values()].find(
            (b) => b.elementId === this.selected,
          );
          if (b) callbacks.select(b, true);
        }
      });
    this.controls.addEventListener("submit", (e) => {
      e.preventDefault();
      void this.submit();
    });
    this.controls.addEventListener("click", (e) => {
      if ((e.target as Element).closest("[data-cancel]")) {
        this.tool = undefined;
        this.renderTools();
      }
    });
    this.resize = new ResizeObserver(() => {
      if (this.needsFit) this.fit(false);
      else this.transform();
    });
    this.resize.observe(this.graph);
    this.status("Open a folder with SysML view usages to explore diagrams.");
  }
  reset(presentation: Presentation) {
    this.sequence++;
    this.cancelGesture();
    this.placing = undefined;
    this.response = undefined;
    this.layout = undefined;
    this.presentation = presentation;
    this.tool = undefined;
    this.selected = undefined;
    this.viewList = [];
    this.views([]);
    this.graph.querySelector(".diagram-world")!.replaceChildren();
    this.paletteElement.replaceChildren();
    this.controls.hidden = true;
    this.status("Waiting for a clean model and its view usages…");
  }
  views(views: View[], current?: string) {
    this.viewList = views;
    this.root.querySelector("select")!.innerHTML = views.length
      ? views
          .map(
            (v) =>
              `<option value="${esc(v.key)}" ${v.key === current ? "selected" : ""}>${esc(v.name)}</option>`,
          )
          .join("")
      : "<option>No view usages</option>";
    if (!views.length) {
      this.requestView("");
      this.status(
        "No view usages. Define a view specializing a standard GeneralView or InterconnectionView.",
      );
    }
  }
  requestView(key: string) {
    if (this.response?.scene.view.key === key) return;
    this.sequence++;
    this.cancelGesture();
    this.placing = undefined;
    this.response = undefined;
    this.layout = undefined;
    this.tool = undefined;
    this.graph.querySelector(".diagram-world")!.replaceChildren();
    this.paletteElement.replaceChildren();
    this.controls.hidden = true;
    this.root.querySelector<HTMLElement>(".diagram-details")!.hidden = true;
    this.status("Loading view…");
  }
  setScene(response: SceneResponse) {
    validateSceneResponse(response);
    const changed = this.response?.scene.view.key !== response.scene.view.key;
    this.response = response;
    this.stale = false;
    this.tool = undefined;
    if (changed) {
      this.viewport = {
        ...(this.presentation.views[response.scene.view.key]?.viewport ?? {
          x: 0,
          y: 0,
          zoom: 1,
        }),
      };
      this.layout = undefined;
      this.needsFit = !this.presentation.views[response.scene.view.key];
    }
    this.root.querySelector(".diagram-kind")!.textContent =
      response.scene.view.kind;
    const details = this.root.querySelector<HTMLElement>(".diagram-details")!;
    details.hidden = !response.scene.view.details.length;
    details.innerHTML = response.scene.view.details
      .map(
        (d, i) =>
          `<button data-detail="${i}" title="Reveal declaration">${esc(d.label)} ↗</button>`,
      )
      .join("");
    this.views(this.viewList, response.scene.view.key);
    this.renderTools();
    this.status(response.scene.problems.join(" · "));
    this.arrange(false);
  }
  setStale() {
    this.stale = true;
    this.cancelGesture();
    this.sequence++;
    this.renderTools();
    if (this.response)
      this.status(
        "Source changed · showing the last accepted model. Modeling tools resume after clean analysis.",
      );
  }
  setPresentation(value: Presentation) {
    this.presentation = value;
    if (this.response) {
      const saved = value.views[this.response.scene.view.key];
      if (saved) this.viewport = { ...saved.viewport };
      this.arrange(false);
    }
  }
  status(text: string) {
    this.message.textContent = text;
    this.message.hidden = !text;
  }
  setPaletteFiles(paths: string[], selected = "") {
    this.palettePaths = paths;
    this.palettePath = selected;
    this.renderTools();
  }
  private palettePaths: string[] = [];
  private palettePath = "";
  highlight(id: string | null) {
    this.selected = id ?? undefined;
    for (const el of this.graph.querySelectorAll<SVGElement>("[data-binding]"))
      el.classList.toggle(
        "diagram-selected",
        this.bindings.get(el.dataset.binding!)?.elementId === id,
      );
  }
  private state() {
    const key = this.response?.scene.view.key;
    if (!key) return;
    return (this.presentation.views[key] ??= {
      positions: {},
      viewport: { ...this.viewport },
    });
  }
  private persist() {
    const s = this.state();
    if (s) {
      s.viewport = { ...this.viewport };
      this.callbacks.presentation(structuredClone(this.presentation));
    }
  }
  private transform() {
    this.graph
      .querySelector(".diagram-world")!
      .setAttribute(
        "transform",
        `translate(${this.viewport.x},${this.viewport.y}) scale(${this.viewport.zoom})`,
      );
    this.root.querySelector(".diagram-zoom span")!.textContent =
      `${Math.round(this.viewport.zoom * 100)}%`;
  }
  private zoom(factor: number, at?: Point) {
    const r = this.graph.getBoundingClientRect(),
      p = at ?? { x: r.width / 2, y: r.height / 2 },
      z = Math.max(0.05, Math.min(4, this.viewport.zoom * factor));
    const ratio = z / this.viewport.zoom;
    this.viewport.x = p.x - (p.x - this.viewport.x) * ratio;
    this.viewport.y = p.y - (p.y - this.viewport.y) * ratio;
    this.viewport.zoom = z;
    this.transform();
    this.persist();
  }
  private fit(save = true) {
    if (!this.layout) return;
    const roots = this.response!.scene.nodes.map(
      (n) => this.layout!.nodes[n.id],
    ).filter(Boolean);
    if (!roots.length) return;
    const x = Math.min(...roots.map((g) => g.x)),
      y = Math.min(...roots.map((g) => g.y)),
      w = Math.max(...roots.map((g) => g.x + g.width)) - x,
      h = Math.max(...roots.map((g) => g.y + g.height)) - y,
      r = this.graph.getBoundingClientRect();
    if (r.width < 100 || r.height < 100) return;
    const z = Math.max(
      0.05,
      Math.min(1.2, (r.width - 70) / w, (r.height - 70) / h),
    );
    this.viewport = {
      x: (r.width - w * z) / 2 - x * z,
      y: (r.height - h * z) / 2 - y * z,
      zoom: z,
    };
    this.needsFit = false;
    this.transform();
    if (save) this.persist();
  }
  private arrange(clear = true) {
    if (!this.response) return;
    if (clear) {
      const state = this.state();
      if (state) state.positions = {};
      this.persist();
    }
    this.worker.postMessage({
      id: ++this.sequence,
      scene: this.response.scene,
      presentation: this.presentation,
    });
  }
  private renderTools() {
    const response = this.response;
    if (!response) return;
    this.paletteElement.innerHTML = `<label class="palette-heading">PALETTE<select data-palette-file aria-label="Palette definition"><option value="">Standard structure</option>${this.palettePaths.map((path) => `<option value="${esc(path)}" ${path === this.palettePath ? "selected" : ""}>${esc(path)}</option>`).join("")}</select></label>${response.scene.palette.groups
      .map(
        (g) =>
          `<section><h4>${esc(g.label)}</h4>${g.entries
            .map((t) => {
              const cap = response.capabilities.find((c) => c.toolId === t.id),
                disabled =
                  this.busy ||
                  !cap?.enabled ||
                  (this.stale && !t.command.startsWith("presentation."));
              return `<button type="button" data-tool="${esc(t.id)}" draggable="${!disabled && t.gesture === "place" && this.placement(t) !== "connection"}" class="palette-tool ${this.tool?.id === t.id ? "active" : ""}" ${disabled ? "disabled" : ""} title="${esc(cap?.reason || t.label)}"><span class="palette-glyph" aria-hidden="true">${esc(glyph[t.icon] ?? "◇")}</span>${esc(t.label)}</button>`;
            })
            .join("")}</section>`,
      )
      .join("")}`;
    this.controls.hidden = !this.tool;
    if (this.tool) {
      this.controls.innerHTML = `<strong>${esc(this.tool.label)}</strong><label>Owner<select name="owner">${response.scene.view.owners.map((o) => `<option value="${esc(o.key)}">${esc(o.label)}</option>`).join("")}</select></label>${(this.tool.inputs ?? []).map((i) => `<label>${esc(i.label)}<input name="arg:${esc(i.id)}" aria-label="${esc(i.label)}" value="${esc(i.default)}" required maxlength="128" autocomplete="off"/></label>`).join("")}<button type="submit" ${this.stale || this.busy ? "disabled" : ""}>Create</button><button type="button" data-cancel>Cancel</button><span>Drag from the palette or click the canvas to place. Drag a connection between parts or ports.</span>`;
    }
    this.root.classList.toggle("placing", !!this.tool);
    const hint = this.root.querySelector<HTMLElement>(".diagram-hint")!;
    hint.textContent = this.tool
      ? this.placement(this.tool) === "connection"
        ? "Drag from a part or port to another endpoint · Escape cancels"
        : this.placement(this.tool) === "border"
          ? "Drop on a part border · Escape cancels"
          : "Drop on the canvas or inside a part · Escape cancels"
      : "Drag symbols to arrange · drag ports along borders · drag background to pan · scroll to zoom";
  }
  private activate(id: string) {
    const tool = this.response?.scene.palette.groups
      .flatMap((g) => g.entries)
      .find((t) => t.id === id);
    if (!tool) return;
    if (tool.command === "presentation.fit") {
      this.fit();
      return;
    }
    if (tool.command === "presentation.layout") {
      this.arrange();
      return;
    }
    if (tool.gesture === "select") {
      this.tool = undefined;
      this.renderTools();
      return;
    }
    this.cancelGesture();
    this.tool = tool;
    this.renderTools();
    this.controls.querySelector("input")?.focus();
  }
  private async submit() {
    if (!this.tool || this.stale || this.busy) return;
    const form = new FormData(this.controls as HTMLFormElement),
      args: Record<string, string> = {};
    for (const [key, value] of form)
      if (key.startsWith("arg:")) args[key.slice(4)] = String(value);
    const tool = this.tool,
      owner = String(form.get("owner") ?? "");
    this.busy = true;
    this.renderTools();
    try {
      await this.callbacks.command(tool, owner, args);
      this.tool = undefined;
    } catch (error) {
      this.status(error instanceof Error ? error.message : String(error));
    } finally {
      this.busy = false;
      this.renderTools();
    }
  }
  private bindingAt(target: EventTarget | null) {
    const el = (target as Element)?.closest<SVGElement>("[data-binding]");
    return el ? this.bindings.get(el.dataset.binding!) : undefined;
  }
  private pointerDown(e: PointerEvent) {
    if (e.button !== 0) return;
    if (this.busy) return;
    const world = this.worldPoint(e.clientX, e.clientY);
    const endpoint = this.endpointAt(e.clientX, e.clientY);
    if (this.tool && !this.stale) {
      if (this.placement(this.tool) === "connection") {
        if (!endpoint) {
          this.status("Start the connection on a part or port.");
          return;
        }
        this.connection = {
          source: endpoint,
          start: this.anchor(endpoint, world),
          current: world,
          pointer: e.pointerId,
        };
        this.graph.setPointerCapture(e.pointerId);
        this.previewConnection();
      } else void this.place(e.clientX, e.clientY);
      return;
    }
    const handle = (e.target as Element).closest<SVGElement>("[data-resize]");
    if (handle && this.layout) {
      const id = handle.dataset.resize!,
        g = this.layout.nodes[id];
      this.resizeDrag = { id, start: world, width: g.width, height: g.height };
      this.graph.setPointerCapture(e.pointerId);
      return;
    }
    const port = (e.target as Element).closest<SVGElement>("[data-port]")
      ?.dataset.port;
    if (port) {
      this.portDrag = {
        id: port,
        owner: this.portOwners.get(port)!,
        moved: false,
      };
      this.graph.setPointerCapture(e.pointerId);
    }
    const binding = this.bindingAt(e.target);
    // Pointer capture retargets the subsequent double-click to the SVG root.
    this.clickBinding = binding;
    if (binding) {
      this.highlight(binding.elementId);
      this.callbacks.select(binding, false);
    }
    if (port) return;

    const symbol = (e.target as Element).closest<SVGElement>("[data-symbol]")
      ?.dataset.symbol;
    const g = symbol ? this.layout?.nodes[symbol] : undefined;
    this.drag = {
      id: g ? symbol : undefined,
      start: { x: e.clientX, y: e.clientY },
      origin: g
        ? { x: g.x, y: g.y }
        : { x: this.viewport.x, y: this.viewport.y },
      moved: false,
    };
    this.graph.setPointerCapture(e.pointerId);
    this.root.querySelector<HTMLElement>(".diagram-viewport")!.focus();
  }
  private pointerMove(e: PointerEvent) {
    const point = this.worldPoint(e.clientX, e.clientY);
    if (this.connection) {
      const endpoint = this.endpointAt(e.clientX, e.clientY);
      this.connection.current = endpoint ? this.anchor(endpoint, point) : point;
      this.previewConnection();
      this.dropFeedback(e.clientX, e.clientY);
      return;
    }
    if (this.portDrag) {
      const owner = this.absolute.get(this.portDrag.owner);
      if (owner) {
        const p = this.borderPoint(point, owner);
        this.state()!.positions[this.portDrag.id] = {
          x: p.x,
          y: p.y,
          width: 12,
          height: 12,
        };
        this.portDrag.moved = true;
        this.draw();
      }
      return;
    }
    if (this.resizeDrag && this.layout) {
      const drag = this.resizeDrag,
        g = this.layout.nodes[drag.id];
      g.width = Math.max(150, drag.width + point.x - drag.start.x);
      g.height = Math.max(70, drag.height + point.y - drag.start.y);
      this.draw();
      return;
    }
    const drag = this.drag;
    if (!drag) return;
    const dx = e.clientX - drag.start.x,
      dy = e.clientY - drag.start.y;
    if (Math.abs(dx) + Math.abs(dy) < 3 && !drag.moved) return;
    drag.moved = true;
    if (drag.id && this.layout) {
      const g = this.layout.nodes[drag.id];
      g.x = drag.origin.x + dx / this.viewport.zoom;
      g.y = drag.origin.y + dy / this.viewport.zoom;
      this.draw();
    } else {
      this.viewport.x = drag.origin.x + dx;
      this.viewport.y = drag.origin.y + dy;
      this.transform();
    }
  }
  private pointerUp(e: PointerEvent) {
    if (this.graph.hasPointerCapture(e.pointerId))
      this.graph.releasePointerCapture(e.pointerId);
    if (this.connection) {
      const source = this.connection.source,
        target = this.endpointAt(e.clientX, e.clientY);
      this.connection = undefined;
      this.previewConnection();
      this.clearFeedback();
      if (target && target !== source) void this.connect(source, target);
      else this.status("Choose a different part or port as the target.");
      return;
    }
    if (this.portDrag) {
      const moved = this.portDrag.moved;
      this.portDrag = undefined;
      if (moved) this.persist();
      return;
    }
    if (this.resizeDrag && this.layout) {
      this.state()!.positions[this.resizeDrag.id] = {
        ...this.layout.nodes[this.resizeDrag.id],
      };
      this.resizeDrag = undefined;
      this.persist();
      return;
    }
    const drag = this.drag;
    this.drag = undefined;
    if (this.graph.hasPointerCapture(e.pointerId))
      this.graph.releasePointerCapture(e.pointerId);
    if (drag?.moved) {
      if (drag.id && this.layout) {
        const s = this.state();
        if (s) s.positions[drag.id] = { ...this.layout.nodes[drag.id] };
      }
      this.persist();
    }
  }
  private draw() {
    if (!this.response || !this.layout) return;
    this.bindings.clear();
    this.absolute.clear();
    this.portOwners.clear();
    const bind = (b: Binding) => {
      const id = `b${this.bindings.size}`;
      this.bindings.set(id, b);
      return `data-binding="${id}"`;
    };
    const render = (n: Node, offset: Point): string => {
      const g = this.layout!.nodes[n.id];
      if (!g) return "";
      const a = { ...g, x: g.x + offset.x, y: g.y + offset.y };
      this.absolute.set(n.id, a);
      const style =
        defaultNotation.shapes[n.notation] ?? defaultNotation.shapes.usage;
      let y = 66;
      const compartments = n.compartments
        .map((c) => {
          let html = `<path class="compartment-rule" d="M0 ${y - 12} H${g.width}"/><text class="compartment-label" x="14" y="${y + 2}">${esc(c.label)}</text>`;
          y += 23;
          for (const row of c.rows) {
            html += `<g class="diagram-field" ${bind(row.binding)}><rect x="1" y="${y - 14}" width="${g.width - 2}" height="20"/><text x="14" y="${y}">${esc(row.label)}</text><title>${esc(row.label)}</title></g>`;
            y += 20;
          }
          return html;
        })
        .join("");
      const ports = n.ports
        .map((p, i) => {
          const old = this.layout!.ports[p.id];
          let point = old
            ? { ...old }
            : { x: a.x + g.width, y: a.y + 65 + i * 28 };
          // ELK stores absolute port centers. Translate them with their owning node.
          const base = this.portBase.get(p.id);
          if (base)
            point = {
              x: base.x + (a.x - base.ownerX),
              y: base.y + (a.y - base.ownerY),
            };
          else {
            this.portBase.set(p.id, { ...point, ownerX: a.x, ownerY: a.y });
          }
          const saved =
            this.presentation.views[this.response!.scene.view.key]?.positions[
              p.id
            ];
          if (saved) {
            const local = this.borderPoint(
              { x: a.x + saved.x, y: a.y + saved.y },
              a,
            );
            point = { x: a.x + local.x, y: a.y + local.y };
          }
          this.absolute.set(p.id, { ...point, width: 0, height: 0 });
          this.portOwners.set(p.id, n.id);
          const x = point.x - a.x,
            y = point.y - a.y,
            west = x < g.width / 2;
          return `<g class="diagram-port" data-port="${esc(p.id)}" ${bind(p.binding)}><rect x="${x - 6}" y="${y - 6}" width="12" height="12"/><text x="${x + (west ? 12 : -12)}" y="${y - 10}" text-anchor="${west ? "start" : "end"}">${esc(p.label)}</text><title>${esc(p.label)}</title></g>`;
        })
        .join("");
      return `<g class="diagram-node fill-${esc(style.fill)}" data-symbol="${esc(n.id)}" transform="translate(${g.x},${g.y})" ${bind(n.binding)}><rect class="node-body" width="${g.width}" height="${g.height}" rx="${style.radius ?? 0}"/><text class="node-subtitle" x="${g.width / 2}" y="20" text-anchor="middle">«${esc(n.subtitle)}»</text><text class="node-title" x="${g.width / 2}" y="40" text-anchor="middle">${esc(n.title)}</text>${compartments}${n.children.map((c) => render(c, a)).join("")}${ports}<rect class="node-resize" data-resize="${esc(n.id)}" x="${g.width - 5}" y="${g.height - 5}" width="9" height="9"/><title>${esc(n.title)} · Double click to reveal source</title></g>`;
    };
    // A new layout invalidates the initial absolute-port cache.
    if (this.portLayout !== this.layout) {
      this.portBase.clear();
      this.portLayout = this.layout;
    }
    const nodes = this.response.scene.nodes
      .map((n) => render(n, { x: 0, y: 0 }))
      .join("");
    const edges = this.response.scene.edges
      .map((edge) => {
        const s = this.absolute.get(edge.source),
          t = this.absolute.get(edge.target);
        if (!s || !t) return "";
        const a = this.anchor(edge.source, {
            x: t.x + t.width / 2,
            y: t.y + t.height / 2,
          }),
          b = this.anchor(edge.target, a),
          mid = (a.x + b.x) / 2;
        const route = `M${a.x} ${a.y} H${mid} V${b.y} H${b.x}`;
        return `<g class="diagram-edge" ${bind(edge.binding)}><path class="edge-hit" d="${route}"/><path class="edge-line" d="${route}" ${edge.notation === "specialization" ? 'marker-end="url(#general-arrow)"' : ""}/><text x="${mid + 6}" y="${(a.y + b.y) / 2 - 7}">${esc(edge.label)}</text><title>${esc(edge.label || edge.notation)}</title></g>`;
      })
      .join("");
    // Connections sit above containers, whose fill would otherwise hide them.
    this.graph.querySelector(".diagram-world")!.innerHTML = nodes + edges;
    this.transform();
    this.highlight(this.selected ?? null);
  }
  private portBase = new Map<
    string,
    Point & { ownerX: number; ownerY: number }
  >();
  private portLayout?: Layout;

  private findTool(id?: string) {
    return this.response?.scene.palette.groups
      .flatMap((g) => g.entries)
      .find((t) => t.id === id);
  }
  private placement(tool: Tool) {
    return tool.parameters?.placement ?? "node";
  }
  private worldPoint(x: number, y: number): Point {
    const r = this.graph.getBoundingClientRect();
    return {
      x: (x - r.left - this.viewport.x) / this.viewport.zoom,
      y: (y - r.top - this.viewport.y) / this.viewport.zoom,
    };
  }
  private elementAt(x: number, y: number) {
    // Edge hit targets sit above nodes; let gestures still reach their endpoints.
    for (const element of document.elementsFromPoint(x, y)) {
      const endpoint = element.closest<SVGElement>(
        "[data-port], [data-symbol]",
      );
      if (endpoint && this.graph.contains(endpoint)) return endpoint;
    }
    return undefined;
  }
  private endpointAt(x: number, y: number) {
    const el = this.elementAt(x, y);
    return el && this.graph.contains(el)
      ? (el.dataset.port ?? el.dataset.symbol)
      : undefined;
  }
  private nodeAt(x: number, y: number) {
    const el = this.elementAt(x, y)?.closest<SVGElement>("[data-symbol]");
    return el && this.graph.contains(el) ? el.dataset.symbol : undefined;
  }
  private node(
    id?: string,
    nodes = this.response?.scene.nodes ?? [],
  ): Node | undefined {
    for (const node of nodes) {
      if (node.id === id) return node;
      const child = this.node(id, node.children);
      if (child) return child;
    }
  }
  private borderPoint(point: Point, g: Geometry): Point {
    const x = Math.max(0, Math.min(g.width, point.x - g.x));
    const y = Math.max(0, Math.min(g.height, point.y - g.y));
    const distances = [x, g.width - x, y, g.height - y];
    const side = distances.indexOf(Math.min(...distances));
    return {
      x: side === 0 ? 0 : side === 1 ? g.width : x,
      y: side === 2 ? 0 : side === 3 ? g.height : y,
    };
  }
  private anchor(id: string, toward: Point): Point {
    const g = this.absolute.get(id);
    if (!g) return toward;
    if (!g.width && !g.height) return { x: g.x, y: g.y };
    const dx = toward.x - g.x - g.width / 2,
      dy = toward.y - g.y - g.height / 2;
    const scale =
      1 /
      Math.max(Math.abs(dx) / (g.width / 2), Math.abs(dy) / (g.height / 2), 1);
    return {
      x: g.x + g.width / 2 + dx * scale,
      y: g.y + g.height / 2 + dy * scale,
    };
  }
  private clearFeedback() {
    this.graph
      .querySelectorAll(".diagram-drop-target")
      .forEach((el) => el.classList.remove("diagram-drop-target"));
  }
  private dropFeedback(x: number, y: number) {
    this.clearFeedback();
    this.elementAt(x, y)?.classList.add("diagram-drop-target");
  }
  private previewConnection() {
    this.graph.querySelector(".connection-preview")?.remove();
    if (!this.connection) return;
    const { start: a, current: b } = this.connection;
    const line = document.createElementNS("http://www.w3.org/2000/svg", "path");
    line.classList.add("connection-preview");
    line.setAttribute(
      "d",
      `M${a.x} ${a.y} H${(a.x + b.x) / 2} V${b.y} H${b.x}`,
    );
    this.graph.querySelector(".diagram-world")!.append(line);
  }
  private cancelGesture() {
    if (
      this.connection &&
      this.graph.hasPointerCapture(this.connection.pointer)
    )
      this.graph.releasePointerCapture(this.connection.pointer);
    this.connection = undefined;
    this.drag = undefined;
    this.portDrag = undefined;
    this.resizeDrag = undefined;
    this.previewConnection();
    this.clearFeedback();
  }
  private arguments(tool: Tool) {
    const args: Record<string, string> = {};
    for (const input of tool.inputs ?? []) {
      const field = [
        ...this.controls.querySelectorAll<HTMLInputElement>("input"),
      ].find((el) => el.name === `arg:${input.id}`);
      args[input.id] = field?.value ?? input.default ?? "";
    }
    return args;
  }
  private selectedOwner() {
    return (
      this.controls.querySelector<HTMLSelectElement>('[name="owner"]')?.value ??
      this.response?.scene.view.owners[0]?.key ??
      ""
    );
  }
  private async place(x: number, y: number) {
    const tool = this.tool;
    if (!tool || this.stale || this.busy || !this.response) return;
    const id = this.nodeAt(x, y),
      node = this.node(id),
      border = this.placement(tool) === "border";
    if (border && !node) {
      this.status("Drop a port on a part border.");
      return;
    }
    if (node?.binding.readOnly) {
      this.status("This owner is read-only.");
      return;
    }
    const owner = node?.binding.key ?? this.selectedOwner();
    if (!owner) {
      this.status("Choose a writable owner first.");
      return;
    }
    const before = new Set(this.absolute.keys()),
      point = this.worldPoint(x, y);
    this.placing = { before, point, owner: id, border };
    await this.execute(tool, owner, this.arguments(tool));
  }
  private bindingForEndpoint(id: string): Binding | undefined {
    const node = this.node(id);
    if (node) return node.binding;
    const owner = this.node(this.portOwners.get(id));
    return owner?.ports.find((p) => p.id === id)?.binding;
  }
  private async connect(source: string, target: string) {
    const tool = this.tool,
      a = this.bindingForEndpoint(source),
      b = this.bindingForEndpoint(target);
    if (!tool || !a || !b || this.stale || this.busy) return;
    const common = a.context.filter(
      (key, i) =>
        b.context[i] === key &&
        a.context.slice(0, i).every((k, j) => b.context[j] === k),
    );
    const owner = common.at(-1) ?? this.selectedOwner();
    await this.execute(tool, owner, {
      ...this.arguments(tool),
      source,
      target,
    });
  }
  private async execute(
    tool: Tool,
    owner: string,
    args: Record<string, string>,
  ) {
    if (this.busy || this.stale) return;
    const state = this.state();
    if (state && this.layout)
      for (const [id, g] of Object.entries(this.layout.nodes))
        state.positions[id] = { ...g };
    this.busy = true;
    this.renderTools();
    try {
      await this.callbacks.command(tool, owner, args);
      this.tool = undefined;
    } catch (error) {
      this.placing = undefined;
      this.status(error instanceof Error ? error.message : String(error));
    } finally {
      this.busy = false;
      this.renderTools();
    }
  }
  private finishPlacement() {
    const placement = this.placing;
    if (!placement || !this.layout) return;
    const id = [...this.absolute.keys()].find(
      (id) =>
        !placement.before.has(id) &&
        (placement.border ? this.portOwners.has(id) : !!this.layout!.nodes[id]),
    );
    if (!id) {
      this.placing = undefined;
      return;
    }
    const parent = placement.owner
      ? this.absolute.get(placement.owner)
      : undefined;
    const state = this.state();
    if (!state) return;
    if (placement.border && parent) {
      state.positions[id] = {
        ...this.borderPoint(placement.point, parent),
        width: 12,
        height: 12,
      };
    } else {
      const g = this.layout.nodes[id];
      g.x = placement.point.x - (parent?.x ?? 0);
      g.y = placement.point.y - (parent?.y ?? 0);
      state.positions[id] = { ...g };
    }
    this.placing = undefined;
    this.draw();
    this.persist();
  }
}

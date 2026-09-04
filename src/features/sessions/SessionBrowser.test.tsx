// @vitest-environment jsdom

import {act, fireEvent, render, screen, waitFor} from "@testing-library/react";
import {readFileSync} from "node:fs";
import {resolve} from "node:path";
import {describe, expect, it, vi} from "vitest";
import {save} from "@tauri-apps/plugin-dialog";
import axe from "axe-core";
import type {
  IndexedSessionDetailV1,
  IndexedSessionPageV1,
  IndexedSessionSummaryV1,
  IndexRefreshStateV1,
  SelectableEntryV1,
} from "../../../packages/replay-contract/src";
import type {IndexedSessionClient} from "../../lib/tauri/indexed-sessions";
import {projectPresentationPlan} from "../../../packages/replay-engine/src";
import {SessionBrowser} from "./SessionBrowser";

const applicationStyles = readFileSync(resolve("src/styles.css"), "utf8");

vi.mock("@tauri-apps/plugin-dialog", () => ({save: vi.fn()}));

const codexSummary: IndexedSessionSummaryV1 = {
  schemaVersion: 1,
  sessionId: "session_codex",
  source: "codex",
  title: "Indexed API cleanup",
  createdAtMs: 1_788_109_200_000,
  lastIndexedAtMs: 1_788_112_800_000,
  sourcePresent: false,
  entryCount: 2,
  selectedEntryCount: 2,
  durationMs: 1_500,
  diagnosticCount: 0,
  contentAvailability: {reasoning: "unavailable", toolDetails: "unavailable"},
};

const claudeSummary: IndexedSessionSummaryV1 = {
  ...codexSummary,
  sessionId: "session_claude",
  source: "claude-code",
  title: "Checkout redesign",
  sourcePresent: true,
};

const copilotSummary: IndexedSessionSummaryV1 = {
  ...codexSummary,
  sessionId: "session_copilot",
  source: "copilot-cli",
  title: "GitHub Copilot CLI session",
  sourcePresent: true,
};

const vscodeSummary: IndexedSessionSummaryV1 = {
  ...codexSummary,
  sessionId: "session_vscode",
  source: "vscode-copilot",
  title: "VS Code Copilot chat",
  sourcePresent: true,
};

const entries: readonly SelectableEntryV1[] = [
  {
    entry: {
      entryKey: "entry_user",
      ordinal: 0,
      atMs: 0,
      kind: "user",
      text: "Render <script>globalThis.pwned = true</script> as text",
    },
    selected: true,
  },
  {
    entry: {
      entryKey: "entry_assistant",
      ordinal: 1,
      atMs: 1_500,
      kind: "assistant",
      markdown: "**Done** without executing markup.",
    },
    selected: true,
  },
];

function detail(
  summary: IndexedSessionSummaryV1 = codexSummary,
  pageEntries: readonly SelectableEntryV1[] = entries,
  nextCursor: string | null = null,
): IndexedSessionDetailV1 {
  return {
    schemaVersion: 1,
    summary,
    revision: {
      schemaVersion: 1,
      revisionId: `revision_${summary.sessionId}`,
      sessionId: summary.sessionId,
      indexedAtMs: summary.lastIndexedAtMs,
      entryCount: summary.entryCount,
      durationMs: summary.durationMs,
      diagnosticCount: summary.diagnosticCount,
    },
    entryPage: {
      schemaVersion: 1,
      sessionId: summary.sessionId,
      revisionId: `revision_${summary.sessionId}`,
      entries: pageEntries,
      nextCursor,
      totalEntryCount: summary.entryCount,
    },
    preferences: {
      schemaVersion: 1,
      visibility: {showToolCalls: true, showToolDetails: false, showReasoning: true},
      timing: {entryDelayMs: 1_000, playbackSpeed: 1},
      appearance: {
        theme: {
          background: "#101211",
          surface: "#171A18",
          text: "#F5F5F4",
          muted: "#9B9E9C",
          accent: "#D6AA68",
          success: "#A8D5BD",
          error: "#E59A91",
        },
        font: {family: "JetBrains Mono", sizePx: 34, lineHeight: 1.5},
      },
    },
  };
}

const idle: IndexRefreshStateV1 = {
  schemaVersion: 1,
  status: "idle",
  lastCompletedAtMs: null,
};

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return {promise, resolve, reject};
}

function clientWith(overrides: Partial<IndexedSessionClient> = {}) {
  let observer: ((state: IndexRefreshStateV1) => void) | undefined;
  const client: IndexedSessionClient = {
    listSessions: vi.fn(async (): Promise<IndexedSessionPageV1> => ({
      schemaVersion: 1,
      sortOrder: "newest",
      items: [codexSummary, claudeSummary],
      nextCursor: null,
    })),
    getSession: vi.fn(async (sessionId) =>
      detail(sessionId === claudeSummary.sessionId ? claudeSummary : codexSummary),
    ),
    refresh: vi.fn(async () => idle),
    currentRefreshState: vi.fn(async () => idle),
    subscribeToRefresh: vi.fn(async (nextObserver) => {
      observer = nextObserver;
      return vi.fn();
    }),
    getJetBrainsStatus: vi.fn<IndexedSessionClient["getJetBrainsStatus"]>(async () => ({
      schemaVersion: 1,
      pluginStatus: "not-detected",
      nativeTranscriptStatus: "unsupported",
      copilotCliStatus: "not-detected",
    })),
    deleteSession: vi.fn<IndexedSessionClient["deleteSession"]>(async () => ({
      schemaVersion: 1,
      suppressionId: "suppression_1",
      source: "codex",
      sourceDeleted: false,
      suppressedAtMs: 2_000,
    })),
    prepareSourceDeletion: vi.fn<IndexedSessionClient["prepareSourceDeletion"]>(async () => ({
      schemaVersion: 1,
      sessionId: "session_codex",
      confirmationToken: "delete_token_1",
      artifactCount: 1,
      expiresAtMs: 302_000,
    })),
    deleteSessionWithSource: vi.fn<IndexedSessionClient["deleteSessionWithSource"]>(async () => ({
      schemaVersion: 1,
      suppressionId: "suppression_1",
      source: "codex",
      sourceDeleted: true,
      suppressedAtMs: 2_000,
    })),
    listSuppressed: vi.fn<IndexedSessionClient["listSuppressed"]>(async () => ({
      schemaVersion: 1,
      items: [],
      nextCursor: null,
    })),
    restoreSuppressed: vi.fn<IndexedSessionClient["restoreSuppressed"]>(async (suppressionId) => ({
      schemaVersion: 1,
      suppressionId,
    })),
    resetLocalDatabase: vi.fn<IndexedSessionClient["resetLocalDatabase"]>(async () => ({
      schemaVersion: 1,
      resetAtMs: 3_000,
    })),
    renameSession: vi.fn<IndexedSessionClient["renameSession"]>(async (request) =>
      detail({...codexSummary, title: request.title}),
    ),
    setEntrySelections: vi.fn<IndexedSessionClient["setEntrySelections"]>(async () =>
      detail(codexSummary),
    ),
    setSessionPreferences: vi.fn<IndexedSessionClient["setSessionPreferences"]>(async () =>
      detail(codexSummary),
    ),
    createPresentationPlan: vi.fn<IndexedSessionClient["createPresentationPlan"]>(async () =>
      projectPresentationPlan(
        detail(codexSummary),
        detail(codexSummary).entryPage.entries,
        "plan_test",
        3_000,
      ),
    ),
    exportPresentation: vi.fn<IndexedSessionClient["exportPresentation"]>(async () => ({
      codec: "h264",
      width: 1_920,
      height: 1_080,
      pixelFormat: "yuv420p",
      durationMs: 2_000,
      frameCount: 60,
    })),
    cancelExport: vi.fn<IndexedSessionClient["cancelExport"]>(async () => undefined),
    revealExport: vi.fn<IndexedSessionClient["revealExport"]>(async () => undefined),
    subscribeToExportProgress: vi.fn<IndexedSessionClient["subscribeToExportProgress"]>(async () => () => undefined),
    ...overrides,
  };
  return {client, emit: (state: IndexRefreshStateV1) => observer?.(state)};
}

describe("SessionBrowser", () => {
  it("keeps the loading screen visible until an empty startup index is reconciled", async () => {
    const listSessions = vi
      .fn<IndexedSessionClient["listSessions"]>()
      .mockResolvedValueOnce({schemaVersion: 1, sortOrder: "newest", items: [], nextCursor: null})
      .mockResolvedValueOnce({schemaVersion: 1, sortOrder: "newest", items: [codexSummary], nextCursor: null});
    const {client, emit} = clientWith({listSessions});
    render(<SessionBrowser client={client} />);

    await waitFor(() => expect(listSessions).toHaveBeenCalledTimes(1));
    expect(screen.getByText("Preparing your local library")).toBeDefined();
    expect(screen.queryByText("No indexed sessions yet")).toBeNull();

    act(() => emit({
      schemaVersion: 1,
      status: "completed",
      generation: 1,
      startedAtMs: 1_000,
      completedAtMs: 2_000,
      discoveredCount: 1,
      processedCount: 1,
      indexedCount: 1,
      unchangedCount: 0,
      failedCount: 0,
      skippedCount: 0,
      warningCount: 0,
    }));
    expect(await screen.findByText("Indexed API cleanup")).toBeDefined();
    expect(screen.queryByText("Preparing your local library")).toBeNull();
  });

  it("leaves the loading screen when startup refresh status is unavailable", async () => {
    const {client} = clientWith({
      listSessions: vi.fn<IndexedSessionClient["listSessions"]>(async () => ({
        schemaVersion: 1,
        sortOrder: "newest",
        items: [],
        nextCursor: null,
      })),
      currentRefreshState: vi.fn(async () => Promise.reject(new Error("status unavailable"))),
    });
    render(<SessionBrowser client={client} />);

    expect(await screen.findByText("No indexed sessions yet")).toBeDefined();
    expect(screen.queryByText("Preparing your local library")).toBeNull();
  });

  it("keeps the sort control readable against the dark application surface", () => {
    expect(applicationStyles).toMatch(
      /\.sort-field select\s*\{[^}]*color:\s*var\(--text\);[^}]*background:\s*var\(--surface-raised\);[^}]*border:\s*1px solid var\(--border-strong\);/s,
    );
    expect(applicationStyles).toMatch(
      /\.sort-field option\s*\{[^}]*color:\s*var\(--text\);[^}]*background:\s*var\(--canvas\);/s,
    );
  });

  it("sizes source-status badges like adjacent action buttons", () => {
    expect(applicationStyles).toMatch(
      /\.source-presence\s*\{[^}]*min-height:\s*36px;[^}]*padding:\s*8px 12px;[^}]*border-radius:\s*5px;[^}]*font-size:\s*inherit;/s,
    );
  });

  it("keeps long session content inside the list column", async () => {
    const longTitle = "A-very-long-unbroken-session-title-".repeat(16);
    const longSummary = {...codexSummary, title: longTitle};
    const {client} = clientWith({
      listSessions: vi.fn<IndexedSessionClient["listSessions"]>(async () => ({
        schemaVersion: 1,
        sortOrder: "newest",
        items: [longSummary],
        nextCursor: null,
      })),
    });
    const {container} = render(<SessionBrowser client={client} />);

    await screen.findByText(longTitle);
    const list = container.querySelector<HTMLElement>(".session-list");
    const items = container.querySelector<HTMLElement>(".session-items");
    const row = container.querySelector<HTMLElement>(".session-row");
    const topline = container.querySelector<HTMLElement>(".session-row__topline");
    const title = topline?.querySelector<HTMLElement>("strong");
    expect(list).not.toBeNull();
    expect(items).not.toBeNull();
    expect(row).not.toBeNull();
    expect(topline).not.toBeNull();
    expect(title).not.toBeNull();
    expect(applicationStyles).toMatch(
      /\.session-list\s*\{[^}]*overflow-x:\s*hidden;/s,
    );
    expect(applicationStyles).toMatch(
      /\.session-items\s*\{[^}]*min-width:\s*0;/s,
    );
    expect(applicationStyles).toMatch(
      /\.session-items > li\s*\{[^}]*min-width:\s*0;/s,
    );
    expect(applicationStyles).toMatch(
      /\.session-row\s*\{[^}]*min-width:\s*0;[^}]*max-width:\s*100%;/s,
    );
    expect(applicationStyles).toMatch(
      /\.session-row__topline[^{]*\{[^}]*min-width:\s*0;/s,
    );
    expect(applicationStyles).toMatch(
      /\.session-row__topline strong\s*\{[^}]*min-width:\s*0;[^}]*text-overflow:\s*ellipsis;/s,
    );
  });

  it("keeps selected-session actions below long titles and inside the viewport", () => {
    expect(applicationStyles).toMatch(
      /\.conversation__header\s*\{[^}]*display:\s*grid;[^}]*grid-template-columns:\s*minmax\(0, 1fr\);/s,
    );
    expect(applicationStyles).toMatch(
      /\.conversation__header > div:first-child\s*\{[^}]*min-width:\s*0;/s,
    );
    expect(applicationStyles).toMatch(
      /\.conversation__header h2\s*\{[^}]*max-width:\s*100%;[^}]*overflow-wrap:\s*anywhere;[^}]*-webkit-line-clamp:\s*2;[^}]*white-space:\s*normal;/s,
    );
    expect(applicationStyles).toMatch(
      /\.conversation__actions\s*\{[^}]*width:\s*100%;[^}]*min-width:\s*0;[^}]*flex-wrap:\s*wrap;/s,
    );
  });

  it("defaults to newest-first and reloads the complete list when sort order changes", async () => {
    const listSessions = vi.fn<IndexedSessionClient["listSessions"]>(async (request) => ({
      schemaVersion: 1,
      sortOrder: request.sortOrder,
      items: request.sortOrder === "newest"
        ? [claudeSummary, codexSummary]
        : [codexSummary, claudeSummary],
      nextCursor: null,
    }));
    const {client} = clientWith({listSessions});
    render(<SessionBrowser client={client} />);

    await waitFor(() => expect(listSessions).toHaveBeenCalledWith(
      expect.objectContaining({sortOrder: "newest", cursor: null}),
    ));
    expect(screen.getAllByRole("button", {name: /^Open .* from /})[0]?.getAttribute("aria-label"))
      .toBe("Open Checkout redesign from Claude Code");

    fireEvent.change(screen.getByRole("combobox", {name: "Sort sessions"}), {
      target: {value: "oldest"},
    });
    await waitFor(() => expect(listSessions).toHaveBeenLastCalledWith(
      expect.objectContaining({sortOrder: "oldest", cursor: null}),
    ));
    expect(screen.getAllByRole("button", {name: /^Open .* from /})[0]?.getAttribute("aria-label"))
      .toBe("Open Indexed API cleanup from Codex CLI");
  });

  it("has no detectable structural accessibility violations", async () => {
    const {client} = clientWith();
    render(<SessionBrowser client={client} />);
    fireEvent.click(await screen.findByRole("button", {name: "Open Indexed API cleanup from Codex CLI"}));
    await screen.findByRole("heading", {name: "Indexed API cleanup"});

    const results = await axe.run(document.body, {
      rules: {"color-contrast": {enabled: false}},
    });
    expect(results.violations.map(({id}) => id)).toEqual([]);
  });

  it("opens an interactive presentation without starting an export", async () => {
    const {client} = clientWith();
    render(<SessionBrowser client={client} />);
    fireEvent.click(
      await screen.findByRole("button", {name: "Open Indexed API cleanup from Codex CLI"}),
    );
    await screen.findByRole("heading", {name: "Indexed API cleanup"});

    fireEvent.click(screen.getByRole("button", {name: "Present"}));
    const dialog = await screen.findByRole("dialog", {name: "Indexed API cleanup"});
    const stage = screen.getByRole("region", {name: "Presentation entry"});
    expect(document.activeElement).toBe(stage);
    expect(dialog.textContent).toContain("1 / 2");
    fireEvent.keyDown(stage, {key: "ArrowDown"});
    expect(dialog.textContent).toContain("2 / 2");
    expect(stage.querySelector('[data-testid="entry-entry_user"]')).toBeNull();
    fireEvent.keyDown(stage, {key: "ArrowUp"});
    expect(dialog.textContent).toContain("1 / 2");
    fireEvent.keyDown(stage, {key: "ArrowRight"});
    expect(dialog.textContent).toContain("2 / 2");
    fireEvent.keyDown(stage, {key: "Home"});
    expect(dialog.textContent).toContain("1 / 2");
    const play = screen.getByRole("button", {name: "Play"});
    play.focus();
    fireEvent.keyDown(play, {key: " "});
    expect(screen.getByRole("button", {name: "Play"})).toBeDefined();
    fireEvent.keyDown(dialog, {key: " "});
    expect(screen.getByRole("button", {name: "Pause"})).toBeDefined();
    const requestFullscreen = vi.fn(async () => undefined);
    Object.defineProperty(dialog, "requestFullscreen", {configurable: true, value: requestFullscreen});
    fireEvent.click(screen.getByRole("button", {name: "Enter fullscreen"}));
    await waitFor(() => expect(requestFullscreen).toHaveBeenCalledTimes(1));
    expect(await screen.findByRole("button", {name: "Exit fullscreen"})).toBeDefined();
  });

  it("presents the complete revision even when the conversation page is partial", async () => {
    const unloadedEntry: SelectableEntryV1 = {
      entry: {
        entryKey: "entry_unloaded",
        ordinal: 1,
        atMs: 1_500,
        kind: "assistant",
        markdown: "Loaded directly into the complete presentation plan",
      },
      selected: true,
    };
    const partialDetail = detail(codexSummary, [entries[0]!], "entry_user");
    const fullDetail = detail(codexSummary, [entries[0]!, unloadedEntry]);
    const createPresentationPlan = vi.fn<IndexedSessionClient["createPresentationPlan"]>(async () =>
      projectPresentationPlan(fullDetail, fullDetail.entryPage.entries, "plan_complete", 3_000),
    );
    const {client} = clientWith({
      getSession: vi.fn(async () => partialDetail),
      createPresentationPlan,
    });
    render(<SessionBrowser client={client} />);
    fireEvent.click(
      await screen.findByRole("button", {name: "Open Indexed API cleanup from Codex CLI"}),
    );

    fireEvent.click(await screen.findByRole("button", {name: "Present"}));
    const dialog = await screen.findByRole("dialog", {name: "Indexed API cleanup"});
    expect(dialog.textContent).toContain("1 / 2");
    fireEvent.keyDown(dialog, {key: "ArrowRight"});
    expect(screen.getByText("Loaded directly into the complete presentation plan")).toBeDefined();
    expect(createPresentationPlan).toHaveBeenCalledWith(
      "session_codex",
      "revision_session_codex",
    );
  });

  it("uses a viewport presentation with monospace controls and scrollable entries", async () => {
    const {client} = clientWith();
    const {container} = render(<SessionBrowser client={client} />);
    fireEvent.click(
      await screen.findByRole("button", {name: "Open Indexed API cleanup from Codex CLI"}),
    );
    fireEvent.click(await screen.findByRole("button", {name: "Present"}));
    const dialog = await screen.findByRole("dialog", {name: "Indexed API cleanup"});
    const card = container.querySelector<HTMLElement>(".presentation-stage .shared-entry-card");

    expect(card?.style.overflowY).toBe("auto");
    expect(card?.tabIndex).toBe(0);
    expect(applicationStyles).toMatch(
      /\.presentation-dialog\s*\{[^}]*width:\s*100vw;[^}]*height:\s*100vh;[^}]*border-radius:\s*0;[^}]*font-family:\s*"JetBrains Mono"[^;]*;/s,
    );
    expect(applicationStyles).toMatch(
      /\.presentation-controls button\s*\{[^}]*display:\s*inline-flex;[^}]*align-items:\s*center;[^}]*justify-content:\s*center;/s,
    );
    expect(dialog.classList.contains("presentation-dialog")).toBe(true);
  });

  it("keeps reduced-motion presentations manually navigable without autoplay", async () => {
    const originalMatchMedia = window.matchMedia;
    Object.defineProperty(window, "matchMedia", {
      configurable: true,
      value: vi.fn(() => ({
        matches: true,
        media: "(prefers-reduced-motion: reduce)",
        onchange: null,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
        addListener: vi.fn(),
        removeListener: vi.fn(),
        dispatchEvent: vi.fn(() => true),
      })),
    });
    try {
      const {client} = clientWith();
      render(<SessionBrowser client={client} />);
      fireEvent.click(await screen.findByRole("button", {name: "Open Indexed API cleanup from Codex CLI"}));
      fireEvent.click(await screen.findByRole("button", {name: "Present"}));

      expect(await screen.findByText("Reduced motion active")).toBeDefined();
      expect(screen.getByRole("button", {name: "Play"})).toHaveProperty("disabled", true);
      fireEvent.click(screen.getByRole("button", {name: "Next entry"}));
      expect(screen.getByRole("dialog").textContent).toContain("2 / 2");
    } finally {
      Object.defineProperty(window, "matchMedia", {
        configurable: true,
        value: originalMatchMedia,
      });
    }
  });

  it("announces and focuses a newly selected bounded entry window", async () => {
    const manyEntries: readonly SelectableEntryV1[] = Array.from({length: 101}, (_, ordinal) => ({
      entry: {
        entryKey: `entry_${ordinal}`,
        ordinal,
        atMs: ordinal,
        kind: "user" as const,
        text: `Entry ${ordinal + 1}`,
      },
      selected: true,
    }));
    const summary = {...codexSummary, entryCount: 101, selectedEntryCount: 101};
    const {client} = clientWith({getSession: vi.fn(async () => detail(summary, manyEntries))});
    render(<SessionBrowser client={client} />);
    fireEvent.click(await screen.findByRole("button", {name: "Open Indexed API cleanup from Codex CLI"}));
    expect(await screen.findByText("Showing 1–100 of 101 loaded")).toBeDefined();

    const nextEntries = screen.getByRole("button", {name: "Next entries"});
    expect(nextEntries).toHaveProperty("disabled", false);
    fireEvent.click(nextEntries);
    expect(await screen.findByText("Showing 101–101 of 101 loaded")).toBeDefined();
    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole("list", {name: "Conversation entries"})));
    expect(screen.queryByText("Entry 1")).toBeNull();
    expect(screen.getByText("Entry 101")).toBeDefined();
  });

  it("disables presentation with no selected entries and safely reports backend projection failure", async () => {
    const empty = detail({...codexSummary, selectedEntryCount: 0});
    const getSession = vi.fn<IndexedSessionClient["getSession"]>(async () => ({
      ...empty,
      entryPage: {
        ...empty.entryPage,
        entries: empty.entryPage.entries.map((item) => ({...item, selected: false})),
      },
    }));
    const {client} = clientWith({getSession});
    const view = render(<SessionBrowser client={client} />);
    fireEvent.click(await screen.findByRole("button", {name: "Open Indexed API cleanup from Codex CLI"}));
    expect(await screen.findByRole("button", {name: "Present"})).toHaveProperty("disabled", true);

    view.unmount();
    const createPresentationPlan = vi.fn<IndexedSessionClient["createPresentationPlan"]>(async () => {
      throw new Error("C:\\private\\plan failure");
    });
    const second = clientWith({createPresentationPlan});
    render(<SessionBrowser client={second.client} />);
    fireEvent.click(await screen.findByRole("button", {name: "Open Indexed API cleanup from Codex CLI"}));
    fireEvent.click(await screen.findByRole("button", {name: "Present"}));
    expect(await screen.findByRole("alert")).toHaveProperty(
      "textContent",
      expect.stringContaining("Presentation could not be created"),
    );
    expect(document.body.textContent).not.toContain("C:\\private");
  });

  it("ignores a presentation plan that resolves after the selected session changes", async () => {
    const pendingPlan = deferred<Awaited<ReturnType<IndexedSessionClient["createPresentationPlan"]>>>();
    const getSession = vi.fn<IndexedSessionClient["getSession"]>(async (sessionId) =>
      detail(sessionId === claudeSummary.sessionId ? claudeSummary : codexSummary));
    const {client} = clientWith({
      getSession,
      createPresentationPlan: vi.fn(() => pendingPlan.promise),
    });
    render(<SessionBrowser client={client} />);
    fireEvent.click(await screen.findByRole("button", {name: "Open Indexed API cleanup from Codex CLI"}));
    fireEvent.click(await screen.findByRole("button", {name: "Present"}));
    expect(screen.getByRole("checkbox", {name: "Exclude You entry"})).toHaveProperty("disabled", true);
    fireEvent.click(screen.getByRole("button", {name: "Open Checkout redesign from Claude Code"}));
    await screen.findByRole("heading", {name: "Checkout redesign"});

    const stalePlan = projectPresentationPlan(
      detail(codexSummary),
      detail(codexSummary).entryPage.entries,
      "plan_stale",
      3_000,
    );
    await act(async () => pendingPlan.resolve(stalePlan));
    expect(screen.queryByRole("dialog", {name: "Indexed API cleanup"})).toBeNull();
  });

  it("exports only after an explicit destination confirmation", async () => {
    const exportPresentation = vi.fn<IndexedSessionClient["exportPresentation"]>(async () => ({
      codec: "h264", width: 1_920, height: 1_080, pixelFormat: "yuv420p", durationMs: 2_000, frameCount: 60,
    }));
    vi.mocked(save).mockResolvedValue("D:\\exports\\session.mp4");
    const revealExport = vi.fn<IndexedSessionClient["revealExport"]>(async () => undefined);
    const {client} = clientWith({exportPresentation, revealExport});
    render(<SessionBrowser client={client} />);
    fireEvent.click(await screen.findByRole("button", {name: "Open Indexed API cleanup from Codex CLI"}));
    await screen.findByRole("heading", {name: "Indexed API cleanup"});
    expect(exportPresentation).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", {name: "Present"}));
    fireEvent.click(await screen.findByRole("button", {name: "Export MP4"}));

    await waitFor(() => expect(exportPresentation).toHaveBeenCalledWith(
      "plan_test",
      "D:\\exports\\session.mp4",
      expect.stringMatching(/^export_/),
    ));
    expect(save).toHaveBeenCalledWith(expect.objectContaining({
      defaultPath: expect.stringMatching(/^session-replay-[a-z0-9]+\.mp4$/),
    }));
    expect(await screen.findByText("Export complete")).toBeDefined();
    fireEvent.click(screen.getByRole("button", {name: "Show in folder"}));
    expect(revealExport).toHaveBeenCalledWith(expect.stringMatching(/^export_/));
  });

  it("exports directly from the main session view", async () => {
    const exportPresentation = vi.fn<IndexedSessionClient["exportPresentation"]>(async () => ({
      codec: "h264", width: 1_920, height: 1_080, pixelFormat: "yuv420p", durationMs: 2_000, frameCount: 60,
    }));
    vi.mocked(save).mockResolvedValue("D:\\exports\\main-session.mp4");
    const revealExport = vi.fn<IndexedSessionClient["revealExport"]>(async () => undefined);
    const {client} = clientWith({exportPresentation, revealExport});
    render(<SessionBrowser client={client} />);
    fireEvent.click(
      await screen.findByRole("button", {name: "Open Indexed API cleanup from Codex CLI"}),
    );

    fireEvent.click(await screen.findByRole("button", {name: "Export session as MP4"}));

    await waitFor(() => expect(exportPresentation).toHaveBeenCalledWith(
      "plan_test",
      "D:\\exports\\main-session.mp4",
      expect.stringMatching(/^export_/),
    ));
    expect(screen.queryByRole("dialog", {name: "Indexed API cleanup"})).toBeNull();
    expect(await screen.findByText("Export complete")).toBeDefined();
    fireEvent.click(screen.getByRole("button", {name: "Show in folder"}));
    expect(revealExport).toHaveBeenCalledWith(expect.stringMatching(/^export_/));
  });

  it("shows main-view export progress and actions in a neutral operation bar", async () => {
    const pending = deferred<Awaited<ReturnType<IndexedSessionClient["exportPresentation"]>>>();
    vi.mocked(save).mockResolvedValue("D:\\exports\\busy-session.mp4");
    const {client} = clientWith({
      exportPresentation: vi.fn(() => pending.promise),
    });
    const {container} = render(<SessionBrowser client={client} />);
    fireEvent.click(
      await screen.findByRole("button", {name: "Open Indexed API cleanup from Codex CLI"}),
    );
    fireEvent.click(await screen.findByRole("button", {name: "Export session as MP4"}));

    const operationBar = await waitFor(() => {
      const candidate = container.querySelector<HTMLElement>(".conversation__operation-bar");
      expect(candidate).not.toBeNull();
      return candidate!;
    });
    expect(operationBar.textContent).toContain("Preparing renderer…");
    expect(operationBar.querySelector("button")?.textContent).toBe("Cancel export");
    expect(operationBar.classList.contains("conversation__operation-bar--error")).toBe(false);
    expect(screen.queryByRole("alert")).toBeNull();

    await act(async () => pending.resolve({
      codec: "h264", width: 1_920, height: 1_080, pixelFormat: "yuv420p", durationMs: 2_000, frameCount: 60,
    }));
  });

  it("shows validated export progress without starting another job", async () => {
    const pending = deferred<Awaited<ReturnType<IndexedSessionClient["exportPresentation"]>>>();
    const unsubscribe = vi.fn();
    let progressObserver: Parameters<IndexedSessionClient["subscribeToExportProgress"]>[1] | undefined;
    const exportPresentation = vi.fn<IndexedSessionClient["exportPresentation"]>(() => pending.promise);
    const subscribeToExportProgress = vi.fn<IndexedSessionClient["subscribeToExportProgress"]>(
      async (_jobId, observer) => {
        progressObserver = observer;
        return unsubscribe;
      },
    );
    vi.mocked(save).mockResolvedValue("D:\\exports\\session.mp4");
    const {client} = clientWith({exportPresentation, subscribeToExportProgress});
    render(<SessionBrowser client={client} />);
    fireEvent.click(await screen.findByRole("button", {name: "Open Indexed API cleanup from Codex CLI"}));
    await screen.findByRole("heading", {name: "Indexed API cleanup"});
    fireEvent.click(screen.getByRole("button", {name: "Present"}));
    fireEvent.click(await screen.findByRole("button", {name: "Export MP4"}));

    await waitFor(() => expect(progressObserver).toBeDefined());
    act(() => progressObserver?.({schemaVersion: 1, jobId: "export_test", renderedFrames: 30, totalFrames: 60}));
    expect(screen.getByText("Exporting 50%")).toBeDefined();
    expect(exportPresentation).toHaveBeenCalledTimes(1);

    await act(async () => pending.resolve({
      codec: "h264", width: 1_920, height: 1_080, pixelFormat: "yuv420p", durationMs: 2_000, frameCount: 60,
    }));
    expect(unsubscribe).toHaveBeenCalledTimes(1);
  });

  it("cancels an active export explicitly and reports cancellation separately", async () => {
    const pending = deferred<Awaited<ReturnType<IndexedSessionClient["exportPresentation"]>>>();
    const cancelExport = vi.fn<IndexedSessionClient["cancelExport"]>(async () => undefined);
    vi.mocked(save).mockResolvedValue("D:\\exports\\session.mp4");
    const {client} = clientWith({
      cancelExport,
      exportPresentation: vi.fn(() => pending.promise),
    });
    render(<SessionBrowser client={client} />);
    fireEvent.click(await screen.findByRole("button", {name: "Open Indexed API cleanup from Codex CLI"}));
    fireEvent.click(await screen.findByRole("button", {name: "Present"}));
    fireEvent.click(await screen.findByRole("button", {name: "Export MP4"}));
    fireEvent.click(await screen.findByRole("button", {name: "Cancel export"}));
    expect(cancelExport).toHaveBeenCalledWith(expect.stringMatching(/^export_/));

    await act(async () => pending.reject({code: "EXPORT_CANCELLED"}));
    expect(await screen.findByText("Export cancelled")).toBeDefined();
  });

  it("keeps an export tracked when a cancellation request fails", async () => {
    const pending = deferred<Awaited<ReturnType<IndexedSessionClient["exportPresentation"]>>>();
    const cancelExport = vi.fn<IndexedSessionClient["cancelExport"]>(async () => {
      throw new Error("cancel unavailable");
    });
    vi.mocked(save).mockResolvedValue("D:\\exports\\session.mp4");
    const {client} = clientWith({cancelExport, exportPresentation: vi.fn(() => pending.promise)});
    render(<SessionBrowser client={client} />);
    fireEvent.click(await screen.findByRole("button", {name: "Open Indexed API cleanup from Codex CLI"}));
    fireEvent.click(await screen.findByRole("button", {name: "Present"}));
    fireEvent.click(await screen.findByRole("button", {name: "Export MP4"}));
    fireEvent.click(await screen.findByRole("button", {name: "Cancel export"}));

    expect(await screen.findByText("Cancellation unavailable; export continues")).toBeDefined();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getByRole("button", {name: "Exporting…"})).toHaveProperty("disabled", true);
    await act(async () => pending.resolve({
      codec: "h264", width: 1_920, height: 1_080, pixelFormat: "yuv420p", durationMs: 2_000, frameCount: 60,
    }));
  });

  it("explains an existing MP4 destination instead of reporting a generic failure", async () => {
    vi.mocked(save).mockResolvedValue("D:\\exports\\existing.mp4");
    const {client} = clientWith({
      exportPresentation: vi.fn(async () => Promise.reject({code: "EXPORT_OUTPUT_EXISTS"})),
    });
    render(<SessionBrowser client={client} />);
    fireEvent.click(
      await screen.findByRole("button", {name: "Open Indexed API cleanup from Codex CLI"}),
    );
    fireEvent.click(await screen.findByRole("button", {name: "Present"}));
    fireEvent.click(await screen.findByRole("button", {name: "Export MP4"}));

    expect(
      await screen.findByText("That MP4 already exists. Choose another filename; existing videos are never overwritten."),
    ).toBeDefined();
    expect(screen.getByRole("alert").classList.contains("toast-notification")).toBe(true);
    fireEvent.click(screen.getByRole("button", {name: "Dismiss notification"}));
    expect(screen.queryByText("That MP4 already exists. Choose another filename; existing videos are never overwritten.")).toBeNull();
  });

  it("optimistically persists selection, visibility, timing, and appearance", async () => {
    const setEntrySelections = vi.fn<IndexedSessionClient["setEntrySelections"]>(async (request) => ({
      ...detail(codexSummary),
      summary: {...codexSummary, selectedEntryCount: request.changes[0]?.selected ? 2 : 1},
    }));
    const setSessionPreferences = vi.fn<IndexedSessionClient["setSessionPreferences"]>(
      async (request) => ({...detail(codexSummary), preferences: request.preferences}),
    );
    const {client} = clientWith({setEntrySelections, setSessionPreferences});
    render(<SessionBrowser client={client} />);
    fireEvent.click(
      await screen.findByRole("button", {name: "Open Indexed API cleanup from Codex CLI"}),
    );
    await screen.findByRole("heading", {name: "Indexed API cleanup"});

    fireEvent.click(screen.getByRole("checkbox", {name: "Exclude You entry"}));
    await waitFor(() => expect(setEntrySelections).toHaveBeenCalledWith({
        schemaVersion: 1,
        sessionId: "session_codex",
        revisionId: "revision_session_codex",
        changes: [{entryKey: "entry_user", selected: false}],
      }));
    expect(screen.getByText("Excluded")).toBeDefined();

    fireEvent.click(screen.getByRole("checkbox", {name: "Reasoning"}));
    await waitFor(() => expect(setSessionPreferences).toHaveBeenCalledWith(
        expect.objectContaining({
          sessionId: "session_codex",
          revisionId: "revision_session_codex",
          preferences: expect.objectContaining({
            visibility: expect.objectContaining({showReasoning: false}),
          }),
        }),
      ));

    fireEvent.change(screen.getByRole("combobox", {name: "Playback speed"}), {target: {value: "2"}});
    await waitFor(() => expect(setSessionPreferences).toHaveBeenLastCalledWith(
      expect.objectContaining({
        preferences: expect.objectContaining({timing: {entryDelayMs: 1_000, playbackSpeed: 2}}),
      }),
    ));

    fireEvent.change(screen.getByRole("combobox", {name: "Presentation palette"}), {target: {value: "midnight"}});
    await waitFor(() => expect(setSessionPreferences).toHaveBeenLastCalledWith(
      expect.objectContaining({
        preferences: expect.objectContaining({
          appearance: expect.objectContaining({theme: expect.objectContaining({background: "#080c16"})}),
        }),
      }),
    ));
  });

  it("navigates conversation entries with arrows and toggles selection with space", async () => {
    const setEntrySelections = vi.fn<IndexedSessionClient["setEntrySelections"]>(async (request) => ({
      ...detail(codexSummary),
      summary: {...codexSummary, selectedEntryCount: request.changes[0]?.selected ? 2 : 1},
    }));
    const {client} = clientWith({setEntrySelections});
    const {container} = render(<SessionBrowser client={client} />);
    fireEvent.click(
      await screen.findByRole("button", {name: "Open Indexed API cleanup from Codex CLI"}),
    );
    await screen.findByRole("heading", {name: "Indexed API cleanup"});
    const conversationEntries = [...container.querySelectorAll<HTMLElement>(".conversation-entry")];

    await waitFor(() => expect(document.activeElement).toBe(conversationEntries[0]));
    expect(conversationEntries[0]!.querySelector("article")?.getAttribute("data-active")).toBe("true");
    expect(conversationEntries[1]!.querySelector("article")?.getAttribute("data-active")).toBe("false");
    fireEvent.keyDown(conversationEntries[0]!, {key: "ArrowDown"});
    expect(document.activeElement).toBe(conversationEntries[1]);
    expect(conversationEntries[0]!.querySelector("article")?.getAttribute("data-active")).toBe("false");
    expect(conversationEntries[1]!.querySelector("article")?.getAttribute("data-active")).toBe("true");
    const secondCheckbox = conversationEntries[1]!.querySelector<HTMLInputElement>('input[type="checkbox"]')!;
    secondCheckbox.focus();
    fireEvent.keyDown(secondCheckbox, {key: "ArrowLeft"});
    expect(document.activeElement).toBe(conversationEntries[0]);
    fireEvent.keyDown(conversationEntries[0]!, {key: " "});

    await waitFor(() => expect(setEntrySelections).toHaveBeenCalledWith({
      schemaVersion: 1,
      sessionId: "session_codex",
      revisionId: "revision_session_codex",
      changes: [{entryKey: "entry_user", selected: false}],
    }));
    expect(document.activeElement).toBe(conversationEntries[0]);
    expect(screen.getByText("Use arrow keys to move; press Space to include or exclude.")).toBeDefined();
  });

  it("keeps entry selection visually stable while the choice is being saved", async () => {
    const pending = deferred<IndexedSessionDetailV1>();
    const setEntrySelections = vi.fn<IndexedSessionClient["setEntrySelections"]>(
      async () => pending.promise,
    );
    const {client} = clientWith({setEntrySelections});
    const {container} = render(<SessionBrowser client={client} />);
    fireEvent.click(
      await screen.findByRole("button", {name: "Open Indexed API cleanup from Codex CLI"}),
    );
    await screen.findByRole("heading", {name: "Indexed API cleanup"});
    const entryElements = [...container.querySelectorAll<HTMLElement>(".conversation-entry")];
    await waitFor(() => expect(document.activeElement).toBe(entryElements[0]));
    const firstCheckbox = screen.getByRole<HTMLInputElement>("checkbox", {name: "Exclude You entry"});
    const secondCheckbox = screen.getByRole<HTMLInputElement>("checkbox", {name: "Exclude Assistant entry"});

    fireEvent.click(firstCheckbox);

    expect(firstCheckbox.checked).toBe(false);
    expect(secondCheckbox.disabled).toBe(false);
    expect(entryElements[0]?.dataset.active).toBe("true");
    expect(entryElements[0]?.getAttribute("aria-current")).toBe("true");
    expect(applicationStyles).toMatch(
      /\.conversation-entry\[data-active="true"\]\s*\{[^}]*background:[^;}]+;[^}]*box-shadow:\s*inset 4px 0 0 var\(--selection\);/s,
    );

    await act(async () => pending.resolve(detail(
      {...codexSummary, selectedEntryCount: 1},
      entries.map((item) => item.entry.entryKey === "entry_user"
        ? {...item, selected: false}
        : item),
    )));
    await waitFor(() => expect(setEntrySelections).toHaveBeenCalledTimes(1));
    expect(firstCheckbox.checked).toBe(false);
  });

  it("bulk-selects and excludes every loaded entry with visible confirmation", async () => {
    const setEntrySelections = vi.fn<IndexedSessionClient["setEntrySelections"]>(async (request) => {
      const selected = request.changes[0]?.selected ?? true;
      return detail(
        {...codexSummary, selectedEntryCount: selected ? 2 : 0},
        entries.map((item) => ({...item, selected})),
      );
    });
    const {client} = clientWith({setEntrySelections});
    render(<SessionBrowser client={client} />);
    fireEvent.click(
      await screen.findByRole("button", {name: "Open Indexed API cleanup from Codex CLI"}),
    );

    fireEvent.click(await screen.findByRole("button", {name: "Exclude loaded"}));
    await waitFor(() => expect(setEntrySelections).toHaveBeenLastCalledWith({
      schemaVersion: 1,
      sessionId: "session_codex",
      revisionId: "revision_session_codex",
      changes: [
        {entryKey: "entry_user", selected: false},
        {entryKey: "entry_assistant", selected: false},
      ],
    }));
    expect(await screen.findByText("All loaded entries excluded.")).toBeDefined();
    expect(screen.getAllByText("Excluded")).toHaveLength(2);

    fireEvent.click(screen.getByRole("button", {name: "Select loaded"}));
    await waitFor(() => expect(setEntrySelections).toHaveBeenLastCalledWith({
      schemaVersion: 1,
      sessionId: "session_codex",
      revisionId: "revision_session_codex",
      changes: [
        {entryKey: "entry_user", selected: true},
        {entryKey: "entry_assistant", selected: true},
      ],
    }));
    expect(await screen.findByText("All loaded entries included.")).toBeDefined();
    expect(screen.queryByText("Excluded")).toBeNull();
  });

  it("renames a session locally and updates both the list and conversation", async () => {
    const renameSession = vi.fn<IndexedSessionClient["renameSession"]>(async (request) =>
      detail({...codexSummary, title: request.title}),
    );
    const {client} = clientWith({renameSession});
    render(<SessionBrowser client={client} />);
    fireEvent.click(
      await screen.findByRole("button", {name: "Open Indexed API cleanup from Codex CLI"}),
    );
    await screen.findByRole("heading", {name: "Indexed API cleanup"});

    fireEvent.click(screen.getByRole("button", {name: "Rename session"}));
    const input = await screen.findByRole("textbox", {name: "Session name"});
    expect(document.activeElement).toBe(input);
    fireEvent.change(input, {target: {value: "  Release walkthrough  "}});
    fireEvent.submit(input.closest("form")!);

    await waitFor(() => expect(renameSession).toHaveBeenCalledWith({
      schemaVersion: 1,
      sessionId: "session_codex",
      title: "Release walkthrough",
    }));
    expect(screen.getByRole("heading", {name: "Release walkthrough"})).toBeDefined();
    expect(screen.getByRole("button", {name: "Open Release walkthrough from Codex CLI"})).toBeDefined();
    expect(screen.getByText("Session renamed.")).toBeDefined();
  });

  it("rolls back an optimistic selection when persistence fails", async () => {
    const setEntrySelections = vi.fn<IndexedSessionClient["setEntrySelections"]>(async () => {
      throw new Error("C:\\private\\database failure");
    });
    const {client} = clientWith({setEntrySelections});
    render(<SessionBrowser client={client} />);
    fireEvent.click(
      await screen.findByRole("button", {name: "Open Indexed API cleanup from Codex CLI"}),
    );
    await screen.findByRole("heading", {name: "Indexed API cleanup"});

    fireEvent.click(screen.getByRole("checkbox", {name: "Exclude You entry"}));

    expect(await screen.findByRole("alert")).toHaveProperty(
      "textContent",
      expect.stringContaining("saved choices were restored or reloaded"),
    );
    expect(screen.getByRole("checkbox", {name: "Exclude You entry"})).toHaveProperty(
      "checked",
      true,
    );
    expect(document.body.textContent).not.toContain("C:\\private");
  });

  it("reloads the current revision after a stale curation write", async () => {
    const original = detail(codexSummary);
    const refreshed = {
      ...original,
      revision: {...original.revision, revisionId: "revision_new", indexedAtMs: 4_000},
      entryPage: {...original.entryPage, revisionId: "revision_new"},
    };
    const getSession = vi
      .fn<IndexedSessionClient["getSession"]>()
      .mockResolvedValueOnce(original)
      .mockResolvedValueOnce(refreshed);
    const setEntrySelections = vi.fn<IndexedSessionClient["setEntrySelections"]>(async () => {
      throw {code: "STALE_REVISION"};
    });
    const {client} = clientWith({getSession, setEntrySelections});
    render(<SessionBrowser client={client} />);
    fireEvent.click(await screen.findByRole("button", {name: "Open Indexed API cleanup from Codex CLI"}));
    fireEvent.click(await screen.findByRole("checkbox", {name: "Exclude You entry"}));

    await waitFor(() => expect(getSession).toHaveBeenCalledTimes(2));
    expect(document.body.textContent).toContain("Jan 1, 1970");
    expect(screen.getByRole("checkbox", {name: "Exclude You entry"})).toHaveProperty(
      "checked",
      true,
    );
  });

  it("requires confirmation before resetting app-owned data and clears the visible library", async () => {
    const resetLocalDatabase = vi.fn<IndexedSessionClient["resetLocalDatabase"]>(async () => ({
      schemaVersion: 1,
      resetAtMs: 3_000,
    }));
    const {client} = clientWith({resetLocalDatabase});
    render(<SessionBrowser client={client} />);
    await screen.findByText("Indexed API cleanup");

    fireEvent.click(screen.getByRole("button", {name: "Open library settings"}));
    expect(screen.getByRole("heading", {name: "Library settings"})).toBeDefined();
    expect(document.body.textContent).toContain("Original source files stay untouched");
    fireEvent.click(screen.getByRole("button", {name: "Reset local database"}));
    expect(resetLocalDatabase).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(screen.getByRole("button", {name: "Go back"}));

    fireEvent.click(screen.getByRole("button", {name: "Permanently reset local database"}));
    await waitFor(() => expect(resetLocalDatabase).toHaveBeenCalledTimes(1));
    expect(await screen.findByText("No indexed sessions yet")).toBeDefined();
    expect(
      screen.getByText("Local library reset. Refresh to index source sessions again."),
    ).toBeDefined();
    expect(client.deleteSession).not.toHaveBeenCalled();
    expect(client.deleteSessionWithSource).not.toHaveBeenCalled();
  });

  it("does not let an older list response repopulate a reset library", async () => {
    const staleList = deferred<IndexedSessionPageV1>();
    const listSessions = vi
      .fn<IndexedSessionClient["listSessions"]>()
      .mockResolvedValueOnce({
        schemaVersion: 1,
        sortOrder: "newest",
        items: [codexSummary],
        nextCursor: null,
      })
      .mockImplementationOnce(() => staleList.promise);
    const {client} = clientWith({listSessions});
    render(<SessionBrowser client={client} />);

    await screen.findByText("Indexed API cleanup");
    fireEvent.change(screen.getByRole("combobox", {name: "Sort sessions"}), {
      target: {value: "oldest"},
    });
    await waitFor(() => expect(listSessions).toHaveBeenCalledTimes(2));
    fireEvent.click(screen.getByRole("button", {name: "Open library settings"}));
    fireEvent.click(screen.getByRole("button", {name: "Reset local database"}));
    fireEvent.click(screen.getByRole("button", {name: "Permanently reset local database"}));
    expect(await screen.findByText("No indexed sessions yet")).toBeDefined();

    await act(async () =>
      staleList.resolve({
        schemaVersion: 1,
        sortOrder: "newest",
        items: [codexSummary],
        nextCursor: null,
      }),
    );
    expect(screen.queryByText("Indexed API cleanup")).toBeNull();
  });

  it("disables reset during refresh and retains the library after reset failure", async () => {
    const resetLocalDatabase = vi.fn<IndexedSessionClient["resetLocalDatabase"]>(async () => {
      throw new Error("C:\\private\\session-library.sqlite3 is locked");
    });
    const {client, emit} = clientWith({resetLocalDatabase});
    render(<SessionBrowser client={client} />);
    await screen.findByText("Indexed API cleanup");
    act(() =>
      emit({
        schemaVersion: 1,
        status: "running",
        generation: 1,
        startedAtMs: 1_000,
        discoveredCount: 1,
        processedCount: 0,
        indexedCount: 0,
        unchangedCount: 0,
        failedCount: 0,
        skippedCount: 0,
        warningCount: 0,
      }),
    );

    fireEvent.click(screen.getByRole("button", {name: "Open library settings"}));
    expect(
      (screen.getByRole("button", {name: "Reset local database"}) as HTMLButtonElement).disabled,
    ).toBe(true);
    expect(screen.getByText("Wait for the current refresh to finish.")).toBeDefined();
    fireEvent.click(screen.getByRole("button", {name: "Done"}));
    act(() =>
      emit({
        schemaVersion: 1,
        status: "completed",
        generation: 1,
        startedAtMs: 1_000,
        completedAtMs: 2_000,
        discoveredCount: 1,
        processedCount: 1,
        indexedCount: 0,
        unchangedCount: 1,
        failedCount: 0,
        skippedCount: 0,
        warningCount: 0,
      }),
    );

    fireEvent.click(screen.getByRole("button", {name: "Open library settings"}));
    fireEvent.click(screen.getByRole("button", {name: "Reset local database"}));
    fireEvent.click(screen.getByRole("button", {name: "Permanently reset local database"}));
    expect(await screen.findByRole("alert")).toHaveProperty(
      "textContent",
      expect.stringContaining("Existing data was retained"),
    );
    expect(screen.getByText("Indexed API cleanup")).toBeDefined();
    expect(document.body.textContent).not.toContain("C:\\private");
  });

  it("deletes only the indexed copy by default without preparing source deletion", async () => {
    const deleteSession = vi.fn<IndexedSessionClient["deleteSession"]>(async () => ({
      schemaVersion: 1,
      suppressionId: "suppression_1",
      source: "claude-code",
      sourceDeleted: false,
      suppressedAtMs: 2_000,
    }));
    const {client} = clientWith({deleteSession});
    render(<SessionBrowser client={client} />);
    fireEvent.click(
      await screen.findByRole("button", {name: "Open Checkout redesign from Claude Code"}),
    );
    await screen.findByRole("heading", {name: "Checkout redesign"});

    fireEvent.click(screen.getByRole("button", {name: "Hide session"}));
    const sourceOption = screen.getByRole("checkbox", {
      name: "Also delete this session from Claude Code",
    });
    expect((sourceOption as HTMLInputElement).checked).toBe(false);
    const cancel = screen.getByRole("button", {name: "Cancel"});
    expect(document.activeElement).toBe(cancel);
    fireEvent.click(screen.getByRole("button", {name: "Hide from library"}));

    await waitFor(() => expect(deleteSession).toHaveBeenCalledWith("session_claude"));
    expect(client.prepareSourceDeletion).not.toHaveBeenCalled();
    expect(client.deleteSessionWithSource).not.toHaveBeenCalled();
    expect(await screen.findByRole("heading", {name: "Select a session"})).toBeDefined();
    expect(screen.queryByText("Checkout redesign")).toBeNull();
  });

  it("does not let an older list response resurrect a just-deleted row", async () => {
    const staleList = deferred<IndexedSessionPageV1>();
    const listSessions = vi
      .fn<IndexedSessionClient["listSessions"]>()
      .mockResolvedValueOnce({
        schemaVersion: 1,
        sortOrder: "newest",
        items: [claudeSummary],
        nextCursor: null,
      })
      .mockImplementationOnce(() => staleList.promise);
    const {client} = clientWith({listSessions});
    render(<SessionBrowser client={client} />);
    fireEvent.click(
      await screen.findByRole("button", {name: "Open Checkout redesign from Claude Code"}),
    );
    await screen.findByRole("heading", {name: "Checkout redesign"});
    fireEvent.change(screen.getByRole("searchbox", {name: "Search sessions"}), {
      target: {value: "checkout"},
    });
    await waitFor(() => expect(listSessions).toHaveBeenCalledTimes(2));

    fireEvent.click(screen.getByRole("button", {name: "Hide session"}));
    fireEvent.click(screen.getByRole("button", {name: "Hide from library"}));
    expect(await screen.findByRole("heading", {name: "Select a session"})).toBeDefined();

    await act(async () => {
      staleList.resolve({schemaVersion: 1, sortOrder: "newest", items: [claudeSummary], nextCursor: null});
    });
    expect(screen.queryByText("Checkout redesign")).toBeNull();
  });

  it("requires a separate impact review before deleting source artifacts", async () => {
    const prepareSourceDeletion = vi.fn<IndexedSessionClient["prepareSourceDeletion"]>(
      async () => ({
        schemaVersion: 1,
        sessionId: "session_claude",
        confirmationToken: "delete_token_claude",
        artifactCount: 3,
        expiresAtMs: 302_000,
      }),
    );
    const deleteSessionWithSource = vi.fn<IndexedSessionClient["deleteSessionWithSource"]>(
      async () => ({
        schemaVersion: 1,
        suppressionId: "suppression_1",
        source: "claude-code",
        sourceDeleted: true,
        suppressedAtMs: 2_000,
      }),
    );
    const {client} = clientWith({prepareSourceDeletion, deleteSessionWithSource});
    render(<SessionBrowser client={client} />);
    fireEvent.click(
      await screen.findByRole("button", {name: "Open Checkout redesign from Claude Code"}),
    );
    await screen.findByRole("heading", {name: "Checkout redesign"});

    fireEvent.click(screen.getByRole("button", {name: "Hide session"}));
    fireEvent.click(
      screen.getByRole("checkbox", {name: "Also delete this session from Claude Code"}),
    );
    fireEvent.click(screen.getByRole("button", {name: "Review source deletion"}));

    expect(await screen.findByText("3 source artifacts will be permanently deleted.")).toBeDefined();
    expect(prepareSourceDeletion).toHaveBeenCalledWith("session_claude");
    expect(deleteSessionWithSource).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", {name: "Delete source and library"}));
    await waitFor(() =>
      expect(deleteSessionWithSource).toHaveBeenCalledWith(
        "session_claude",
        "delete_token_claude",
      ),
    );
  });

  it("disables source deletion when only the indexed copy remains", async () => {
    const {client} = clientWith();
    render(<SessionBrowser client={client} />);
    fireEvent.click(
      await screen.findByRole("button", {name: "Open Indexed API cleanup from Codex CLI"}),
    );
    await screen.findByRole("heading", {name: "Indexed API cleanup"});

    fireEvent.click(screen.getByRole("button", {name: "Hide session"}));

    expect(
      (
        screen.getByRole("checkbox", {
          name: "Also delete this session from Codex CLI",
        }) as HTMLInputElement
      ).disabled,
    ).toBe(true);
    fireEvent.click(screen.getByRole("button", {name: "Cancel"}));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByRole("heading", {name: "Indexed API cleanup"})).toBeDefined();
  });

  it("requires a new source review after a confirmed deletion attempt fails", async () => {
    const deleteSessionWithSource = vi.fn<IndexedSessionClient["deleteSessionWithSource"]>(
      async () => {
        throw new Error("C:\\private\\source failure");
      },
    );
    const {client} = clientWith({
      prepareSourceDeletion: vi.fn<IndexedSessionClient["prepareSourceDeletion"]>(async () => ({
        schemaVersion: 1,
        sessionId: "session_claude",
        confirmationToken: "delete_token_claude",
        artifactCount: 1,
        expiresAtMs: 302_000,
      })),
      deleteSessionWithSource,
    });
    render(<SessionBrowser client={client} />);
    fireEvent.click(
      await screen.findByRole("button", {name: "Open Checkout redesign from Claude Code"}),
    );
    await screen.findByRole("heading", {name: "Checkout redesign"});
    fireEvent.click(screen.getByRole("button", {name: "Hide session"}));
    fireEvent.click(
      screen.getByRole("checkbox", {name: "Also delete this session from Claude Code"}),
    );
    fireEvent.click(screen.getByRole("button", {name: "Review source deletion"}));
    await screen.findByText("1 source artifact will be permanently deleted.");
    fireEvent.click(screen.getByRole("button", {name: "Delete source and library"}));

    expect(await screen.findByRole("alert")).toHaveProperty(
      "textContent",
      expect.stringContaining("Session could not be hidden"),
    );
    expect(screen.getByRole("button", {name: "Review source deletion"})).toBeDefined();
    expect(screen.queryByRole("button", {name: "Delete source and library"})).toBeNull();
    expect(document.body.textContent).not.toContain("C:\\private");
  });

  it("keeps the selected session available when deletion fails without exposing details", async () => {
    const {client} = clientWith({
      deleteSession: vi.fn(async () => {
        throw new Error("C:\\private\\session.jsonl could not be deleted");
      }),
    });
    render(<SessionBrowser client={client} />);
    fireEvent.click(
      await screen.findByRole("button", {name: "Open Checkout redesign from Claude Code"}),
    );
    await screen.findByRole("heading", {name: "Checkout redesign"});

    fireEvent.click(screen.getByRole("button", {name: "Hide session"}));
    fireEvent.click(screen.getByRole("button", {name: "Hide from library"}));

    expect(await screen.findByRole("alert")).toHaveProperty(
      "textContent",
      expect.stringContaining("Session could not be hidden"),
    );
    expect(screen.getByRole("heading", {name: "Checkout redesign"})).toBeDefined();
    expect(document.body.textContent).not.toContain("C:\\private");
  });

  it("lists path-free suppressed sources and restores one for a future refresh", async () => {
    const listSuppressed = vi.fn<IndexedSessionClient["listSuppressed"]>(async () => ({
      schemaVersion: 1,
      items: [
        {
          schemaVersion: 1,
          suppressionId: "suppression_1",
          source: "codex",
          sourceDeleted: false,
          suppressedAtMs: 2_000,
        },
      ],
      nextCursor: null,
    }));
    const restoreSuppressed = vi.fn<IndexedSessionClient["restoreSuppressed"]>(
      async (suppressionId) => ({schemaVersion: 1, suppressionId}),
    );
    const {client} = clientWith({listSuppressed, restoreSuppressed});
    render(<SessionBrowser client={client} />);
    await screen.findByText("Indexed API cleanup");

    fireEvent.click(screen.getByRole("button", {name: "Manage hidden sessions"}));
    expect(await screen.findByText("Source retained")).toBeDefined();
    expect(listSuppressed).toHaveBeenCalledWith({schemaVersion: 1, cursor: null, pageSize: 40});
    fireEvent.click(screen.getByRole("button", {name: "Restore Codex CLI source"}));

    await waitFor(() => expect(restoreSuppressed).toHaveBeenCalledWith("suppression_1"));
    expect(
      await screen.findByText("Source restored. Refresh to index it again."),
    ).toBeDefined();
    expect(screen.queryByText("Source retained")).toBeNull();
  });

  it("opens an indexed session after its source is gone and renders markup inertly", async () => {
    const {client} = clientWith();
    render(<SessionBrowser client={client} />);

    expect(screen.getByText("Preparing your local library")).toBeDefined();
    expect(document.querySelector(".app-loading-screen")).not.toBeNull();
    fireEvent.click(
      await screen.findByRole("button", {name: "Open Indexed API cleanup from Codex CLI"}),
    );

    expect(await screen.findByRole("heading", {name: "Indexed API cleanup"})).toBeDefined();
    expect(screen.getByText("Source unavailable — using indexed copy")).toBeDefined();
    expect(screen.getByText(/Render <script>/)).toBeDefined();
    expect(document.querySelector("script")).toBeNull();
    const conversationEntries = screen.getAllByRole("listitem").filter((item) =>
      item.classList.contains("conversation-entry"),
    );
    expect(document.querySelector(".conversation__chrome")).not.toBeNull();
    expect(document.querySelector(".conversation__scroll")?.contains(conversationEntries[0]!)).toBe(true);
    expect(applicationStyles).toMatch(/\.conversation__chrome\s*\{[^}]*position:\s*sticky;[^}]*top:\s*0;/s);
    expect(conversationEntries[0]?.getAttribute("aria-posinset")).toBe("1");
    expect(conversationEntries[0]?.getAttribute("aria-setsize")).toBe("2");
    expect(client.getSession).toHaveBeenCalledWith("session_codex", null);
  });

  it("renders contract-valid timestamps outside the JavaScript Date range safely", async () => {
    const distantSummary = {
      ...codexSummary,
      createdAtMs: Number.MAX_SAFE_INTEGER,
      lastIndexedAtMs: Number.MAX_SAFE_INTEGER,
    };
    const {client} = clientWith({
      listSessions: vi.fn<IndexedSessionClient["listSessions"]>(async () => ({
        schemaVersion: 1,
        sortOrder: "newest",
        items: [distantSummary],
        nextCursor: null,
      })),
      getSession: vi.fn(async () => detail(distantSummary)),
    });
    render(<SessionBrowser client={client} />);

    expect(await screen.findByText("Date unavailable")).toBeDefined();
    fireEvent.click(
      screen.getByRole("button", {name: "Open Indexed API cleanup from Codex CLI"}),
    );
    await waitFor(() =>
      expect(screen.getAllByText("Date unavailable")).toHaveLength(2),
    );
  });

  it("sends source and search filters to the bounded repository command", async () => {
    const {client} = clientWith();
    render(<SessionBrowser client={client} />);
    await screen.findByText("Indexed API cleanup");

    fireEvent.click(screen.getByRole("button", {name: "Filter Codex CLI sessions"}));
    await waitFor(() =>
      expect(client.listSessions).toHaveBeenLastCalledWith(
        expect.objectContaining({source: "codex", query: "", cursor: null, pageSize: 40}),
      ),
    );
    fireEvent.change(screen.getByRole("searchbox", {name: "Search sessions"}), {
      target: {value: "api"},
    });
    await waitFor(() =>
      expect(client.listSessions).toHaveBeenLastCalledWith(
        expect.objectContaining({source: "codex", query: "api", cursor: null, pageSize: 40}),
      ),
    );
    expect(screen.getByPlaceholderText("Search titles, conversations, or dates")).toBeDefined();
  });

  it("exposes and filters indexed GitHub Copilot CLI sessions", async () => {
    const listSessions = vi.fn<IndexedSessionClient["listSessions"]>(async (request) => ({
      schemaVersion: 1,
      sortOrder: request.sortOrder,
      items: request.source === "copilot-cli" ? [copilotSummary] : [codexSummary],
      nextCursor: null,
    }));
    const {client} = clientWith({listSessions});
    render(<SessionBrowser client={client} />);
    await screen.findByText("Indexed API cleanup");

    fireEvent.click(screen.getByRole("button", {name: "Filter GitHub Copilot CLI sessions"}));

    expect(await screen.findByText("GitHub Copilot CLI session")).toBeDefined();
    expect(listSessions).toHaveBeenLastCalledWith(
      expect.objectContaining({source: "copilot-cli", cursor: null}),
    );
  });

  it("exposes and filters indexed VS Code Copilot Chat sessions", async () => {
    const listSessions = vi.fn<IndexedSessionClient["listSessions"]>(async (request) => ({
      schemaVersion: 1,
      sortOrder: request.sortOrder,
      items: request.source === "vscode-copilot" ? [vscodeSummary] : [codexSummary],
      nextCursor: null,
    }));
    const {client} = clientWith({listSessions});
    render(<SessionBrowser client={client} />);
    await screen.findByText("Indexed API cleanup");

    fireEvent.click(screen.getByRole("button", {name: "Filter VS Code Copilot Chat sessions"}));

    expect(await screen.findByText("VS Code Copilot chat")).toBeDefined();
    expect(listSessions).toHaveBeenLastCalledWith(
      expect.objectContaining({source: "vscode-copilot", cursor: null}),
    );
  });

  it("reports JetBrains detection without presenting native chats as indexed sessions", async () => {
    const getJetBrainsStatus = vi.fn<IndexedSessionClient["getJetBrainsStatus"]>(async () => ({
      schemaVersion: 1,
      pluginStatus: "detected",
      nativeTranscriptStatus: "unsupported",
      copilotCliStatus: "available-separately",
    }));
    const {client} = clientWith({getJetBrainsStatus});
    render(<SessionBrowser client={client} />);

    const status = await screen.findByRole("region", {name: "JetBrains Copilot status"});
    expect(status.textContent).toContain("Plugin found · Copilot CLI separate");
    expect(status.textContent).toContain("Native chat transcripts unavailable");
    expect(screen.queryByRole("button", {name: /Filter JetBrains/})).toBeNull();
    expect(getJetBrainsStatus).toHaveBeenCalledTimes(1);
  });

  it("closes the displayed session when switching to another source", async () => {
    const {client} = clientWith();
    render(<SessionBrowser client={client} />);

    fireEvent.click(
      await screen.findByRole("button", {name: "Open Checkout redesign from Claude Code"}),
    );
    expect(await screen.findByRole("heading", {name: "Checkout redesign"})).toBeDefined();

    fireEvent.click(screen.getByRole("button", {name: "Filter Codex CLI sessions"}));

    expect(screen.getByRole("heading", {name: "Select a session"})).toBeDefined();
    expect(screen.queryByRole("heading", {name: "Checkout redesign"})).toBeNull();
  });

  it("does not reopen a pending session after switching to another source", async () => {
    const pendingDetail = deferred<IndexedSessionDetailV1>();
    const getSession = vi.fn<IndexedSessionClient["getSession"]>(() => pendingDetail.promise);
    const {client} = clientWith({getSession});
    render(<SessionBrowser client={client} />);

    fireEvent.click(
      await screen.findByRole("button", {name: "Open Checkout redesign from Claude Code"}),
    );
    expect(screen.getByText("Opening indexed conversation…")).toBeDefined();

    fireEvent.click(screen.getByRole("button", {name: "Filter Codex CLI sessions"}));
    expect(screen.getByRole("heading", {name: "Select a session"})).toBeDefined();

    await act(async () => pendingDetail.resolve(detail(claudeSummary)));
    expect(screen.getByRole("heading", {name: "Select a session"})).toBeDefined();
  });

  it("shows a search-specific empty state for full-text results", async () => {
    const listSessions = vi
      .fn<IndexedSessionClient["listSessions"]>()
      .mockResolvedValueOnce({
        schemaVersion: 1,
        sortOrder: "newest",
        items: [codexSummary],
        nextCursor: null,
      })
      .mockResolvedValue({
        schemaVersion: 1,
        sortOrder: "newest",
        items: [],
        nextCursor: null,
      });
    const {client} = clientWith({listSessions});
    render(<SessionBrowser client={client} />);
    await screen.findByText("Indexed API cleanup");

    fireEvent.change(screen.getByRole("searchbox", {name: "Search sessions"}), {
      target: {value: "conversation-only term"},
    });

    expect(await screen.findByText("No matching sessions")).toBeDefined();
    expect(screen.getByText("Try different words or clear the search.")).toBeDefined();
  });

  it("appends bounded session and entry pages", async () => {
    const nextSummary = {...claudeSummary, sessionId: "session_next", title: "Next page"};
    const nextEntry: SelectableEntryV1 = {
      entry: {
        entryKey: "entry_next",
        ordinal: 1,
        atMs: 1_500,
        kind: "assistant",
        markdown: "Second page",
      },
      selected: true,
    };
    const listSessions = vi
      .fn<IndexedSessionClient["listSessions"]>()
      .mockResolvedValueOnce({schemaVersion: 1, sortOrder: "newest", items: [codexSummary], nextCursor: "newest__session_codex"})
      .mockResolvedValueOnce({schemaVersion: 1, sortOrder: "newest", items: [nextSummary], nextCursor: null});
    const getSession = vi
      .fn<IndexedSessionClient["getSession"]>()
      .mockResolvedValueOnce(detail(codexSummary, [entries[0]!], "entry_user"))
      .mockResolvedValueOnce(detail(codexSummary, [nextEntry]));
    const {client} = clientWith({listSessions, getSession});
    render(<SessionBrowser client={client} />);

    await screen.findByText("Indexed API cleanup");
    fireEvent.click(screen.getByRole("button", {name: "Load more sessions"}));
    expect(await screen.findByText("Next page")).toBeDefined();
    expect(listSessions).toHaveBeenLastCalledWith(expect.objectContaining({cursor: "newest__session_codex"}));

    fireEvent.click(screen.getByRole("button", {name: "Open Indexed API cleanup from Codex CLI"}));
    await screen.findByText(/Render <script>/);
    fireEvent.click(screen.getByRole("button", {name: "Load more messages"}));
    expect(await screen.findByText("Second page")).toBeDefined();
    expect(getSession).toHaveBeenLastCalledWith("session_codex", "entry_user");
  });

  it("reloads the first page when the revision changes during message pagination", async () => {
    const pagedSummary = {...codexSummary, entryCount: 201, selectedEntryCount: 201};
    const initialEntry: SelectableEntryV1 = {
      entry: {
        entryKey: "entry_199",
        ordinal: 199,
        atMs: 199_000,
        kind: "assistant",
        markdown: "Current first page",
      },
      selected: true,
    };
    const staleCursorEntry: SelectableEntryV1 = {
      entry: {
        entryKey: "entry_old_tail",
        ordinal: 200,
        atMs: 200_000,
        kind: "assistant",
        markdown: "Weeks-old stale cursor page",
      },
      selected: true,
    };
    const refreshedEntry: SelectableEntryV1 = {
      entry: {
        entryKey: "entry_new_first",
        ordinal: 0,
        atMs: 0,
        kind: "user",
        text: "Latest revision first page",
      },
      selected: true,
    };
    const initial = detail(pagedSummary, [initialEntry], "entry_199");
    const changedRevisionPageBase = detail(pagedSummary, [staleCursorEntry]);
    const changedRevisionPage: IndexedSessionDetailV1 = {
      ...changedRevisionPageBase,
      revision: {...changedRevisionPageBase.revision, revisionId: "revision_new"},
      entryPage: {...changedRevisionPageBase.entryPage, revisionId: "revision_new"},
    };
    const changedRevisionFirstPageBase = detail(pagedSummary, [refreshedEntry], "entry_new_first");
    const changedRevisionFirstPage: IndexedSessionDetailV1 = {
      ...changedRevisionFirstPageBase,
      revision: {...changedRevisionFirstPageBase.revision, revisionId: "revision_new"},
      entryPage: {...changedRevisionFirstPageBase.entryPage, revisionId: "revision_new"},
    };
    const getSession = vi
      .fn<IndexedSessionClient["getSession"]>()
      .mockResolvedValueOnce(initial)
      .mockResolvedValueOnce(changedRevisionPage)
      .mockResolvedValueOnce(changedRevisionFirstPage);
    const {client} = clientWith({getSession});
    render(<SessionBrowser client={client} />);

    fireEvent.click(
      await screen.findByRole("button", {name: "Open Indexed API cleanup from Codex CLI"}),
    );
    await screen.findByText("Current first page");
    fireEvent.click(screen.getByRole("button", {name: "Load more messages"}));

    expect(await screen.findByText("Latest revision first page")).toBeDefined();
    expect(screen.queryByText("Weeks-old stale cursor page")).toBeNull();
    expect(getSession).toHaveBeenLastCalledWith("session_codex", null);
  });

  it("keeps the current library and conversation mounted throughout refresh", async () => {
    const refresh = deferred<IndexRefreshStateV1>();
    const detailRefresh = deferred<IndexedSessionDetailV1>();
    const refreshedSummary = {
      ...codexSummary,
      title: "Indexed API cleanup updated",
      lastIndexedAtMs: codexSummary.lastIndexedAtMs + 1_000,
      entryCount: 1,
      selectedEntryCount: 1,
    };
    const refreshedDetail = detail(refreshedSummary, [
      {
        entry: {
          entryKey: "entry_updated",
          ordinal: 0,
          atMs: 0,
          kind: "assistant",
          markdown: "Updated indexed conversation",
        },
        selected: true,
      },
    ]);
    const listSessions = vi
      .fn<IndexedSessionClient["listSessions"]>()
      .mockResolvedValueOnce({schemaVersion: 1, sortOrder: "newest", items: [codexSummary], nextCursor: null})
      .mockResolvedValue({schemaVersion: 1, sortOrder: "newest", items: [refreshedSummary], nextCursor: null});
    const getSession = vi
      .fn<IndexedSessionClient["getSession"]>()
      .mockResolvedValueOnce(detail(codexSummary))
      .mockImplementationOnce(() => detailRefresh.promise);
    const {client, emit} = clientWith({
      listSessions,
      refresh: vi.fn(() => refresh.promise),
      getSession,
    });
    render(<SessionBrowser client={client} />);
    fireEvent.click(
      await screen.findByRole("button", {name: "Open Indexed API cleanup from Codex CLI"}),
    );
    await screen.findByText(/Render <script>/);

    fireEvent.click(screen.getByRole("button", {name: "Refresh"}));
    act(() =>
      emit({
        schemaVersion: 1,
        status: "running",
        generation: 7,
        startedAtMs: 1_000,
        discoveredCount: 3,
        processedCount: 1,
        indexedCount: 1,
        unchangedCount: 0,
        failedCount: 0,
        skippedCount: 0,
        warningCount: 0,
      }),
    );
    expect(screen.getByRole("heading", {name: "Indexed API cleanup"})).toBeDefined();
    expect(screen.getByText(/Render <script>/)).toBeDefined();
    expect(screen.getByRole("complementary", {name: "Session sources"})).toBeDefined();
    expect(
      (screen.getByRole("button", {name: "Refreshing sessions"}) as HTMLButtonElement)
        .disabled,
    ).toBe(true);
    expect(screen.getByText("Refreshing 1 of 3 sessions…")).toBeDefined();

    const completed: IndexRefreshStateV1 = {
      schemaVersion: 1,
      status: "completed",
      generation: 7,
      startedAtMs: 1_000,
      completedAtMs: 2_000,
      discoveredCount: 3,
      processedCount: 3,
      indexedCount: 1,
      unchangedCount: 2,
      failedCount: 0,
      skippedCount: 0,
      warningCount: 0,
    };
    await act(async () => {
      emit(completed);
      refresh.resolve(completed);
    });
    expect(await screen.findByText("Indexed API cleanup updated")).toBeDefined();
    expect(screen.getByRole("heading", {name: "Indexed API cleanup"})).toBeDefined();
    expect(screen.getByText(/Render <script>/)).toBeDefined();
    await waitFor(() => expect(getSession).toHaveBeenLastCalledWith("session_codex", null));

    await act(async () => detailRefresh.resolve(refreshedDetail));
    expect(
      await screen.findByRole("heading", {name: "Indexed API cleanup updated"}),
    ).toBeDefined();
    expect(screen.getByText("Updated indexed conversation")).toBeDefined();
  });

  it("reconciles a detail read that finishes after its refresh generation", async () => {
    const openingDetail = deferred<IndexedSessionDetailV1>();
    const refreshedSummary = {
      ...codexSummary,
      title: "Indexed after racing refresh",
      lastIndexedAtMs: codexSummary.lastIndexedAtMs + 1_000,
    };
    const getSession = vi
      .fn<IndexedSessionClient["getSession"]>()
      .mockImplementationOnce(() => openingDetail.promise)
      .mockResolvedValueOnce(detail(refreshedSummary));
    const {client, emit} = clientWith({getSession});
    render(<SessionBrowser client={client} />);
    fireEvent.click(
      await screen.findByRole("button", {name: "Open Indexed API cleanup from Codex CLI"}),
    );

    act(() =>
      emit({
        schemaVersion: 1,
        status: "completed",
        generation: 6,
        startedAtMs: 1_000,
        completedAtMs: 2_000,
        discoveredCount: 1,
        processedCount: 1,
        indexedCount: 1,
        unchangedCount: 0,
        failedCount: 0,
        skippedCount: 0,
        warningCount: 0,
      }),
    );
    await act(async () => openingDetail.resolve(detail(codexSummary)));

    expect(
      await screen.findByRole("heading", {name: "Indexed after racing refresh"}),
    ).toBeDefined();
    expect(getSession).toHaveBeenCalledTimes(2);
  });

  it("keeps navigation available through list errors and retry", async () => {
    const listSessions = vi
      .fn<IndexedSessionClient["listSessions"]>()
      .mockRejectedValueOnce(new Error("C:\\private\\library.sqlite3"))
      .mockResolvedValueOnce({schemaVersion: 1, sortOrder: "newest", items: [], nextCursor: null});
    const {client} = clientWith({listSessions});
    render(<SessionBrowser client={client} />);

    expect(await screen.findByRole("alert")).toHaveProperty(
      "textContent",
      expect.stringContaining("Indexed library could not be loaded"),
    );
    expect(screen.getByRole("complementary", {name: "Session sources"})).toBeDefined();
    expect(document.body.textContent).not.toContain("C:\\private");
    fireEvent.click(screen.getByRole("button", {name: "Try again"}));
    expect(await screen.findByText("No indexed sessions yet")).toBeDefined();
  });

  it("scopes a safe detail error to the selected session", async () => {
    const getSession = vi.fn<IndexedSessionClient["getSession"]>(async () => {
      throw new Error("C:\\private\\source.jsonl contained a secret");
    });
    const {client} = clientWith({getSession});
    render(<SessionBrowser client={client} />);

    fireEvent.click(
      await screen.findByRole("button", {name: "Open Indexed API cleanup from Codex CLI"}),
    );
    expect(await screen.findByRole("alert")).toHaveProperty(
      "textContent",
      expect.stringContaining("This indexed session could not be opened"),
    );
    expect(screen.getByText("Checkout redesign")).toBeDefined();
    expect(document.body.textContent).not.toContain("C:\\private");
  });

  it("retains indexed results when refresh fails", async () => {
    const {client, emit} = clientWith({
      refresh: vi.fn(async () => {
        throw new Error("C:\\private\\scan failure");
      }),
    });
    render(<SessionBrowser client={client} />);
    await screen.findByText("Indexed API cleanup");

    fireEvent.click(screen.getByRole("button", {name: "Refresh"}));
    expect(await screen.findByRole("alert")).toHaveProperty(
      "textContent",
      expect.stringContaining("Refresh status is unavailable"),
    );
    act(() =>
      emit({
        schemaVersion: 1,
        status: "failed",
        generation: 9,
        startedAtMs: 1_000,
        completedAtMs: 2_000,
        discoveredCount: 2,
        processedCount: 1,
        indexedCount: 0,
        unchangedCount: 0,
        failedCount: 1,
        skippedCount: 0,
        warningCount: 0,
        errorCode: "INDEX_STORAGE_FAILED",
      }),
    );
    expect(screen.getByRole("alert").textContent).toContain(
      "Refresh failed. Showing last-known-good indexed sessions.",
    );
    expect(screen.getByText("Indexed API cleanup")).toBeDefined();
    expect(document.body.textContent).not.toContain("C:\\private");
  });

  it("separates failed sessions from source warnings after a partial refresh", async () => {
    const {client, emit} = clientWith();
    render(<SessionBrowser client={client} />);
    await screen.findByText("Indexed API cleanup");

    act(() => emit({
      schemaVersion: 1,
      status: "completed",
      generation: 10,
      startedAtMs: 1_000,
      completedAtMs: 2_000,
      discoveredCount: 3,
      processedCount: 3,
      indexedCount: 1,
      unchangedCount: 1,
      failedCount: 1,
      skippedCount: 0,
      warningCount: 2,
    }));

    expect(await screen.findByText("Library refreshed. 1 updated, 1 unchanged, 1 failed, 2 source warnings.")).toBeDefined();
  });

  it("does not let a stale current-state response replace a newer refresh event", async () => {
    const currentState = deferred<IndexRefreshStateV1>();
    const {client, emit} = clientWith({
      currentRefreshState: vi.fn(() => currentState.promise),
    });
    render(<SessionBrowser client={client} />);
    await screen.findByText("Indexed API cleanup");

    act(() =>
      emit({
        schemaVersion: 1,
        status: "completed",
        generation: 4,
        startedAtMs: 1_000,
        completedAtMs: 2_000,
        discoveredCount: 2,
        processedCount: 2,
        indexedCount: 1,
        unchangedCount: 1,
        failedCount: 0,
        skippedCount: 0,
        warningCount: 0,
      }),
    );
    expect(await screen.findByText("Library refreshed. 1 updated, 1 unchanged.")).toBeDefined();

    await act(async () => currentState.resolve(idle));
    expect(screen.getByText("Library refreshed. 1 updated, 1 unchanged.")).toBeDefined();
  });

  it("keeps the previous conversation when post-refresh reconciliation fails", async () => {
    const getSession = vi
      .fn<IndexedSessionClient["getSession"]>()
      .mockResolvedValueOnce(detail(codexSummary))
      .mockRejectedValueOnce(new Error("C:\\private\\new-source.jsonl"));
    const {client, emit} = clientWith({getSession});
    render(<SessionBrowser client={client} />);
    fireEvent.click(
      await screen.findByRole("button", {name: "Open Indexed API cleanup from Codex CLI"}),
    );
    await screen.findByText(/Render <script>/);

    act(() =>
      emit({
        schemaVersion: 1,
        status: "completed",
        generation: 8,
        startedAtMs: 1_000,
        completedAtMs: 2_000,
        discoveredCount: 1,
        processedCount: 1,
        indexedCount: 1,
        unchangedCount: 0,
        failedCount: 0,
        skippedCount: 0,
        warningCount: 0,
      }),
    );

    expect(
      await screen.findByText(
        "Conversation update unavailable. Showing the previous indexed revision.",
      ),
    ).toBeDefined();
    expect(screen.getByText(/Render <script>/)).toBeDefined();
    expect(document.body.textContent).not.toContain("C:\\private");
  });

  it("collapses the source pane and cleans up its refresh subscription", async () => {
    const unlisten = vi.fn();
    const {client} = clientWith({subscribeToRefresh: vi.fn(async () => unlisten)});
    const rendered = render(<SessionBrowser client={client} />);
    await screen.findByText("Indexed API cleanup");
    const sourcePane = screen.getByRole("complementary", {name: "Session sources"});

    fireEvent.click(screen.getByRole("button", {name: "Collapse sources"}));
    expect(sourcePane.classList.contains("source-sidebar--collapsed")).toBe(true);
    expect(
      screen.getByRole("button", {name: "Expand sources"}).getAttribute("aria-expanded"),
    ).toBe("false");

    rendered.unmount();
    await waitFor(() => expect(unlisten).toHaveBeenCalledTimes(1));
  });
});

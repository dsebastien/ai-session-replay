// @vitest-environment jsdom

import {render, screen} from "@testing-library/react";
import {describe, expect, it, vi} from "vitest";
import type {PresentationPlanV1} from "@ai-session-replay/replay-contract";
import fixture from "../../../tests/fixtures/indexed-library-contracts-v1.json";
import {
  calculateTerminalReplayMetadata,
  terminalFrameState,
  TerminalFrame,
} from "./TerminalReplay";
import {presentationStageState} from "./ConversationStage";

const fontLoads = vi.hoisted(() => [] as Array<Record<string, unknown>>);
vi.mock("@remotion/fonts", () => ({
  loadFont: (options: Record<string, unknown>) => {
    fontLoads.push(options);
    return Promise.resolve();
  },
}));

const plan = fixture.presentationPlan as PresentationPlanV1;

describe("TerminalReplay", () => {
  it("loads the bundled font through Remotion's render gate", () => {
    expect(fontLoads).toHaveLength(1);
    expect(fontLoads[0]).toMatchObject({
      family: "JetBrains Mono",
      url: expect.stringMatching(/fonts\/JetBrainsMono\.woff2/),
      weight: "400",
    });
  });

  it("derives fixed render metadata from the frozen plan", async () => {
    const metadata = await calculateTerminalReplayMetadata({
      props: {plan},
      defaultProps: {plan},
      abortSignal: new AbortController().signal,
      compositionId: "TerminalReplay",
      isRendering: true,
    });
    expect(metadata).toMatchObject({durationInFrames: 150, fps: 30, width: 1_920, height: 1_080});
  });

  it("reveals an entry on its exact frame boundary", () => {
    expect(terminalFrameState(plan, 29).visibleEntries.map(({entry}) => entry.entryKey)).toEqual(["entry_01"]);
    expect(terminalFrameState(plan, 30).visibleEntries.map(({entry}) => entry.entryKey)).toEqual(["entry_02"]);
    expect(terminalFrameState(plan, 30).activeEntryKey).toBe("entry_02");
    expect(presentationStageState(plan, 1)).toEqual({
      visibleEntries: terminalFrameState(plan, 30).visibleEntries,
      visibleEntryCount: 2,
      activeEntryKey: "entry_02",
    });
    expect(() => presentationStageState(plan, -1)).toThrow(RangeError);
    expect(() => presentationStageState(plan, plan.entries.length)).toThrow(RangeError);
  });

  it("finds and renders only the current entry in a 100,000-entry plan", () => {
    const entries = Array.from({length: 100_000}, (_, index) => ({
      entry: {entryKey: `entry_${index}`, ordinal: index, atMs: index, kind: "user" as const, text: "x"},
      revealAtMs: index,
    }));
    const state = terminalFrameState({...plan, entries}, 3_000);
    expect(state.visibleEntries).toHaveLength(1);
    expect(state.visibleEntries.at(-1)?.entry.entryKey).toBe("entry_99999");
  });

  it("renders transcript markup as inert text through the shared card", () => {
    const hostile: PresentationPlanV1 = {
      ...plan,
      entries: [{
        entry: {entryKey: "hostile", ordinal: 0, atMs: 0, kind: "assistant", markdown: "[Link](https://example.com) <img src=x onerror=alert(1)>"},
        revealAtMs: 0,
      }],
      durationMs: 1_000,
    };
    const {container} = render(<TerminalFrame frame={0} plan={hostile} />);
    expect(screen.getByText(/\[Link\]/)).toBeTruthy();
    expect(container.querySelector("a, img, script")).toBeNull();
    expect(screen.getByTestId("entry-hostile").dataset.active).toBe("true");
  });
});

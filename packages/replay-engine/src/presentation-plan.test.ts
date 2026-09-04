import {describe, expect, it} from "vitest";
import fixture from "../../../tests/fixtures/indexed-library-contracts-v1.json";
import type {IndexedSessionDetailV1} from "@ai-session-replay/replay-contract";
import {projectPresentationPlan} from "./presentation-plan";

const detail = fixture.sessionDetail as IndexedSessionDetailV1;

describe("projectPresentationPlan", () => {
  it("filters selections and visibility and computes deterministic reveal times", () => {
    const plan = projectPresentationPlan(detail, detail.entryPage.entries, "plan_1", 5_000);

    expect(plan.entries.map(({entry}) => entry.entryKey)).toEqual([
      "entry_01",
      "entry_02",
      "entry_03",
      "entry_04",
      "entry_06",
      "entry_07",
    ]);
    expect(plan.entries.map(({revealAtMs}) => revealAtMs)).toEqual([0, 1_000, 2_000, 3_000, 4_000, 5_000]);
    expect(plan.entries[3]?.entry).toMatchObject({kind: "tool-call", detail: null});
    expect(plan.durationMs).toBe(6_000);
  });

  it("rejects incomplete, stale, empty, and overlong projections", () => {
    expect(() => projectPresentationPlan(
      {...detail, entryPage: {...detail.entryPage, nextCursor: "entry_07"}},
      detail.entryPage.entries,
      "plan_1",
      5_000,
    )).toThrow("complete conversation");
    expect(() => projectPresentationPlan(
      detail,
      detail.entryPage.entries.map((item) => ({...item, selected: false})),
      "plan_1",
      5_000,
    )).toThrow("no visible selected entries");
    expect(() => projectPresentationPlan(
      {...detail, revision: {...detail.revision, revisionId: "revision_other"}},
      detail.entryPage.entries,
      "plan_1",
      5_000,
    )).toThrow("revision does not match");
    const overlongEntries = Array.from({length: 181}, (_, index) => ({
        entry: {entryKey: `entry_${index}`, ordinal: index, atMs: index, kind: "user" as const, text: "x"},
        selected: true,
      }));
    expect(() => projectPresentationPlan(
      {
        ...detail,
        summary: {...detail.summary, entryCount: 181, selectedEntryCount: 181},
        revision: {...detail.revision, entryCount: 181},
        entryPage: {...detail.entryPage, entries: overlongEntries, totalEntryCount: 181},
        preferences: {
          ...detail.preferences,
          timing: {entryDelayMs: 10_000, playbackSpeed: 0.25},
        },
      },
      overlongEntries,
      "plan_1",
      5_000,
    )).toThrow("duration exceeds");
    expect(() => projectPresentationPlan(detail, detail.entryPage.entries, "../bad", 5_000)).toThrow();
  });

  it("applies hidden categories and available tool detail", () => {
    const entries = detail.entryPage.entries.map((item) =>
      item.entry.kind === "tool-call"
        ? {...item, entry: {...item.entry, detail: {availability: "available" as const, arguments: "{}", result: "ok"}}}
        : item,
    );
    const plan = projectPresentationPlan(
      {
        ...detail,
        entryPage: {...detail.entryPage, entries},
        preferences: {
          ...detail.preferences,
          visibility: {showToolCalls: true, showToolDetails: true, showReasoning: false},
        },
      },
      entries,
      "plan_2",
      5_000,
    );
    expect(plan.entries.some(({entry}) => entry.kind === "reasoning")).toBe(false);
    expect(plan.entries.find(({entry}) => entry.kind === "tool-call")?.entry).toMatchObject({
      detail: {arguments: "{}", result: "ok"},
    });

    const noTools = projectPresentationPlan(
      {
        ...detail,
        preferences: {
          ...detail.preferences,
          visibility: {showToolCalls: false, showToolDetails: false, showReasoning: true},
        },
      },
      detail.entryPage.entries,
      "plan_3",
      5_000,
    );
    expect(noTools.entries.some(({entry}) => entry.kind === "tool-call")).toBe(false);
  });
});

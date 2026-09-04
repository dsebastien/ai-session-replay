// @vitest-environment node

import {describe, expect, it} from "vitest";
import {displayStateAtSourceMs} from "./index";
import type {ReplayEvent} from "@ai-session-replay/replay-contract";

const events: readonly ReplayEvent[] = [
  {id: "one", atMs: 0, kind: "user", text: "Create a replay"},
  {id: "two", atMs: 500, kind: "assistant", markdown: "Working"},
  {
    id: "three",
    atMs: 1_000,
    kind: "tool",
    name: "powershell",
    status: "succeeded",
    summary: "Tests passed",
  },
];

describe("displayStateAtSourceMs", () => {
  it("includes events at the exact current source time", () => {
    const state = displayStateAtSourceMs(events, 500);

    expect(state.visibleEvents.map((event) => event.id)).toEqual(["one", "two"]);
    expect(state.activeEventId).toBe("two");
  });

  it("keeps the final event visible during terminal hold", () => {
    const state = displayStateAtSourceMs(events, 2_000);

    expect(state.visibleEvents.at(-1)?.id).toBe("three");
    expect(state.activeEventId).toBe("three");
  });

  it("rejects negative and non-finite source times", () => {
    expect(() => displayStateAtSourceMs(events, -1)).toThrow(
      "Source time must be finite and non-negative",
    );
    expect(() => displayStateAtSourceMs(events, Number.POSITIVE_INFINITY)).toThrow(
      "Source time must be finite and non-negative",
    );
  });
});

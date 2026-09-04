import {describe, expect, it} from "vitest";
import {conversationWindow} from "./SessionSelection";

describe("conversationWindow", () => {
  it("keeps the rendered DOM window bounded for 100,000 logical entries", () => {
    const entries = Array.from({length: 100_000}, (_, index) => index);

    expect(conversationWindow(entries, 0)).toEqual(entries.slice(0, 100));
    expect(conversationWindow(entries, 99_900)).toEqual(entries.slice(99_900));
    expect(conversationWindow(entries, -1)).toEqual(entries.slice(0, 100));
  });
});

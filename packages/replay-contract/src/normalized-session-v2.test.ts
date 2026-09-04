// @vitest-environment node

import {describe, expect, it} from "vitest";
import fixture from "../../../tests/fixtures/normalized-session-v2.json";
import legacyProject from "../../../tests/fixtures/contracts-v1.json";
import {
  migrateNormalizedSessionV1,
  validateNormalizedSessionV2,
  type NormalizedSessionV1,
} from "./index";

describe("normalized session V2", () => {
  it("accepts the cross-language rich-entry fixture", () => {
    expect(validateNormalizedSessionV2(fixture)).toEqual({ok: true, value: fixture});
  });

  it("migrates V1 without inventing reasoning or tool detail", () => {
    const legacy = legacyProject.session as NormalizedSessionV1;
    const result = migrateNormalizedSessionV1(legacy);

    expect(result).toMatchObject({
      ok: true,
      value: {
        schemaVersion: 2,
        contentAvailability: {reasoning: "unavailable", toolDetails: "unavailable"},
      },
    });
    if (!result.ok) throw new Error("migration should succeed");
    expect(result.value.entries).toHaveLength(legacy.events.length);
    expect(result.value.entries.some(({kind}) => kind === "reasoning")).toBe(false);
    expect(result.value.entries.filter(({kind}) => kind === "tool-call")).toEqual([
      expect.objectContaining({detail: {availability: "unavailable"}}),
      expect.objectContaining({detail: {availability: "unavailable"}}),
      expect.objectContaining({detail: {availability: "unavailable"}}),
    ]);
  });

  it("preserves safe V1 IDs and deterministically maps unsafe IDs", () => {
    const legacy = legacyProject.session as NormalizedSessionV1;
    const safe = {
      ...legacy,
      events: [
        {...legacy.events[0]!, id: "safe_entry-1"},
        {...legacy.events[1]!, id: "unsafe:entry:2"},
      ],
      durationMs: legacy.events[1]!.atMs + 1_500,
      unknownRecordCount: 0,
    };

    const first = migrateNormalizedSessionV1(safe);
    const second = migrateNormalizedSessionV1(structuredClone(safe));
    expect(first).toEqual(second);
    if (!first.ok) throw new Error("migration should succeed");
    expect(first.value.entries[0]?.entryKey).toBe("safe_entry-1");
    expect(first.value.entries[1]?.entryKey).toBe("legacy-288ce059a7cfea15");
  });

  it("keeps every migrated prefix key stable across append-only growth", () => {
    const legacy = legacyProject.session as NormalizedSessionV1;
    const entries = Array.from({length: 64}, (_, index) => ({
      id: `entry-${index}`,
      atMs: index,
      kind: "user" as const,
      text: `synthetic ${index}`,
    }));

    for (let prefixLength = 1; prefixLength < entries.length; prefixLength += 1) {
      const before = migrateNormalizedSessionV1({
        ...legacy,
        events: entries.slice(0, prefixLength),
        durationMs: prefixLength - 1 + 1_500,
        unknownRecordCount: 0,
      });
      const after = migrateNormalizedSessionV1({
        ...legacy,
        events: entries.slice(0, prefixLength + 1),
        durationMs: prefixLength + 1_500,
        unknownRecordCount: 0,
      });
      if (!before.ok || !after.ok) throw new Error("migration should succeed");
      expect(after.value.entries.slice(0, prefixLength).map(({entryKey}) => entryKey)).toEqual(
        before.value.entries.map(({entryKey}) => entryKey),
      );
    }
  });

  it.each([
    ["unknown fields", {...fixture, rawVendorRecord: {secret: true}}, "UNKNOWN_FIELD"],
    ["duplicate keys", {...fixture, entries: fixture.entries.map((entry, index) => index === 1 ? {...entry, entryKey: fixture.entries[0]!.entryKey} : entry)}, "DUPLICATE_ENTRY_KEY"],
    ["non-monotonic time", {...fixture, entries: fixture.entries.map((entry, index) => index === 2 ? {...entry, atMs: 1} : entry)}, "INVALID_ENTRY_TIMELINE"],
    ["invented reasoning", {...fixture, contentAvailability: {...fixture.contentAvailability, reasoning: "unavailable"}}, "CONTENT_AVAILABILITY_MISMATCH"],
    ["invented tool detail", {...fixture, contentAvailability: {...fixture.contentAvailability, toolDetails: "unavailable"}}, "CONTENT_AVAILABILITY_MISMATCH"],
    ["empty available detail", {...fixture, entries: fixture.entries.map((entry) => entry.kind === "tool-call" && "detail" in entry && entry.detail?.availability === "available" ? {...entry, detail: {...entry.detail, arguments: null, result: null}} : entry)}, "INVALID_TOOL_DETAIL"],
    ["missing nullable detail field", {...fixture, entries: fixture.entries.map((entry) => entry.kind === "tool-call" && "detail" in entry && entry.detail?.availability === "available" ? {...entry, detail: {availability: "available", arguments: entry.detail.arguments}} : entry)}, "INVALID_TOOL_DETAIL"],
    ["invalid key", {...fixture, entries: fixture.entries.map((entry, index) => index === 0 ? {...entry, entryKey: "../entry"} : entry)}, "INVALID_ENTRY_KEY"],
    ["NUL content", {...fixture, entries: fixture.entries.map((entry, index) => index === 0 ? {...entry, text: "bad\0text"} : entry)}, "INVALID_ENTRY"],
  ])("rejects %s", (_name, candidate, code) => {
    expect(validateNormalizedSessionV2(candidate)).toMatchObject({ok: false, error: {code}});
  });

  it("requires unknown accounting to exactly match unknown entries", () => {
    expect(validateNormalizedSessionV2({...fixture, unknownRecordCount: 0})).toMatchObject({
      ok: false,
      error: {code: "INVALID_UNKNOWN_RECORD_COUNT"},
    });
  });
});

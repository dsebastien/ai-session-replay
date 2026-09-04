// @vitest-environment node

import {describe, expect, it} from "vitest";
import fixture from "../../../tests/fixtures/indexed-library-contracts-v1.json";
import {
  MAX_INDEXED_PAGE_SIZE,
  MAX_PRESENTATION_ENTRY_COUNT,
  MAX_SELECTION_CHANGE_COUNT,
  validateDeleteIndexedSessionRequest,
  validateDeleteIndexedSessionWithSourceRequest,
  validateIndexRefreshState,
  validateIndexedSessionDetail,
  validateIndexedSessionListRequest,
  validateIndexedSessionPage,
  validatePrepareSourceDeletionRequest,
  validatePresentationPlan,
  validateResetLocalDatabaseRequest,
  validateResetLocalDatabaseResult,
  validateRenameIndexedSessionRequest,
  validateRestoreSuppressedSourceRequest,
  validateRestoreSuppressedSourceResult,
  validateSetEntrySelectionsRequest,
  validateSetSessionPreferencesRequest,
  validateSourceDeletionConfirmation,
  validateSuppressedSource,
  validateSuppressedSourceListRequest,
  validateSuppressedSourcePage,
} from "./indexed-library";

type ValidationResult = Readonly<
  | {ok: true; value: unknown}
  | {ok: false; error: Readonly<{code: string; message: string}>}
>;

const validators: ReadonlyArray<
  readonly [string, (input: unknown) => ValidationResult, unknown]
> = [
  ["list request", validateIndexedSessionListRequest, fixture.listRequest],
  ["session page", validateIndexedSessionPage, fixture.sessionPage],
  ["session detail", validateIndexedSessionDetail, fixture.sessionDetail],
  ["selection request", validateSetEntrySelectionsRequest, fixture.selectionRequest],
  ["rename request", validateRenameIndexedSessionRequest, fixture.renameRequest],
  ["preferences request", validateSetSessionPreferencesRequest, fixture.preferencesRequest],
  ["refresh state", validateIndexRefreshState, fixture.refreshState],
  ["library deletion", validateDeleteIndexedSessionRequest, fixture.libraryDeletionRequest],
  ["source deletion preparation", validatePrepareSourceDeletionRequest, fixture.prepareSourceDeletionRequest],
  ["source deletion confirmation", validateSourceDeletionConfirmation, fixture.sourceDeletionConfirmation],
  ["source deletion", validateDeleteIndexedSessionWithSourceRequest, fixture.sourceDeletionRequest],
  ["suppression restore", validateRestoreSuppressedSourceRequest, fixture.restoreRequest],
  ["suppressed source", validateSuppressedSource, fixture.suppressedSource],
  ["suppressed-source list request", validateSuppressedSourceListRequest, fixture.suppressedSourceListRequest],
  ["suppressed-source page", validateSuppressedSourcePage, fixture.suppressedSourcePage],
  ["suppression restore result", validateRestoreSuppressedSourceResult, fixture.restoreResult],
  ["local database reset", validateResetLocalDatabaseRequest, fixture.resetRequest],
  ["local database reset result", validateResetLocalDatabaseResult, fixture.resetResult],
  ["presentation plan", validatePresentationPlan, fixture.presentationPlan],
];

function clone<T>(value: T): T {
  return structuredClone(value);
}

function mutated(value: unknown, path: readonly (string | number)[], replacement: unknown): unknown {
  const copy: unknown = structuredClone(value);
  let cursor = copy;
  for (const segment of path.slice(0, -1)) {
    if (Array.isArray(cursor) && typeof segment === "number") {
      cursor = cursor[segment];
    } else if (typeof cursor === "object" && cursor !== null && !Array.isArray(cursor) && typeof segment === "string") {
      cursor = (cursor as Record<string, unknown>)[segment];
    } else {
      throw new Error("invalid fixture mutation path");
    }
  }
  const finalSegment = path.at(-1);
  if (Array.isArray(cursor) && typeof finalSegment === "number") {
    cursor[finalSegment] = replacement;
  } else if (typeof cursor === "object" && cursor !== null && !Array.isArray(cursor) && typeof finalSegment === "string") {
    (cursor as Record<string, unknown>)[finalSegment] = replacement;
  } else {
    throw new Error("invalid fixture mutation target");
  }
  return copy;
}

function expectInvalid(result: ValidationResult, code: string): void {
  expect(result).toMatchObject({ok: false, error: {code}});
}

describe("indexed-library v1 cross-language fixture", () => {
  it.each(validators)("accepts and canonicalizes the %s", (_name, validate, value) => {
    expect(validate(value)).toEqual({ok: true, value});
  });

  it("contains every rich entry variant and both tool-detail states", () => {
    const entries = fixture.sessionDetail.entryPage.entries.map(({entry}) => entry);

    expect(entries.map(({kind}) => kind)).toEqual([
      "user",
      "assistant",
      "reasoning",
      "tool-call",
      "tool-call",
      "file-change",
      "unknown",
    ]);
    const detailStates = entries.flatMap((entry) =>
      entry.kind === "tool-call" && entry.detail !== undefined
        ? [entry.detail.availability]
        : [],
    );
    expect(detailStates).toEqual([
      "available",
      "unavailable",
    ]);
  });
});

describe("indexed-library v1 boundary validation", () => {
  it.each(validators)("rejects non-object and unknown-field %s payloads", (_name, validate, value) => {
    expect(validate(null).ok).toBe(false);
    expectInvalid(validate(mutated(value, ["unexpected"], true)), "UNKNOWN_FIELD");
  });

  it("rejects unknown fields that could smuggle a source or database path", () => {
    for (const [validate, value] of [
      [validateIndexedSessionPage, {...fixture.sessionPage, sourcePath: "C:\\private\\session.jsonl"}],
      [validateIndexedSessionDetail, {...fixture.sessionDetail, databasePath: "C:\\private\\library.db"}],
      [validateDeleteIndexedSessionWithSourceRequest, {...fixture.sourceDeletionRequest, path: "C:\\private"}],
      [validatePresentationPlan, {...fixture.presentationPlan, rawRecord: {secret: true}}],
    ] as const) {
      expectInvalid(validate(value), "UNKNOWN_FIELD");
    }
  });

  it("accepts VS Code Copilot and rejects unknown discriminated-union variants", () => {
    expect(validateIndexedSessionListRequest({
      ...fixture.listRequest,
      source: "vscode-copilot",
    }).ok).toBe(true);
    expectInvalid(
      validateIndexedSessionListRequest({...fixture.listRequest, sortOrder: "recent"}),
      "INVALID_SORT_ORDER",
    );
    expectInvalid(
      validateIndexedSessionListRequest({...fixture.listRequest, cursor: "oldest__cursor_01"}),
      "INVALID_CURSOR",
    );
    expect(validateIndexedSessionPage({
      ...fixture.sessionPage,
      items: [{...fixture.sessionPage.items[0], source: "vscode-copilot"}],
    }).ok).toBe(true);
    expectInvalid(
      validateIndexedSessionListRequest({...fixture.listRequest, source: "jetbrains"}),
      "INVALID_SOURCE",
    );
    expectInvalid(
      validateIndexedSessionPage({...fixture.sessionPage, sortOrder: "recent"}),
      "INVALID_SORT_ORDER",
    );
    expectInvalid(
      validateIndexedSessionPage({
        ...fixture.sessionPage,
        sortOrder: "oldest",
        nextCursor: "newest__session_01",
      }),
      "INVALID_CURSOR",
    );
    expectInvalid(
      validateIndexRefreshState({...fixture.refreshState, status: "paused"}),
      "INVALID_REFRESH_STATUS",
    );

    const detail = clone(fixture.sessionDetail) as {entryPage: {entries: Array<{entry: {kind: string}}>}};
    detail.entryPage.entries[0]!.entry.kind = "system";
    expectInvalid(validateIndexedSessionDetail(detail), "INVALID_ENTRY_KIND");

    expectInvalid(
      validatePresentationPlan(mutated(fixture.presentationPlan, ["entries", 0, "entry", "kind"], "system")),
      "INVALID_ENTRY_KIND",
    );
  });

  it("rejects omitted nullable fields and oversized opaque values", () => {
    const listWithoutSource = clone(fixture.listRequest) as Record<string, unknown>;
    delete listWithoutSource.source;
    expectInvalid(validateIndexedSessionListRequest(listWithoutSource), "INVALID_SOURCE");

    const listWithoutCursor = clone(fixture.listRequest) as Record<string, unknown>;
    delete listWithoutCursor.cursor;
    expectInvalid(validateIndexedSessionListRequest(listWithoutCursor), "INVALID_CURSOR");

    const listWithoutSort = clone(fixture.listRequest) as Record<string, unknown>;
    delete listWithoutSort.sortOrder;
    expectInvalid(validateIndexedSessionListRequest(listWithoutSort), "INVALID_SORT_ORDER");

    const detailWithoutCreationTime = clone(fixture.sessionDetail) as {
      summary: Record<string, unknown>;
    };
    delete detailWithoutCreationTime.summary.createdAtMs;
    expectInvalid(validateIndexedSessionDetail(detailWithoutCreationTime), "INVALID_TIMESTAMP");

    const pageWithoutCursor = clone(fixture.sessionPage) as Record<string, unknown>;
    delete pageWithoutCursor.nextCursor;
    expectInvalid(validateIndexedSessionPage(pageWithoutCursor), "INVALID_CURSOR");

    const detailWithoutCursor = clone(fixture.sessionDetail) as {
      entryPage: Record<string, unknown>;
    };
    delete detailWithoutCursor.entryPage.nextCursor;
    expectInvalid(validateIndexedSessionDetail(detailWithoutCursor), "INVALID_CURSOR");

    for (const nullableField of ["arguments", "result"] as const) {
      const detailWithoutToolField = clone(fixture.sessionDetail) as {
        entryPage: {entries: Array<{entry: {detail?: Record<string, unknown>}}>};
      };
      delete detailWithoutToolField.entryPage.entries[3]!.entry.detail?.[nullableField];
      expectInvalid(validateIndexedSessionDetail(detailWithoutToolField), "INVALID_TOOL_DETAIL");
    }

    const planWithoutDetail = clone(fixture.presentationPlan) as {
      entries: Array<{entry: Record<string, unknown>}>;
    };
    delete planWithoutDetail.entries[3]!.entry.detail;
    expectInvalid(validatePresentationPlan(planWithoutDetail), "INVALID_TOOL_DETAIL");

    const planWithoutNullableDetailField = clone(fixture.presentationPlan) as {
      entries: Array<{entry: Record<string, unknown>}>;
    };
    planWithoutNullableDetailField.entries[3]!.entry.detail = {arguments: "fixture.json"};
    expectInvalid(validatePresentationPlan(planWithoutNullableDetailField), "INVALID_TOOL_DETAIL");

    const idleWithoutCompletion = {schemaVersion: 1, status: "idle"};
    expectInvalid(validateIndexRefreshState(idleWithoutCompletion), "INVALID_TIMESTAMP");

    expectInvalid(
      validateDeleteIndexedSessionRequest({
        ...fixture.libraryDeletionRequest,
        sessionId: "a".repeat(257),
      }),
      "INVALID_OPAQUE_ID",
    );
    expectInvalid(
      validateDeleteIndexedSessionWithSourceRequest({
        ...fixture.sourceDeletionRequest,
        confirmationToken: "a".repeat(257),
      }),
      "INVALID_CONFIRMATION_TOKEN",
    );
    expectInvalid(
      validateIndexedSessionListRequest({...fixture.listRequest, schemaVersion: 2}),
      "UNSUPPORTED_SCHEMA_VERSION",
    );
  });

  it("validates summary, revision, and entry-page scalar bounds", () => {
    const summaryCases: ReadonlyArray<readonly [readonly (string | number)[], unknown, string]> = [
      [["items", 0, "title"], "", "INVALID_TITLE"],
      [["items", 0, "title"], "x".repeat(513), "INVALID_TITLE"],
      [["items", 0, "title"], "bad\0title", "INVALID_TITLE"],
      [["items", 0, "createdAtMs"], Number.MAX_SAFE_INTEGER + 1, "INVALID_TIMESTAMP"],
      [["items", 0, "lastIndexedAtMs"], Number.MAX_SAFE_INTEGER + 1, "INVALID_TIMESTAMP"],
      [["items", 0, "sourcePresent"], "yes", "INVALID_SOURCE_PRESENCE"],
      [["items", 0, "entryCount"], 0, "INVALID_ENTRY_COUNT"],
      [["items", 0, "entryCount"], MAX_PRESENTATION_ENTRY_COUNT + 1, "INVALID_ENTRY_COUNT"],
      [["items", 0, "selectedEntryCount"], 8, "INVALID_ENTRY_COUNT"],
      [["items", 0, "durationMs"], 604_800_001, "INVALID_SESSION_DURATION"],
      [["items", 0, "diagnosticCount"], 1_001, "INVALID_DIAGNOSTIC_COUNT"],
      [["items", 0, "contentAvailability", "reasoning"], "partial", "INVALID_CONTENT_AVAILABILITY"],
    ];
    for (const [path, value, code] of summaryCases) {
      expectInvalid(validateIndexedSessionPage(mutated(fixture.sessionPage, path, value)), code);
    }

    expectInvalid(
      validateIndexedSessionPage({
        ...fixture.sessionPage,
        items: [fixture.sessionPage.items[0], fixture.sessionPage.items[0]],
      }),
      "DUPLICATE_SESSION_ID",
    );
    expectInvalid(
      validateIndexedSessionPage({...fixture.sessionPage, nextCursor: "../cursor"}),
      "INVALID_CURSOR",
    );

    const revisionCases: ReadonlyArray<readonly [string, unknown]> = [
      ["indexedAtMs", Number.MAX_SAFE_INTEGER + 1],
      ["entryCount", 0],
      ["entryCount", MAX_PRESENTATION_ENTRY_COUNT + 1],
      ["durationMs", 604_800_001],
      ["diagnosticCount", 1_001],
    ];
    for (const [field, value] of revisionCases) {
      expectInvalid(
        validateIndexedSessionDetail(mutated(fixture.sessionDetail, ["revision", field], value)),
        "INVALID_REVISION",
      );
    }

    expectInvalid(
      validateIndexedSessionDetail(mutated(
        fixture.sessionDetail,
        ["entryPage", "entries"],
        Array.from({length: MAX_INDEXED_PAGE_SIZE + 1}, () => fixture.sessionDetail.entryPage.entries[0]),
      )),
      "PAGE_LIMIT_EXCEEDED",
    );
    for (const total of [0, MAX_PRESENTATION_ENTRY_COUNT + 1, 1]) {
      expectInvalid(
        validateIndexedSessionDetail(mutated(fixture.sessionDetail, ["entryPage", "totalEntryCount"], total)),
        "INVALID_ENTRY_COUNT",
      );
    }
    expectInvalid(
      validateIndexedSessionDetail(mutated(fixture.sessionDetail, ["entryPage", "entries", 2, "entry", "atMs"], 500)),
      "INVALID_ENTRY_ORDER",
    );
  });

  it("validates every session identity and metadata relationship", () => {
    const cases: ReadonlyArray<readonly [readonly string[], unknown, string]> = [
      [["revision", "sessionId"], "session_other", "SESSION_MISMATCH"],
      [["entryPage", "sessionId"], "session_other", "SESSION_MISMATCH"],
      [["revision", "entryCount"], 6, "SESSION_METADATA_MISMATCH"],
      [["entryPage", "totalEntryCount"], 8, "SESSION_METADATA_MISMATCH"],
      [["revision", "durationMs"], 7_001, "SESSION_METADATA_MISMATCH"],
      [["revision", "diagnosticCount"], 2, "SESSION_METADATA_MISMATCH"],
    ];
    for (const [path, value, code] of cases) {
      expectInvalid(validateIndexedSessionDetail(mutated(fixture.sessionDetail, path, value)), code);
    }
  });

  it("rejects malformed root metadata before domain logic runs", () => {
    const cases: ReadonlyArray<readonly [(input: unknown) => ValidationResult, unknown, string]> = [
      [validateIndexedSessionPage, {...fixture.sessionPage, schemaVersion: 2}, "UNSUPPORTED_SCHEMA_VERSION"],
      [validateIndexedSessionPage, {...fixture.sessionPage, items: null}, "INVALID_SESSION_PAGE"],
      [validateIndexedSessionDetail, {...fixture.sessionDetail, schemaVersion: 2}, "UNSUPPORTED_SCHEMA_VERSION"],
      [validateIndexedSessionDetail, {...fixture.sessionDetail, preferences: null}, "INVALID_PREFERENCES"],
      [validateSetEntrySelectionsRequest, {...fixture.selectionRequest, schemaVersion: 2}, "UNSUPPORTED_SCHEMA_VERSION"],
      [validateSetEntrySelectionsRequest, {...fixture.selectionRequest, sessionId: "../session"}, "INVALID_OPAQUE_ID"],
      [validateSetEntrySelectionsRequest, {...fixture.selectionRequest, revisionId: "../revision"}, "INVALID_OPAQUE_ID"],
      [validateSetEntrySelectionsRequest, {...fixture.selectionRequest, changes: null}, "INVALID_SELECTION_REQUEST"],
      [validateRenameIndexedSessionRequest, {...fixture.renameRequest, schemaVersion: 2}, "UNSUPPORTED_SCHEMA_VERSION"],
      [validateRenameIndexedSessionRequest, {...fixture.renameRequest, sessionId: "../session"}, "INVALID_OPAQUE_ID"],
      [validateRenameIndexedSessionRequest, {...fixture.renameRequest, title: "   "}, "INVALID_SESSION_TITLE"],
      [validateRenameIndexedSessionRequest, {...fixture.renameRequest, title: "x".repeat(513)}, "INVALID_SESSION_TITLE"],
      [validateSetSessionPreferencesRequest, {...fixture.preferencesRequest, schemaVersion: 2}, "UNSUPPORTED_SCHEMA_VERSION"],
      [validateSetSessionPreferencesRequest, {...fixture.preferencesRequest, sessionId: "../session"}, "INVALID_OPAQUE_ID"],
      [validateSetSessionPreferencesRequest, {...fixture.preferencesRequest, revisionId: "../revision"}, "INVALID_OPAQUE_ID"],
      [validateIndexRefreshState, {...fixture.refreshState, schemaVersion: 2}, "UNSUPPORTED_SCHEMA_VERSION"],
      [validatePrepareSourceDeletionRequest, {...fixture.prepareSourceDeletionRequest, mode: "library-only"}, "INVALID_DELETION_MODE"],
      [validatePrepareSourceDeletionRequest, {...fixture.prepareSourceDeletionRequest, sessionId: "../session"}, "INVALID_OPAQUE_ID"],
      [validateSourceDeletionConfirmation, {...fixture.sourceDeletionConfirmation, sessionId: "../session"}, "INVALID_OPAQUE_ID"],
      [validateSourceDeletionConfirmation, {...fixture.sourceDeletionConfirmation, confirmationToken: "../token"}, "INVALID_CONFIRMATION_TOKEN"],
      [validateDeleteIndexedSessionWithSourceRequest, {...fixture.sourceDeletionRequest, sessionId: "../session"}, "INVALID_OPAQUE_ID"],
      [validateRestoreSuppressedSourceRequest, {...fixture.restoreRequest, suppressionId: "../suppression"}, "INVALID_OPAQUE_ID"],
      [validateResetLocalDatabaseRequest, {...fixture.resetRequest, mode: "reset-all-files"}, "INVALID_RESET_MODE"],
      [validateResetLocalDatabaseResult, {...fixture.resetResult, resetAtMs: -1}, "INVALID_TIMESTAMP"],
      [validatePresentationPlan, {...fixture.presentationPlan, schemaVersion: 2}, "UNSUPPORTED_SCHEMA_VERSION"],
      [validatePresentationPlan, {...fixture.presentationPlan, entries: null}, "INVALID_PRESENTATION_PLAN"],
      [validatePresentationPlan, {...fixture.presentationPlan, preferences: null}, "INVALID_PREFERENCES"],
    ];
    for (const [validate, value, code] of cases) {
      expectInvalid(validate(value), code);
    }
  });

  it.each(["../session", "C:\\private", "token with spaces", ""]) (
    "rejects path-like or malformed opaque identifiers: %s",
    (sessionId) => {
      expectInvalid(
        validateDeleteIndexedSessionRequest({
          ...fixture.libraryDeletionRequest,
          sessionId,
        }),
        "INVALID_OPAQUE_ID",
      );
    },
  );

  it("bounds list pages and selection batches", () => {
    expectInvalid(
      validateIndexedSessionListRequest({...fixture.listRequest, pageSize: MAX_INDEXED_PAGE_SIZE + 1}),
      "INVALID_PAGE_SIZE",
    );
    expectInvalid(
      validateIndexedSessionPage({
        ...fixture.sessionPage,
        items: Array.from({length: MAX_INDEXED_PAGE_SIZE + 1}, () => fixture.sessionPage.items[0]),
      }),
      "PAGE_LIMIT_EXCEEDED",
    );
    expectInvalid(
      validateSetEntrySelectionsRequest({
        ...fixture.selectionRequest,
        changes: Array.from({length: MAX_SELECTION_CHANGE_COUNT + 1}, (_, index) => ({
          entryKey: `entry_${index}`,
          selected: true,
        })),
      }),
      "SELECTION_CHANGE_LIMIT_EXCEEDED",
    );
  });

  it("rejects duplicate selection keys and stale cross-object revision identities", () => {
    expectInvalid(
      validateSetEntrySelectionsRequest({
        ...fixture.selectionRequest,
        changes: [fixture.selectionRequest.changes[0], fixture.selectionRequest.changes[0]],
      }),
      "DUPLICATE_ENTRY_KEY",
    );
    expectInvalid(
      validateIndexedSessionDetail({
        ...fixture.sessionDetail,
        entryPage: {...fixture.sessionDetail.entryPage, revisionId: "revision_other"},
      }),
      "REVISION_MISMATCH",
    );
  });

  it("requires honest reasoning and tool-detail availability", () => {
    expectInvalid(
      validateIndexedSessionDetail({
        ...fixture.sessionDetail,
        summary: {
          ...fixture.sessionDetail.summary,
          contentAvailability: {
            ...fixture.sessionDetail.summary.contentAvailability,
            reasoning: "unavailable",
          },
        },
      }),
      "CONTENT_AVAILABILITY_MISMATCH",
    );
    expectInvalid(
      validateIndexedSessionDetail({
        ...fixture.sessionDetail,
        summary: {
          ...fixture.sessionDetail.summary,
          contentAvailability: {
            ...fixture.sessionDetail.summary.contentAvailability,
            toolDetails: "unavailable",
          },
        },
      }),
      "CONTENT_AVAILABILITY_MISMATCH",
    );
    expectInvalid(
      validateIndexedSessionDetail({
        ...fixture.sessionDetail,
        summary: {
          ...fixture.sessionDetail.summary,
          contentAvailability: {
            ...fixture.sessionDetail.summary.contentAvailability,
            toolDetails: "available",
          },
        },
      }),
      "CONTENT_AVAILABILITY_MISMATCH",
    );
  });

  it("rejects out-of-order and duplicate entry identities", () => {
    const duplicate = clone(fixture.sessionDetail);
    const firstEntry = duplicate.entryPage.entries[0];
    if (firstEntry === undefined) {
      throw new Error("fixture must contain an entry");
    }
    duplicate.entryPage.entries[1] = clone(firstEntry);
    expectInvalid(validateIndexedSessionDetail(duplicate), "INVALID_ENTRY_ORDER");

    const reversed = clone(fixture.sessionDetail);
    reversed.entryPage.entries.reverse();
    expectInvalid(validateIndexedSessionDetail(reversed), "INVALID_ENTRY_ORDER");
  });

  it("bounds preference timing and validates visual settings", () => {
    expectInvalid(
      validateSetSessionPreferencesRequest({
        ...fixture.preferencesRequest,
        preferences: {
          ...fixture.preferencesRequest.preferences,
          timing: {entryDelayMs: 249, playbackSpeed: 1},
        },
      }),
      "INVALID_PRESENTATION_TIMING",
    );
    expectInvalid(
      validateSetSessionPreferencesRequest({
        ...fixture.preferencesRequest,
        preferences: {
          ...fixture.preferencesRequest.preferences,
          appearance: {
            ...fixture.preferencesRequest.preferences.appearance,
            theme: {
              ...fixture.preferencesRequest.preferences.appearance.theme,
              accent: "red",
            },
          },
        },
      }),
      "INVALID_APPEARANCE",
    );
  });

  it("validates rich-entry and preference field boundaries", () => {
    const entryCases: ReadonlyArray<readonly [number, string, unknown, string]> = [
      [0, "entryKey", "../entry", "INVALID_OPAQUE_ID"],
      [0, "ordinal", MAX_PRESENTATION_ENTRY_COUNT, "INVALID_ENTRY"],
      [0, "atMs", 604_800_001, "INVALID_ENTRY"],
      [0, "text", "bad\0text", "INVALID_ENTRY"],
      [1, "markdown", "bad\0markdown", "INVALID_ENTRY"],
      [2, "text", "bad\0reasoning", "INVALID_ENTRY"],
      [3, "name", "", "INVALID_ENTRY"],
      [3, "status", "cancelled", "INVALID_ENTRY"],
      [3, "summary", "bad\0summary", "INVALID_ENTRY"],
      [5, "displayPath", "", "INVALID_ENTRY"],
      [5, "summary", "bad\0summary", "INVALID_ENTRY"],
      [6, "sourceType", "", "INVALID_ENTRY"],
    ];
    for (const [index, field, value, code] of entryCases) {
      expectInvalid(
        validateIndexedSessionDetail(mutated(
          fixture.sessionDetail,
          ["entryPage", "entries", index, "entry", field],
          value,
        )),
        code,
      );
    }

    expectInvalid(
      validateIndexedSessionDetail(mutated(
        fixture.sessionDetail,
        ["entryPage", "entries", 3, "entry", "detail"],
        {availability: "available", arguments: null, result: null},
      )),
      "INVALID_TOOL_DETAIL",
    );
    expectInvalid(
      validateIndexedSessionDetail(mutated(
        fixture.sessionDetail,
        ["entryPage", "entries", 0, "selected"],
        "yes",
      )),
      "INVALID_SELECTION",
    );

    const preferenceCases: ReadonlyArray<readonly [readonly (string | number)[], unknown, string]> = [
      [["preferences", "visibility", "showToolCalls"], false, "INVALID_VISIBILITY"],
      [["preferences", "timing", "entryDelayMs"], 10_001, "INVALID_PRESENTATION_TIMING"],
      [["preferences", "timing", "playbackSpeed"], 4.01, "INVALID_PRESENTATION_TIMING"],
      [["preferences", "appearance", "theme", "accent"], "1234567", "INVALID_APPEARANCE"],
      [["preferences", "appearance", "theme", "accent"], "#gggggg", "INVALID_APPEARANCE"],
      [["preferences", "appearance", "font", "family"], "Comic Sans", "INVALID_APPEARANCE"],
      [["preferences", "appearance", "font", "sizePx"], 11, "INVALID_APPEARANCE"],
      [["preferences", "appearance", "font", "lineHeight"], 3, "INVALID_APPEARANCE"],
    ];
    for (const [path, value, code] of preferenceCases) {
      const request = mutated(fixture.preferencesRequest, path, value);
      if (path.at(-1) === "showToolCalls") {
        const mutable = request as {preferences: {visibility: {showToolDetails: boolean}}};
        mutable.preferences.visibility.showToolDetails = true;
      }
      expectInvalid(validateSetSessionPreferencesRequest(request), code);
    }
  });

  it("keeps library-only and source-destructive deletion structurally distinct", () => {
    expectInvalid(
      validateDeleteIndexedSessionRequest(fixture.sourceDeletionRequest),
      "INVALID_DELETION_MODE",
    );
    expectInvalid(
      validateDeleteIndexedSessionWithSourceRequest(fixture.libraryDeletionRequest),
      "INVALID_DELETION_MODE",
    );
    expectInvalid(
      validateDeleteIndexedSessionWithSourceRequest({...fixture.sourceDeletionRequest, confirmationToken: ""}),
      "INVALID_CONFIRMATION_TOKEN",
    );
  });

  it("rejects impossible refresh counters", () => {
    expectInvalid(
      validateIndexRefreshState({...fixture.refreshState, processedCount: 13}),
      "INVALID_REFRESH_COUNTS",
    );
    expectInvalid(
      validateIndexRefreshState({...fixture.refreshState, indexedCount: 7}),
      "INVALID_REFRESH_COUNTS",
    );
  });

  it("accepts every refresh state and rejects status-specific fields", () => {
    const counts = {
      discoveredCount: 12,
      processedCount: 7,
      indexedCount: 2,
      unchangedCount: 4,
      failedCount: 1,
      skippedCount: 0,
      warningCount: 2,
    };
    expect(validateIndexRefreshState({
      schemaVersion: 1,
      status: "idle",
      lastCompletedAtMs: null,
    }).ok).toBe(true);
    expect(validateIndexRefreshState({
      schemaVersion: 1,
      status: "completed",
      generation: 4,
      startedAtMs: 1788163260000,
      completedAtMs: 1788163270000,
      ...counts,
    }).ok).toBe(true);
    expect(validateIndexRefreshState({
      schemaVersion: 1,
      status: "failed",
      generation: 4,
      startedAtMs: 1788163260000,
      completedAtMs: 1788163270000,
      errorCode: "SOURCE_READ_FAILED",
      ...counts,
    }).ok).toBe(true);
    expectInvalid(
      validateIndexRefreshState({
        schemaVersion: 1,
        status: "idle",
        lastCompletedAtMs: null,
        sourcePath: "C:\\private",
      }),
      "UNKNOWN_FIELD",
    );
  });

  it("validates refresh lifecycle and safe-code boundaries", () => {
    const runningCases: ReadonlyArray<readonly [string, unknown, string]> = [
      ["generation", 0, "INVALID_REFRESH_STATE"],
      ["generation", Number.MAX_SAFE_INTEGER + 1, "INVALID_REFRESH_STATE"],
      ["startedAtMs", Number.MAX_SAFE_INTEGER + 1, "INVALID_REFRESH_STATE"],
      ["discoveredCount", MAX_PRESENTATION_ENTRY_COUNT + 1, "INVALID_REFRESH_COUNTS"],
      ["indexedCount", 8, "INVALID_REFRESH_COUNTS"],
      ["unchangedCount", 8, "INVALID_REFRESH_COUNTS"],
      ["failedCount", 8, "INVALID_REFRESH_COUNTS"],
      ["skippedCount", 8, "INVALID_REFRESH_COUNTS"],
      ["warningCount", MAX_PRESENTATION_ENTRY_COUNT + 1, "INVALID_REFRESH_COUNTS"],
    ];
    for (const [field, value, code] of runningCases) {
      expectInvalid(validateIndexRefreshState({...fixture.refreshState, [field]: value}), code);
    }

    const completed = {
      schemaVersion: 1,
      status: "completed",
      generation: 4,
      startedAtMs: 200,
      completedAtMs: 100,
      discoveredCount: 1,
      processedCount: 1,
      indexedCount: 1,
      unchangedCount: 0,
      failedCount: 0,
      skippedCount: 0,
      warningCount: 0,
    };
    expectInvalid(validateIndexRefreshState(completed), "INVALID_TIMESTAMP");
    expectInvalid(
      validateIndexRefreshState({
        schemaVersion: 1,
        status: "idle",
        lastCompletedAtMs: Number.MAX_SAFE_INTEGER + 1,
      }),
      "INVALID_TIMESTAMP",
    );

    for (const errorCode of ["", "lowercase", "1_STARTS_WITH_DIGIT", "A".repeat(129)]) {
      expectInvalid(
        validateIndexRefreshState({...completed, status: "failed", completedAtMs: 201, errorCode}),
        "INVALID_ERROR_CODE",
      );
    }
  });

  it("validates deletion confirmation metadata and every operation version", () => {
    for (const artifactCount of [0, 65]) {
      expectInvalid(
        validateSourceDeletionConfirmation({...fixture.sourceDeletionConfirmation, artifactCount}),
        "INVALID_ARTIFACT_COUNT",
      );
    }
    expectInvalid(
      validateSourceDeletionConfirmation({
        ...fixture.sourceDeletionConfirmation,
        expiresAtMs: Number.MAX_SAFE_INTEGER + 1,
      }),
      "INVALID_TIMESTAMP",
    );

    for (const [, validate, value] of validators.slice(6, 11)) {
      expectInvalid(validate({...value as object, schemaVersion: 2}), "UNSUPPORTED_SCHEMA_VERSION");
    }
  });

  it("bounds path-free suppression pages and restore results", () => {
    expectInvalid(
      validateSuppressedSource({...fixture.suppressedSource, vendorSessionId: "private"}),
      "UNKNOWN_FIELD",
    );
    expectInvalid(
      validateSuppressedSource({...fixture.suppressedSource, sourceDeleted: "yes"}),
      "INVALID_SUPPRESSION",
    );
    expectInvalid(
      validateSuppressedSourceListRequest({...fixture.suppressedSourceListRequest, pageSize: 0}),
      "INVALID_PAGE_SIZE",
    );
    expectInvalid(
      validateSuppressedSourcePage({...fixture.suppressedSourcePage, nextCursor: "../private"}),
      "INVALID_OPAQUE_ID",
    );
    expectInvalid(
      validateRestoreSuppressedSourceResult({...fixture.restoreResult, sourcePath: "C:\\private"}),
      "UNKNOWN_FIELD",
    );
  });

  it("bounds presentation plans and requires deterministic reveal order", () => {
    expectInvalid(
      validatePresentationPlan({...fixture.presentationPlan, entries: []}),
      "EMPTY_PRESENTATION_PLAN",
    );
    expectInvalid(
      validatePresentationPlan({
        ...fixture.presentationPlan,
        entries: Array.from({length: MAX_PRESENTATION_ENTRY_COUNT + 1}, () => fixture.presentationPlan.entries[0]),
      }),
      "PRESENTATION_ENTRY_LIMIT_EXCEEDED",
    );
    const reversed = clone(fixture.presentationPlan);
    reversed.entries.reverse();
    expectInvalid(validatePresentationPlan(reversed), "INVALID_PRESENTATION_ORDER");
  });

  it("requires presentation entries to honor frozen visibility", () => {
    expectInvalid(
      validatePresentationPlan({
        ...fixture.presentationPlan,
        preferences: {
          ...fixture.presentationPlan.preferences,
          visibility: {
            ...fixture.presentationPlan.preferences.visibility,
            showReasoning: false,
          },
        },
      }),
      "PRESENTATION_VISIBILITY_MISMATCH",
    );
    const visibleDetail: unknown = clone(fixture.presentationPlan);
    const mutablePlan = visibleDetail as {
      entries: Array<{
        entry: {
          kind: string;
          detail?: {arguments: string | null; result: string | null} | null;
        };
      }>;
    };
    const tool = mutablePlan.entries.find(({entry}) => entry.kind === "tool-call");
    if (tool?.entry.kind !== "tool-call") {
      throw new Error("fixture must contain a tool call");
    }
    tool.entry.detail = {arguments: "fixture.json", result: "7 entries"};
    expectInvalid(validatePresentationPlan(visibleDetail), "PRESENTATION_VISIBILITY_MISMATCH");
  });

  it("validates presentation metadata, content, timing, and render boundaries", () => {
    const planCases: ReadonlyArray<readonly [readonly (string | number)[], unknown, string]> = [
      [["planId"], "../plan", "INVALID_OPAQUE_ID"],
      [["sessionId"], "../session", "INVALID_OPAQUE_ID"],
      [["revisionId"], "../revision", "INVALID_OPAQUE_ID"],
      [["sessionTitle"], "", "INVALID_PRESENTATION_PLAN"],
      [["createdAtMs"], Number.MAX_SAFE_INTEGER + 1, "INVALID_PRESENTATION_PLAN"],
      [["entries", 0, "entry", "ordinal"], MAX_PRESENTATION_ENTRY_COUNT, "INVALID_ENTRY"],
      [["entries", 0, "entry", "atMs"], 604_800_001, "INVALID_ENTRY"],
      [["entries", 0, "entry", "text"], "bad\0text", "INVALID_PRESENTATION_ENTRY"],
      [["entries", 1, "entry", "markdown"], "bad\0markdown", "INVALID_PRESENTATION_ENTRY"],
      [["entries", 2, "entry", "text"], "bad\0reasoning", "INVALID_PRESENTATION_ENTRY"],
      [["entries", 3, "entry", "name"], "", "INVALID_PRESENTATION_ENTRY"],
      [["entries", 3, "entry", "status"], "cancelled", "INVALID_PRESENTATION_ENTRY"],
      [["entries", 3, "entry", "summary"], "bad\0summary", "INVALID_PRESENTATION_ENTRY"],
      [["entries", 4, "entry", "displayPath"], "", "INVALID_PRESENTATION_ENTRY"],
      [["entries", 0, "revealAtMs"], 7_200_001, "INVALID_PRESENTATION_ENTRY"],
      [["entries", 1, "entry", "entryKey"], "entry_01", "INVALID_PRESENTATION_ORDER"],
      [["entries", 2, "entry", "atMs"], 500, "INVALID_PRESENTATION_ORDER"],
      [["entries", 2, "revealAtMs"], 500, "INVALID_PRESENTATION_ORDER"],
      [["durationMs"], 0, "INVALID_PRESENTATION_DURATION"],
      [["durationMs"], 7_200_001, "INVALID_PRESENTATION_DURATION"],
      [["durationMs"], 4_000, "INVALID_PRESENTATION_DURATION"],
      [["fps"], 60, "INVALID_RENDER_SETTINGS"],
      [["width"], 1280, "INVALID_RENDER_SETTINGS"],
      [["height"], 720, "INVALID_RENDER_SETTINGS"],
    ];
    for (const [path, value, code] of planCases) {
      expectInvalid(validatePresentationPlan(mutated(fixture.presentationPlan, path, value)), code);
    }

    expectInvalid(
      validatePresentationPlan(mutated(
        fixture.presentationPlan,
        ["preferences", "visibility", "showToolCalls"],
        false,
      )),
      "PRESENTATION_VISIBILITY_MISMATCH",
    );
    expectInvalid(
      validatePresentationPlan(mutated(
        fixture.presentationPlan,
        ["entries", 3, "entry", "detail"],
        {arguments: null, result: null},
      )),
      "INVALID_TOOL_DETAIL",
    );

    const withUnknown = mutated(
      fixture.presentationPlan,
      ["entries", 4, "entry"],
      {entryKey: "entry_07", ordinal: 6, atMs: 6_000, kind: "unknown", sourceType: "future-record"},
    );
    expect(validatePresentationPlan(withUnknown).ok).toBe(true);
    expectInvalid(
      validatePresentationPlan(mutated(withUnknown, ["entries", 4, "entry", "sourceType"], "")),
      "INVALID_PRESENTATION_ENTRY",
    );
  });
});

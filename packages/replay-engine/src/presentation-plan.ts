import {
  MAX_PRESENTATION_DURATION_MS,
  validatePresentationPlan,
  type IndexedSessionDetailV1,
  type PresentationEntryContentV1,
  type PresentationPlanV1,
  type SelectableEntryV1,
  type SessionEntryV1,
} from "@ai-session-replay/replay-contract";

export function projectPresentationPlan(
  detail: IndexedSessionDetailV1,
  entries: readonly SelectableEntryV1[],
  planId: string,
  createdAtMs: number,
): PresentationPlanV1 {
  if (
    detail.entryPage.nextCursor !== null ||
    entries.length !== detail.entryPage.totalEntryCount
  ) {
    throw new Error("Presentation requires the complete conversation");
  }
  if (detail.revision.revisionId !== detail.entryPage.revisionId) {
    throw new Error("Presentation revision does not match the loaded entries");
  }

  const projected = entries
    .filter(({selected, entry}) => selected && isVisible(entry, detail.preferences.visibility))
    .map(({entry}, index) => ({
      entry: projectEntry(entry, detail.preferences.visibility.showToolDetails),
      revealAtMs: Math.round(
        (index * detail.preferences.timing.entryDelayMs) /
          detail.preferences.timing.playbackSpeed,
      ),
    }));
  if (projected.length === 0) {
    throw new Error("Presentation has no visible selected entries");
  }
  const durationMs = Math.round(
    (projected.length * detail.preferences.timing.entryDelayMs) /
      detail.preferences.timing.playbackSpeed,
  );
  if (durationMs > MAX_PRESENTATION_DURATION_MS) {
    throw new Error("Presentation duration exceeds the supported limit");
  }
  const candidate: PresentationPlanV1 = {
    schemaVersion: 1,
    planId,
    sessionId: detail.summary.sessionId,
    revisionId: detail.revision.revisionId,
    sessionTitle: detail.summary.title,
    createdAtMs,
    entries: projected,
    preferences: detail.preferences,
    durationMs,
    fps: 30,
    width: 1_920,
    height: 1_080,
  };
  const validated = validatePresentationPlan(candidate);
  if (!validated.ok) throw new Error(validated.error.message);
  return validated.value;
}

function isVisible(
  entry: SessionEntryV1,
  visibility: IndexedSessionDetailV1["preferences"]["visibility"],
): boolean {
  if (entry.kind === "reasoning") return visibility.showReasoning;
  if (entry.kind === "tool-call") return visibility.showToolCalls;
  return true;
}

function projectEntry(
  entry: SessionEntryV1,
  showToolDetails: boolean,
): PresentationEntryContentV1 {
  if (entry.kind !== "tool-call") return entry;
  return {
    ...entry,
    detail:
      showToolDetails && entry.detail.availability === "available"
        ? {arguments: entry.detail.arguments, result: entry.detail.result}
        : null,
  };
}

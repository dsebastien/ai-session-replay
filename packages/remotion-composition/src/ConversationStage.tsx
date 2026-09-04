import type {PresentationPlanEntryV1, PresentationPlanV1} from "@ai-session-replay/replay-contract";
import {ConversationCard} from "./ConversationCard";

export interface ConversationStageProps {
  readonly plan: PresentationPlanV1;
  readonly visibleEntries: readonly PresentationPlanEntryV1[];
  readonly activeEntryKey: string | null;
  readonly visibleEntryCount: number;
  readonly interactive?: boolean;
}

export function PresentationFrame({plan, index, interactive = false}: Readonly<{plan: PresentationPlanV1; index: number; interactive?: boolean}>) {
  return <ConversationStage plan={plan} interactive={interactive} {...presentationStageState(plan, index)} />;
}

export function ConversationStage({plan, visibleEntries, activeEntryKey, visibleEntryCount, interactive = false}: ConversationStageProps) {
  const {theme, font} = plan.preferences.appearance;
  return (
    <div
      data-testid="conversation-stage"
      style={{
        alignItems: "center",
        backgroundColor: theme.background,
        boxSizing: "border-box",
        color: theme.text,
        display: "flex",
        fontFamily: font.family,
        fontSize: `clamp(12px, ${(font.sizePx / 19.2).toFixed(4)}vw, ${font.sizePx}px)`,
        height: "100%",
        justifyContent: "center",
        lineHeight: font.lineHeight,
        padding: "clamp(12px, 3.75vw, 72px)",
        width: "100%",
      }}
    >
      <section
        aria-label={`Terminal replay: ${plan.sessionTitle}`}
        style={{
          backgroundColor: theme.surface,
          border: `2px solid ${withAlpha(theme.muted, "42")}`,
          borderRadius: 18,
          boxShadow: "0 36px 100px rgba(0, 0, 0, 0.34)",
          display: "flex",
          flex: 1,
          flexDirection: "column",
          maxHeight: "100%",
          minHeight: 0,
          overflow: "hidden",
        }}
      >
        <header
          style={{
            alignItems: "center",
            borderBottom: `2px solid ${withAlpha(theme.muted, "30")}`,
            display: "grid",
            flex: "0 0 clamp(52px, 4.4792vw, 86px)",
            gridTemplateColumns: "minmax(76px, 160px) minmax(0, 1fr) minmax(70px, 240px)",
            padding: "0 clamp(12px, 1.7708vw, 34px)",
          }}
        >
          <div aria-hidden="true" style={{display: "flex", gap: 14}}>
            <WindowDot color={theme.error} />
            <WindowDot color={theme.accent} />
            <WindowDot color={theme.success} />
          </div>
          <strong style={{fontSize: 24, fontWeight: 400, overflow: "hidden", textAlign: "center", textOverflow: "ellipsis", whiteSpace: "nowrap"}}>
            {plan.sessionTitle}
          </strong>
          <span style={{color: theme.muted, fontSize: 19, textAlign: "right"}}>
            {visibleEntryCount} / {plan.entries.length}
          </span>
        </header>
        <div style={{display: "flex", flex: 1, flexDirection: "column", justifyContent: "flex-end", minHeight: 0, overflow: "hidden", padding: "clamp(12px, 1.875vw, 36px)", gap: 12}}>
          {visibleEntries.map(({entry}) => (
            <ConversationCard
              active={entry.entryKey === activeEntryKey}
              entry={entry}
              key={entry.entryKey}
              mode="video"
              scrollable={interactive}
              theme={theme}
            />
          ))}
        </div>
      </section>
    </div>
  );
}

export function presentationStageState(
  plan: PresentationPlanV1,
  index: number,
): Omit<ConversationStageProps, "plan"> {
  if (!Number.isSafeInteger(index) || index < 0 || index >= plan.entries.length) {
    throw new RangeError("Presentation index is outside the plan");
  }
  const visibleEntries = plan.entries.slice(index, index + 1);
  return {
    visibleEntries,
    visibleEntryCount: index + 1,
    activeEntryKey: visibleEntries.at(-1)?.entry.entryKey ?? null,
  };
}

function WindowDot({color}: Readonly<{color: string}>) {
  return <span style={{backgroundColor: color, borderRadius: "50%", height: 16, width: 16}} />;
}

function withAlpha(hexColor: string, alpha: string): string {
  return `${hexColor}${alpha}`;
}

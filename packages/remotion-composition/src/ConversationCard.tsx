import type {CSSProperties, ReactNode} from "react";
import type {
  PresentationEntryContentV1,
  SessionEntryV1,
  ThemeSpec,
} from "@ai-session-replay/replay-contract";
import {TerminalText} from "./TerminalText";

export interface ConversationCardProps {
  readonly entry: PresentationEntryContentV1 | SessionEntryV1;
  readonly theme: ThemeSpec;
  readonly active?: boolean;
  readonly mode: "conversation" | "video";
  readonly trailing?: ReactNode;
  readonly showToolDetails?: boolean;
  readonly scrollable?: boolean;
}

export function ConversationCard({entry, theme, active = false, mode, trailing, showToolDetails = true, scrollable = false}: ConversationCardProps) {
  const presentation = entryPresentation(entry);
  const detail = entry.kind === "tool-call" ? toolDetail(entry, showToolDetails) : null;
  return (
    <article
      className={`shared-entry-card shared-entry-card--${mode} shared-entry-card--${presentation.alignment}`}
      data-active={String(active)}
      data-kind={entry.kind}
      data-testid={`entry-${entry.entryKey}`}
      tabIndex={scrollable ? 0 : undefined}
      style={{
        "--entry-accent": presentation.tone === "danger" ? theme.error : presentation.tone === "success" ? theme.success : theme.accent,
        "--entry-background": presentation.alignment === "user" ? theme.surface : theme.background,
        "--entry-border": theme.muted,
        "--entry-muted": theme.muted,
        "--entry-text": theme.text,
        alignSelf: presentation.alignment === "user" ? "flex-end" : "flex-start",
        backgroundColor: presentation.alignment === "user" ? theme.surface : theme.background,
        border: `1px solid ${presentation.alignment === "user" ? theme.accent : theme.muted}`,
        borderRadius: 8,
        boxSizing: "border-box",
        boxShadow: active && mode === "video" ? `0 0 0 2px ${theme.accent}` : "none",
        color: theme.text,
        maxHeight: scrollable ? "100%" : undefined,
        minHeight: scrollable ? 0 : undefined,
        overflowY: scrollable ? "auto" : undefined,
        overscrollBehavior: scrollable ? "contain" : undefined,
        padding: mode === "video" ? "16px 20px" : undefined,
        width: mode === "video" || presentation.alignment === "supporting"
          ? "100%"
          : "min(82%, 760px)",
      } as CSSProperties}
    >
      <header style={{alignItems: "center", display: "flex", gap: 8}}>
        <strong style={{color: presentation.tone === "danger" ? theme.error : presentation.tone === "success" ? theme.success : theme.accent, fontSize: "0.7em", letterSpacing: "0.06em", textTransform: "uppercase"}}>{presentation.label}</strong>
        {trailing}
      </header>
      <p style={{margin: "8px 0 0", overflowWrap: "anywhere", whiteSpace: "pre-wrap"}}>
        <TerminalText text={presentation.body} theme={theme} />
      </p>
      {detail ? (
        <div className="conversation-entry__details" style={{marginTop: 8}}>
          {detail.arguments === null ? null : <pre style={{font: "inherit", margin: 0, overflowWrap: "anywhere", whiteSpace: "pre-wrap"}}><TerminalText text={detail.arguments} theme={theme} /></pre>}
          {detail.result === null ? null : <pre style={{font: "inherit", margin: "6px 0 0", overflowWrap: "anywhere", whiteSpace: "pre-wrap"}}><TerminalText text={detail.result} theme={theme} /></pre>}
        </div>
      ) : null}
    </article>
  );
}

export function entryPresentation(entry: PresentationPlanEntry): Readonly<{
  alignment: "user" | "assistant" | "supporting";
  label: string;
  body: string;
  tone: "accent" | "success" | "danger";
}> {
  switch (entry.kind) {
    case "user": return {alignment: "user", label: "You", body: entry.text, tone: "accent"};
    case "assistant": return {alignment: "assistant", label: "Assistant", body: entry.markdown, tone: "success"};
    case "reasoning": return {alignment: "supporting", label: "Reasoning", body: entry.text, tone: "accent"};
    case "tool-call": return {
      alignment: "supporting",
      label: `${entry.name} · ${entry.status}`,
      body: entry.summary,
      tone: entry.status === "failed" ? "danger" : entry.status === "succeeded" ? "success" : "accent",
    };
    case "file-change": return {alignment: "supporting", label: entry.displayPath, body: entry.summary, tone: "accent"};
    case "unknown": return {alignment: "supporting", label: "Unrecognized source event", body: entry.sourceType, tone: "accent"};
  }
}

type PresentationPlanEntry = PresentationEntryContentV1 | SessionEntryV1;

function toolDetail(
  entry: Extract<PresentationEntryContentV1 | SessionEntryV1, {kind: "tool-call"}>,
  show: boolean,
): Readonly<{arguments: string | null; result: string | null}> | null {
  if (!show || entry.detail === null) return null;
  if ("availability" in entry.detail) {
    return entry.detail.availability === "available"
      ? {arguments: entry.detail.arguments, result: entry.detail.result}
      : null;
  }
  return entry.detail;
}

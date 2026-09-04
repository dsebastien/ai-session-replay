import {loadFont} from "@remotion/fonts";
import {frameCount} from "@ai-session-replay/replay-engine";
import type {PresentationPlanEntryV1, PresentationPlanV1} from "@ai-session-replay/replay-contract";
import {AbsoluteFill, staticFile, useCurrentFrame, type CalculateMetadataFunction} from "remotion";
import {ConversationStage} from "./ConversationStage";

export const TERMINAL_REPLAY_ID = "TerminalReplay";
export const TERMINAL_REPLAY_FPS = 30;
export const TERMINAL_REPLAY_WIDTH = 1_920;
export const TERMINAL_REPLAY_HEIGHT = 1_080;
export const TERMINAL_FONT_FAMILY = "JetBrains Mono";

void loadFont({
  family: TERMINAL_FONT_FAMILY,
  url: staticFile("fonts/JetBrainsMono.woff2"),
  weight: "400",
});

export type TerminalReplayProps = Readonly<{plan: PresentationPlanV1}>;

export type TerminalFrameState = Readonly<{
  visibleEntries: readonly PresentationPlanEntryV1[];
  visibleEntryCount: number;
  activeEntryKey: string | null;
  elapsedMs: number;
}>;

export const calculateTerminalReplayMetadata: CalculateMetadataFunction<TerminalReplayProps> = ({props}) => ({
  durationInFrames: terminalReplayDurationInFrames(props.plan),
  fps: TERMINAL_REPLAY_FPS,
  width: TERMINAL_REPLAY_WIDTH,
  height: TERMINAL_REPLAY_HEIGHT,
});

export function terminalReplayDurationInFrames(plan: PresentationPlanV1): number {
  return frameCount(plan.durationMs, TERMINAL_REPLAY_FPS);
}

export function terminalFrameState(plan: PresentationPlanV1, frame: number): TerminalFrameState {
  if (!Number.isSafeInteger(frame) || frame < 0) {
    throw new RangeError("Frame must be a non-negative safe integer");
  }
  const elapsedMs = (frame / TERMINAL_REPLAY_FPS) * 1_000;
  let lower = 0;
  let upper = plan.entries.length;
  while (lower < upper) {
    const middle = Math.floor((lower + upper) / 2);
    if ((plan.entries[middle]?.revealAtMs ?? Number.POSITIVE_INFINITY) <= elapsedMs) lower = middle + 1;
    else upper = middle;
  }
  const visibleEntries = lower === 0 ? [] : plan.entries.slice(lower - 1, lower);
  return {
    visibleEntries,
    visibleEntryCount: lower,
    activeEntryKey: visibleEntries.at(-1)?.entry.entryKey ?? null,
    elapsedMs,
  };
}

export function TerminalReplay({plan}: TerminalReplayProps) {
  return <TerminalFrame frame={useCurrentFrame()} plan={plan} />;
}

export function TerminalFrame({frame, plan}: TerminalReplayProps & Readonly<{frame: number}>) {
  const state = terminalFrameState(plan, frame);

  return (
    <AbsoluteFill>
      <ConversationStage plan={plan} {...state} />
    </AbsoluteFill>
  );
}

import {
  DEFAULT_PRESENTATION_DELAY_MS,
  type PresentationPlanV1,
} from "@ai-session-replay/replay-contract";
import {AbsoluteFill, Composition, useCurrentFrame} from "remotion";
import {PresentationFrame} from "./ConversationStage";
import {
  calculateTerminalReplayMetadata,
  TERMINAL_REPLAY_FPS,
  TERMINAL_REPLAY_HEIGHT,
  TERMINAL_REPLAY_ID,
  TERMINAL_REPLAY_WIDTH,
  TerminalReplay,
  terminalFrameState,
  terminalReplayDurationInFrames,
  type TerminalReplayProps,
} from "./TerminalReplay";

const defaultPlan: PresentationPlanV1 = {
  schemaVersion: 1,
  planId: "preview-plan",
  sessionId: "preview-session",
  revisionId: "preview-revision",
  sessionTitle: "AI Session Replay",
  createdAtMs: 0,
  entries: [{
    entry: {entryKey: "preview-entry", ordinal: 0, atMs: 0, kind: "assistant", markdown: "Select a session to preview its replay."},
    revealAtMs: 0,
  }],
  preferences: {
    schemaVersion: 1,
    visibility: {showToolCalls: true, showToolDetails: false, showReasoning: true},
    timing: {entryDelayMs: DEFAULT_PRESENTATION_DELAY_MS, playbackSpeed: 1},
    appearance: {
      theme: {
        background: "#101211",
        surface: "#171A18",
        text: "#F5F5F4",
        muted: "#9B9E9C",
        accent: "#D6AA68",
        success: "#A8D5BD",
        error: "#E59A91",
      },
      font: {family: "JetBrains Mono", sizePx: 34, lineHeight: 1.5},
    },
  },
  durationMs: DEFAULT_PRESENTATION_DELAY_MS,
  fps: 30,
  width: 1_920,
  height: 1_080,
};

const defaultProps: TerminalReplayProps = {plan: defaultPlan};

function PresentationEquivalence({plan}: TerminalReplayProps) {
  const state = terminalFrameState(plan, useCurrentFrame());
  return <AbsoluteFill><PresentationFrame plan={plan} index={state.visibleEntryCount - 1} /></AbsoluteFill>;
}

export function RemotionRoot() {
  return (
    <>
      <Composition
        calculateMetadata={calculateTerminalReplayMetadata}
        component={TerminalReplay}
        defaultProps={defaultProps}
        durationInFrames={terminalReplayDurationInFrames(defaultPlan)}
        fps={TERMINAL_REPLAY_FPS}
        height={TERMINAL_REPLAY_HEIGHT}
        id={TERMINAL_REPLAY_ID}
        width={TERMINAL_REPLAY_WIDTH}
      />
      <Composition
        calculateMetadata={calculateTerminalReplayMetadata}
        component={PresentationEquivalence}
        defaultProps={defaultProps}
        durationInFrames={terminalReplayDurationInFrames(defaultPlan)}
        fps={TERMINAL_REPLAY_FPS}
        height={TERMINAL_REPLAY_HEIGHT}
        id="PresentationEquivalence"
        width={TERMINAL_REPLAY_WIDTH}
      />
    </>
  );
}

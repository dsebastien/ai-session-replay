export type SessionSource =
  | "claude-code"
  | "codex"
  | "copilot-cli"
  | "vscode-copilot";

export type DiagnosticSeverity = "info" | "warning" | "error";

export interface SourceDiagnostic {
  readonly code: string;
  readonly severity: DiagnosticSeverity;
  readonly message: string;
}

export interface SessionRelationship {
  readonly kind: "parent" | "fork";
  readonly sessionId: string;
}

export type ReplayEvent =
  | Readonly<{id: string; atMs: number; kind: "user"; text: string}>
  | Readonly<{id: string; atMs: number; kind: "assistant"; markdown: string}>
  | Readonly<{
      id: string;
      atMs: number;
      kind: "tool";
      name: string;
      status: "running" | "succeeded" | "failed";
      summary: string;
    }>
  | Readonly<{
      id: string;
      atMs: number;
      kind: "file-change";
      path: string;
      summary: string;
    }>
  | Readonly<{
      id: string;
      atMs: number;
      kind: "unknown";
      sourceType: string;
    }>;

export interface NormalizedSessionV1 {
  readonly schemaVersion: 1;
  readonly id: string;
  readonly source: SessionSource;
  readonly sourceVersion: string | null;
  readonly title: string;
  readonly createdAt: string | null;
  readonly cwd: string | null;
  readonly relationships: readonly SessionRelationship[];
  readonly events: readonly ReplayEvent[];
  readonly durationMs: number;
  readonly diagnostics: readonly SourceDiagnostic[];
  readonly unknownRecordCount: number;
}

export interface SpeedSegment {
  readonly id: string;
  readonly sourceStartMs: number;
  readonly sourceEndMs: number;
  readonly speed: number;
}

export interface ThemeSpec {
  readonly background: string;
  readonly surface: string;
  readonly text: string;
  readonly muted: string;
  readonly accent: string;
  readonly success: string;
  readonly error: string;
}

export interface FontSpec {
  readonly family: "JetBrains Mono";
  readonly sizePx: number;
  readonly lineHeight: number;
}

export interface ReplayProjectV1 {
  readonly schemaVersion: 1;
  readonly session: NormalizedSessionV1;
  readonly trim: Readonly<{startMs: number; endMs: number}>;
  readonly segments: readonly SpeedSegment[];
  readonly theme: ThemeSpec;
  readonly font: FontSpec;
  readonly terminalHoldMs: number;
  readonly fps: 30;
  readonly width: 1920;
  readonly height: 1080;
}

export interface ValidationError {
  readonly code: string;
  readonly message: string;
}

export type NormalizedSessionValidationResult =
  | Readonly<{ok: true; value: NormalizedSessionV1}>
  | Readonly<{ok: false; error: ValidationError}>;

export type ValidationResult =
  | Readonly<{ok: true; value: ReplayProjectV1}>
  | Readonly<{ok: false; error: ValidationError}>;

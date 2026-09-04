import {
  Bot,
  GitFork,
  Blocks,
  Library,
  Monitor,
  PanelLeftClose,
  PanelLeftOpen,
  Terminal,
} from "lucide-react";
import type {ReactNode} from "react";
import type {
  IndexedSessionSourceV1,
  IndexedSessionSummaryV1,
  JetBrainsCopilotStatusV1,
} from "../../../packages/replay-contract/src";

export type SessionSourceFilter = IndexedSessionSourceV1 | "all";

interface SourceSidebarProps {
  readonly entries: readonly IndexedSessionSummaryV1[];
  readonly selectedSource: SessionSourceFilter;
  readonly collapsed: boolean;
  readonly jetBrainsStatus: JetBrainsCopilotStatusV1 | null;
  readonly onSelectSource: (source: SessionSourceFilter) => void;
  readonly onToggleCollapsed: () => void;
}

interface SourceDefinition {
  readonly id: IndexedSessionSourceV1;
  readonly label: string;
  readonly note: string;
  readonly icon: ReactNode;
}

const sources: readonly SourceDefinition[] = [
  {
    id: "claude-code",
    label: "Claude Code",
    note: "Indexed conversations",
    icon: <Bot aria-hidden="true" size={17} strokeWidth={1.75} />,
  },
  {
    id: "codex",
    label: "Codex CLI",
    note: "Active and archived",
    icon: <Terminal aria-hidden="true" size={17} strokeWidth={1.75} />,
  },
  {
    id: "copilot-cli",
    label: "GitHub Copilot CLI",
    note: "Indexed session-state",
    icon: <GitFork aria-hidden="true" size={17} strokeWidth={1.75} />,
  },
  {
    id: "vscode-copilot",
    label: "VS Code Copilot Chat",
    note: "Stable and Insiders",
    icon: <Monitor aria-hidden="true" size={17} strokeWidth={1.75} />,
  },
];

export function SourceSidebar({
  entries,
  selectedSource,
  collapsed,
  jetBrainsStatus,
  onSelectSource,
  onToggleCollapsed,
}: SourceSidebarProps) {
  return (
    <aside
      className={`source-sidebar${collapsed ? " source-sidebar--collapsed" : ""}`}
      aria-label="Session sources"
    >
      <div className="source-sidebar__heading">
        <div className="source-sidebar__heading-copy">
          <p className="eyebrow">Sources</p>
          <p>{entries.length} shown</p>
        </div>
        <button
          className="source-sidebar__toggle"
          type="button"
          aria-controls="session-source-navigation"
          aria-expanded={!collapsed}
          aria-label={collapsed ? "Expand sources" : "Collapse sources"}
          title={collapsed ? "Expand sources" : "Collapse sources"}
          onClick={onToggleCollapsed}
        >
          {collapsed ? (
            <PanelLeftOpen aria-hidden="true" size={17} />
          ) : (
            <PanelLeftClose aria-hidden="true" size={17} />
          )}
        </button>
      </div>
      <nav id="session-source-navigation" aria-label="Filter by source">
        <button
          className="source-option source-option--all"
          type="button"
          aria-label="Show all sessions"
          aria-pressed={selectedSource === "all"}
          onClick={() => onSelectSource("all")}
        >
          <span className="source-option__icon">
            <Library aria-hidden="true" size={17} strokeWidth={1.75} />
          </span>
          <span className="source-option__copy">
            <strong>All sessions</strong>
          </span>
        </button>
        {sources.map((source) => (
          <button
            key={source.id}
            className={`source-option source-option--${source.id}`}
            type="button"
            aria-label={`Filter ${source.label} sessions`}
            aria-pressed={selectedSource === source.id}
            onClick={() => onSelectSource(source.id)}
          >
            <span className="source-option__icon">{source.icon}</span>
            <span className="source-option__copy">
              <strong>{source.label}</strong>
              <small>{source.note}</small>
            </span>
          </button>
        ))}
      </nav>
      <section
        className="source-integration-status"
        aria-label="JetBrains Copilot status"
        title={jetBrainsStatusLabel(jetBrainsStatus)}
      >
        <span className="source-option__icon">
          <Blocks aria-hidden="true" size={17} strokeWidth={1.75} />
        </span>
        <span className="source-integration-status__copy">
          <strong>JetBrains Copilot</strong>
          <small>{jetBrainsStatusLabel(jetBrainsStatus)}</small>
          <small>Native chat transcripts unavailable</small>
        </span>
      </section>
      <p className="source-sidebar__scope">
        Indexed copies remain available when their source is offline.
      </p>
    </aside>
  );
}

export function jetBrainsStatusLabel(status: JetBrainsCopilotStatusV1 | null): string {
  if (status === null) return "Checking integration…";
  if (status.copilotCliStatus === "jetbrains-attributed") {
    return "JetBrains-attributed CLI session found";
  }
  if (status.pluginStatus === "unavailable" && status.copilotCliStatus === "unavailable") {
    return "Detection unavailable";
  }
  if (status.pluginStatus === "detected" && status.copilotCliStatus === "unavailable") {
    return "Plugin found · CLI detection unavailable";
  }
  if (status.pluginStatus === "unavailable" && status.copilotCliStatus === "available-separately") {
    return "Copilot CLI separate · plugin detection unavailable";
  }
  if (status.pluginStatus === "detected" && status.copilotCliStatus === "available-separately") {
    return "Plugin found · Copilot CLI separate";
  }
  if (status.pluginStatus === "detected") return "Plugin found";
  if (status.pluginStatus === "unavailable") return "Plugin detection unavailable";
  if (status.copilotCliStatus === "unavailable") return "CLI detection unavailable";
  if (status.copilotCliStatus === "available-separately") return "Copilot CLI available separately";
  return "Not detected";
}

export function sourceLabel(source: IndexedSessionSourceV1): string {
  return sources.find((candidate) => candidate.id === source)?.label ?? "Session";
}

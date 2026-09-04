import {ShieldCheck} from "lucide-react";
import {SessionBrowser} from "./features/sessions/SessionBrowser";

export function App() {
  return (
    <main className="app-shell">
      <header className="titlebar">
        <div>
          <p className="eyebrow">Local session studio</p>
          <h1>AI Session Replay</h1>
        </div>
        <span className="privacy-badge">
          <ShieldCheck aria-hidden="true" size={14} />
          On-device
        </span>
      </header>
      <SessionBrowser />
    </main>
  );
}

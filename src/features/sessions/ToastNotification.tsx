import {X} from "lucide-react";
import {useState} from "react";

export function ToastNotification({message}: Readonly<{message: string}>) {
  const [dismissed, setDismissed] = useState(false);
  if (dismissed) return null;
  return (
    <aside className="toast-notification" role="alert" aria-live="assertive">
      <span>{message}</span>
      <button type="button" aria-label="Dismiss notification" onClick={() => setDismissed(true)}>
        <X aria-hidden="true" size={15} />
      </button>
    </aside>
  );
}

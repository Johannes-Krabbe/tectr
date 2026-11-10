import { useToasts } from "../toast";

/** What just happened, for a few seconds, over everything else in the window. It never takes
 * a click or the keyboard. */
export function Toasts() {
  const toasts = useToasts();
  return (
    <div className="toasts" role="status" aria-live="polite">
      {toasts.map((toast) => (
        <div key={toast.id} className="toast">
          {toast.text}
        </div>
      ))}
    </div>
  );
}

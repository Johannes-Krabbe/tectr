import { useEffect, useState } from "react";
import { type Device, request } from "./engine";
import { cloneProgress, homeShort, isOnline } from "./model";

/** The folder name git gives a clone of `url`, for before the daemon has said where it goes. */
const nameOf = (url: string) =>
  url.replace(/\/+$/, "").split(/[/:]/).pop()?.replace(/\.git$/, "") || url;

/** What a workspace being cloned shows until it's one: how far along git is, or why it failed,
 * with Try again and Dismiss. Adding a workspace by cloning lands here, so there's no moment
 * somewhere else. It follows the clone by its address, because only the daemon knows the folder's
 * full path, and once the clone is a workspace it hands over to that workspace's page. */
export function ClonePage({
  device,
  url,
  onCloned,
  onGone,
}: {
  device: Device;
  url: string;
  onCloned: (workspaceId: string) => void;
  /** Dismissed, or gone some other way without becoming a workspace. */
  onGone: () => void;
}) {
  const state = device.state;
  const machineId = state?.machine.id;
  const online = isOnline(device);
  const clone = state?.clones?.find((c) => c.url === url);
  // Where it was going, which is what finds the workspace once the clone is gone.
  const [path, setPath] = useState<string>();
  useEffect(() => {
    if (clone) setPath(clone.path);
  }, [clone?.path]);

  const gone = !clone && path !== undefined;
  const workspaceId = gone ? state?.workspaces.find((w) => w.path === path)?.id : undefined;
  useEffect(() => {
    if (workspaceId) onCloned(workspaceId);
    else if (gone) onGone();
  }, [workspaceId, gone]);

  const progress = clone ? cloneProgress(clone) : { text: "Starting", failed: false };
  const name = clone?.name ?? nameOf(url);

  const send = (command: "clone_workspace" | "dismiss_clone") => {
    if (!machineId || !online || !clone) return;
    // A clone that fails again says so here, through the daemon, so there's nothing to catch.
    const sent =
      command === "clone_workspace"
        ? request(machineId, { command, url: clone.url, path: clone.path })
        : request(machineId, { command, path: clone.path });
    sent.catch(() => {});
  };

  return (
    <div className="pane settings workspace-page" tabIndex={-1}>
      <div className="settings-in workspace-in">
        <header className="workspace-head">
          <div className="workspace-heading">
            <h1 className="workspace-title">{name}</h1>
            <p className="workspace-sub">
              <span className="code">{clone ? homeShort(clone.path) : url}</span>
            </p>
          </div>
        </header>
        <section className="workspace-section clone-status">
          {progress.failed ? (
            <>
              <p className="error">{clone?.error}</p>
              <div className="clone-actions">
                <button className="button" disabled={!online} onClick={() => send("dismiss_clone")}>
                  Dismiss
                </button>
                <button className="button primary" disabled={!online} onClick={() => send("clone_workspace")}>
                  Try again
                </button>
              </div>
            </>
          ) : (
            <>
              <div
                className="clone-bar"
                role="progressbar"
                aria-label={`Cloning ${name}`}
                aria-valuemin={0}
                aria-valuemax={100}
                aria-valuenow={clone?.percent}
              >
                <div className="clone-bar-fill" style={{ width: `${clone?.percent ?? 0}%` }} />
              </div>
              <p className="meta">{clone?.phase ? `${clone.phase}, ${clone.percent ?? 0}%` : progress.text}</p>
            </>
          )}
        </section>
      </div>
    </div>
  );
}

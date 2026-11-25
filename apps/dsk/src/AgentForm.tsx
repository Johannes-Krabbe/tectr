import { type ReactNode, type RefObject, useRef, useState } from "react";
import { type Device, errorText, type PermissionMode, type PromptImage } from "./engine";
import { ImageComposer, ImageViewer, useImageField } from "./ImageField";
import { sendImages } from "./images";
import { textBoxKey, titled } from "./keybinds";
import { defaultEffort, defaultMode, isOnline, modelLabel, MODES, settingsLabel } from "./model";
import { refusedPrompt } from "./slashCommands";
import { MenuButton, type MenuItem } from "./ui/Menu";
import { Choice, Picker } from "./ui/Picker";
import { useVim, VimModeLabel } from "./VimBox";

/** What's been chosen and typed for a new agent so far. Only what was changed is here: the
 * rest comes from the provider's defaults, and a choice that doesn't apply to the current
 * device or model is ignored. It belongs to this client and isn't saved anywhere, so
 * restarting clears it. */
export interface AgentDraft {
  text?: string;
  /** The images the text names. */
  images?: PromptImage[];
  agent?: string;
  model?: string;
  effort?: string;
  mode?: PermissionMode;
}

/** Everything starting the agent needs, with the defaults filled in. */
export interface AgentChoice {
  agent: string;
  mode: PermissionMode;
  model: string | null;
  effort: string | null;
  text: string;
  /** Already given to the device the agent starts on. */
  images: PromptImage[];
}

/** What a screen's own picker needs to close back to the message box. */
export interface PickerControl {
  finalFocus: RefObject<HTMLTextAreaElement | null>;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

const NO_IMAGES: PromptImage[] = [];

/** The first prompt for a new agent and what it runs on. The new topic screen and a new tab
 * share it, so starting an agent looks and works the same wherever it happens. The pickers have
 * no keys of their own: Tab from the prompt reaches them. */
export function AgentForm({
  device,
  draft,
  onDraft,
  ready = true,
  autoFocus = false,
  selects,
  onStart,
  onCancel,
  onSettings,
}: {
  device: Device | undefined;
  draft: AgentDraft;
  onDraft: (change: AgentDraft) => void;
  /** Whatever else the screen needs before it can start, such as a workspace. */
  ready?: boolean;
  autoFocus?: boolean;
  selects?: (control: (name: string) => PickerControl) => ReactNode;
  /** Starts the agent. A thrown error stays on the form, so it can be tried again. */
  onStart: (choice: AgentChoice) => Promise<void>;
  onCancel: () => void;
  onSettings: (machineId: string) => void;
}) {
  const text = draft.text ?? "";
  const [picking, setPicking] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const input = useRef<HTMLTextAreaElement>(null);
  const vim = useVim(input);
  const [viewing, setViewing] = useState<{ images: PromptImage[]; start: number } | null>(null);

  const machineId = device?.state?.machine.id;
  const field = useImageField({
    machineId,
    input,
    text,
    known: draft.images ?? NO_IMAGES,
    sent: 0,
    onError: setError,
    onOpen: (images, start) => setViewing({ images, start }),
  });
  const online = device !== undefined && isOnline(device);
  const providers = device?.state?.providers ?? [];
  const provider = providers.find((p) => p.agent === draft.agent) ?? providers[0];
  const model =
    provider?.models.find((m) => m.id === draft.model) ??
    provider?.models.find((m) => m.id === provider.default_model) ??
    provider?.models[0];
  const efforts = model?.efforts ?? [];
  const effort =
    efforts.find((e) => e.id === draft.effort) ??
    (provider && model ? defaultEffort(provider, model) : undefined);
  const modes = MODES.filter((option) => provider?.modes.includes(option.mode));
  const mode =
    modes.find((option) => option.mode === draft.mode) ??
    modes.find((option) => option.mode === defaultMode(provider?.modes ?? []));

  const settingItems: MenuItem[] = [];
  if (provider && providers.length > 1) {
    settingItems.push({
      id: "agent",
      label: "Agent",
      value: provider.name,
      run: () => {},
      items: providers.map((p) => ({
        id: p.agent,
        label: p.name,
        checked: p.agent === provider.agent,
        run: () => onDraft({ agent: p.agent }),
      })),
    });
  }
  if (provider && model) {
    settingItems.push({
      id: "model",
      label: "Model",
      value: model.name,
      run: () => {},
      items: provider.models.map((m) => ({
        id: m.id,
        label: modelLabel(provider, m),
        checked: m.id === model.id,
        run: () => onDraft({ model: m.id }),
      })),
    });
  }
  if (efforts.length > 0) {
    settingItems.push({
      id: "effort",
      label: "Effort",
      value: effort?.name,
      run: () => {},
      items: efforts.map((e) => ({
        id: e.id,
        label: e.name,
        checked: e.id === effort?.id,
        run: () => onDraft({ effort: e.id }),
      })),
    });
  }


  const canStart =
    ready && online && provider !== undefined && mode !== undefined && text.trim() !== "" && !busy;

  const control = (name: string): PickerControl => ({
    finalFocus: input,
    open: picking === name,
    onOpenChange: (open) => setPicking(open ? name : null),
  });

  async function start() {
    if (!canStart) return;
    const prompt = text.trim();
    // Checked before anything is made, so a refused prompt doesn't leave a tab behind.
    const refused = refusedPrompt(prompt);
    if (refused) {
      setError(refused);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      // Pasted before the workspace was chosen, or on another device, they aren't there yet.
      // Nothing on the daemon names them until now, so it may have removed them since.
      const images = field.imagesFor(prompt);
      if (machineId !== undefined) await sendImages(machineId, images, { again: true });
      await onStart({
        agent: provider.agent,
        mode: mode.mode,
        model: model?.id ?? null,
        effort: effort?.id ?? null,
        text: prompt,
        images,
      });
    } catch (reason) {
      setError(errorText(reason));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form
      className="pane create"
      onSubmit={(event) => {
        event.preventDefault();
        void start();
      }}
    >
      <div className="create-in">
        {selects?.(control)}
        <div className="composer">
          <ImageComposer field={field} text={text} input={input} machineId={machineId}>
            <textarea
              ref={input}
              autoFocus={autoFocus}
              readOnly={busy}
              value={text}
              placeholder="What should the agent do?"
              aria-label="First prompt"
              autoCorrect="off"
              autoCapitalize="off"
              onChange={(event) =>
                onDraft({ text: event.target.value, images: field.imagesFor(event.target.value) })
              }
              {...vim.props}
              onPaste={(event) => {
                if (!field.onPaste(event)) vim.props.onPaste(event);
              }}
              onKeyDown={(event) => {
                if (field.onKeyDown(event, vim.shown)) return;
                if (vim.onKeyDown(event)) return;
                textBoxKey(
                  event,
                  ["new_topic.start", () => void start()],
                  "new_topic.newline",
                  ["new_topic.cancel", onCancel],
                );
              }}
            />
          </ImageComposer>
          <div className="composer-pickers">
            {provider ? (
              <>
                <MenuButton
                  label="Agent settings"
                  title="Agent, model and effort"
                  className="picker"
                  align="start"
                  groups={[settingItems]}
                  returnFocus={input}
                >
                  {settingsLabel({
                    // Which agent only needs saying when there's a choice of them.
                    model: [providers.length > 1 ? provider.name : null, model?.name ?? "Default model"]
                      .filter(Boolean)
                      .join(" "),
                    effort: effort?.name,
                    changed: [],
                  })}
                </MenuButton>
                {mode && (
                  <Picker
                    {...control("mode")}
                    label="Permission mode"
                    title="Permission mode"
                    className="picker"
                    value={mode.mode}
                    onChange={(value) => {
                      const chosen = modes.find((option) => option.mode === value);
                      if (chosen) onDraft({ mode: chosen.mode });
                    }}
                    trigger={mode.label}
                  >
                    {modes.map((option) => (
                      <Choice key={option.mode} value={option.mode} label={option.label} />
                    ))}
                  </Picker>
                )}
              </>
            ) : (
              <button
                type="button"
                className="picker"
                disabled={!online || machineId === undefined}
                onClick={() => machineId && onSettings(machineId)}
              >
                Add a provider
              </button>
            )}
            <VimModeLabel mode={vim.shown} />
          </div>
        </div>
        <button
          className="button primary create-start"
          type="submit"
          disabled={!canStart}
          title={titled("Start", "new_topic.start")}
        >
          {busy ? "Starting" : "Start"}
        </button>
        {device?.state && !online && (
          <p className="error">{device.state.machine.name} is offline</p>
        )}
        {error && <p className="error">{error}</p>}
      </div>
      {viewing && (
        <ImageViewer
          machineId={machineId}
          images={viewing.images}
          start={viewing.start}
          onClose={() => setViewing(null)}
        />
      )}
    </form>
  );
}

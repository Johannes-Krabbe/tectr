// Images in the message box and the transcript. Pasting an image puts `[Image #3]` in the
// text where the cursor is, and the text stays a plain textarea, so vim, undo and drafts
// work as before. A layer behind it marks the tokens, and the tray above shows what's
// attached.

import {
  type ClipboardEvent,
  type DragEvent,
  type KeyboardEvent,
  type ReactNode,
  type RefObject,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { shownValue } from "./appearance";
import { errorText, type PromptImage } from "./engine";
import {
  byNumber,
  imagesIn,
  insertion,
  nextNumber,
  splitTokens,
  tokenAt,
  tokenBeside,
  tokensIn,
} from "./imageTokens";
import { imageFiles, prepareImage, sendImages, useImageUrl } from "./images";
import { act, keysLabel, useSettings } from "./keybinds";
import { focusPane } from "./keys";
import { Icon } from "./ui/Icon";
import type { VimMode } from "./vim";

export type OpenImages = (images: PromptImage[], index: number) => void;

/** An image being pointed at, or beside the cursor, and what to show it next to. */
interface Peek {
  image: PromptImage;
  anchor: Element;
  /** It came from the cursor, so the key that opens it is worth showing. */
  fromCursor?: boolean;
}

const PEEK_WIDTH = 320;
const PEEK_HEIGHT = 200;

/** Shown above what it's about, or below when there's no room above. */
function ImagePeek({ machineId, peek }: { machineId: string | undefined; peek: Peek }) {
  const box = useRef<HTMLDivElement>(null);
  const { image } = peek;
  const url = useImageUrl(machineId, image, "thumbnail");
  const scale = Math.min(PEEK_WIDTH / image.width, PEEK_HEIGHT / image.height, 1);

  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    const anchor = peek.anchor.getBoundingClientRect();
    const gap = 8;
    let top = anchor.top - el.offsetHeight - gap;
    if (top < gap) top = anchor.bottom + gap;
    const centered = anchor.left + anchor.width / 2 - el.offsetWidth / 2;
    const left = Math.max(gap, Math.min(centered, window.innerWidth - el.offsetWidth - gap));
    el.style.top = `${top}px`;
    el.style.left = `${left}px`;
  });

  const open = keysLabel("composer.open_image");
  return (
    <div className="image-peek" ref={box} aria-hidden="true">
      <div
        className="image-frame"
        style={{ width: Math.round(image.width * scale), height: Math.round(image.height * scale) }}
      >
        {url && <img src={url} alt="" />}
      </div>
      <div className="image-caption">
        <b>Image #{image.number}</b>
        <span>
          {image.width} x {image.height}
        </span>
        {peek.fromCursor && open && <span className="image-keys">{open} to open</span>}
      </div>
    </div>
  );
}

function ImageThumb({
  machineId,
  image,
  active,
  onOpen,
  onRemove,
  onPeek,
}: {
  machineId: string | undefined;
  image: PromptImage;
  /** Its token is pointed at or has the cursor. */
  active: boolean;
  onOpen: () => void;
  onRemove?: () => void;
  onPeek: (peek: Peek | null) => void;
}) {
  const url = useImageUrl(machineId, image, "thumbnail");
  return (
    <div
      className={`image-thumb ${active ? "on" : ""}`}
      onMouseEnter={(event) => onPeek({ image, anchor: event.currentTarget })}
      onMouseLeave={() => onPeek(null)}
    >
      <button
        type="button"
        className="image-thumb-open"
        tabIndex={-1}
        aria-label={`Open image #${image.number}`}
        // The message box keeps the keyboard, and its cursor.
        onMouseDown={(event) => event.preventDefault()}
        onClick={onOpen}
      >
        {url && <img src={url} alt="" />}
      </button>
      <span className="image-number">#{image.number}</span>
      {onRemove && (
        <button
          type="button"
          className="image-remove"
          tabIndex={-1}
          aria-label={`Remove image #${image.number}`}
          onMouseDown={(event) => event.preventDefault()}
          onClick={onRemove}
        >
          <Icon name="remove" />
        </button>
      )}
    </div>
  );
}

/** Whether a setting that shows thumbnails is on. Both are on unless the file says not. */
function useThumbnails(
  id: "conversation.images.message_box" | "conversation.images.transcript",
): boolean {
  useSettings();
  return shownValue(id) !== false;
}

export interface ImageField {
  /** Every image the box knows, by number, whether or not the text still names it. */
  known: ReadonlyMap<number, PromptImage>;
  /** The images the text names, in the order it first names them. */
  images: PromptImage[];
  /** The same for text that isn't on screen yet, such as a draft about to be saved. */
  imagesFor(text: string): PromptImage[];
  /** Forgets the images pasted here, once they've been sent. */
  clear(): void;
  /** Takes a paste of images. Says whether it took it. */
  onPaste(event: ClipboardEvent<HTMLTextAreaElement>): boolean;
  /** Removing a token whole, and opening images. Says whether it took the press. */
  onKeyDown(event: KeyboardEvent<HTMLTextAreaElement>, vim: VimMode | null): boolean;
  /** Opens the image beside the cursor, or every image in the box. */
  openAtCursor(): void;
  remove(number: number): void;
  attach(files: File[]): void;
  onOpen: OpenImages;
}

export function useImageField({
  machineId,
  input,
  text,
  known: kept,
  sent,
  onError,
  onOpen,
}: {
  /** Where pasted images go straight away. Without one they wait for `sendImages`. */
  machineId: string | undefined;
  input: RefObject<HTMLTextAreaElement | null>;
  text: string;
  /** The images the draft already had, from the daemon or the form. */
  known: readonly PromptImage[];
  /** The highest number the session has sent so far. */
  sent: number;
  onError: (message: string) => void;
  onOpen: OpenImages;
}): ImageField {
  const [pasted, setPasted] = useState<ReadonlyMap<number, PromptImage>>(() => new Map());
  const keptKey = kept.map((image) => `${image.number}:${image.id}`).join(",");
  // What was pasted here wins, because the daemon may not have heard of it yet.
  const known = useMemo(() => new Map([...byNumber(kept), ...pasted]), [keptKey, pasted]);
  const latest = useRef(known);
  latest.current = known;
  const images = useMemo(() => imagesIn(text, known), [text, known]);

  /** Cuts every token for the image out of the text, through the box's editing, so undo
   * brings it back. */
  const remove = (number: number) => {
    const el = input.current;
    if (!el) return;
    el.focus();
    for (const token of tokensIn(el.value).reverse()) {
      if (token.number !== number) continue;
      const end = el.value[token.end] === " " ? token.end + 1 : token.end;
      el.setSelectionRange(token.start, end);
      document.execCommand("delete");
    }
  };

  const attach = async (files: File[]) => {
    for (const file of files) {
      try {
        const prepared = await prepareImage(file);
        if (machineId !== undefined) await sendImages(machineId, [{ ...prepared, number: 0 }]);
        const el = input.current;
        if (!el) return;
        const number = nextNumber(sent, imagesIn(el.value, latest.current));
        const image = { ...prepared, number };
        // Known before the text names it, because saving the draft reads it straight away.
        latest.current = new Map(latest.current).set(number, image);
        setPasted((before) => new Map(before).set(number, image));
        el.focus();
        const { selectionStart: start, selectionEnd: end, value } = el;
        const rest = value.slice(0, start) + value.slice(end);
        document.execCommand("insertText", false, insertion(rest, start, number));
      } catch (reason) {
        onError(reason instanceof Error ? reason.message : errorText(reason));
      }
    }
  };

  const openAtCursor = () => {
    const el = input.current;
    if (!el || images.length === 0) return;
    const token = el.selectionStart === el.selectionEnd ? tokenAt(el.value, el.selectionStart) : undefined;
    const image = token && known.get(token.number);
    onOpen(images, image ? Math.max(0, images.indexOf(image)) : 0);
  };

  return {
    known,
    images,
    imagesFor: (next) => imagesIn(next, latest.current),
    clear: () => setPasted(new Map()),
    onPaste(event) {
      const files = imageFiles(event.clipboardData);
      if (files.length === 0) return false;
      event.preventDefault();
      void attach(files);
      return true;
    },
    onKeyDown(event, vim) {
      if (act(event, "composer.open_image", openAtCursor)) return true;
      // Vim's own keys decide what Backspace does outside insert mode.
      if (vim !== null && vim !== "insert") return false;
      const forward = event.key === "Delete";
      if (!forward && event.key !== "Backspace") return false;
      if (event.altKey || event.metaKey || event.ctrlKey) return false;
      const el = event.currentTarget;
      if (el.selectionStart !== el.selectionEnd) return false;
      const token = tokenBeside(el.value, el.selectionStart, forward);
      if (!token || !known.has(token.number)) return false;
      event.preventDefault();
      el.setSelectionRange(token.start, token.end);
      document.execCommand("delete");
      return true;
    },
    openAtCursor,
    remove,
    attach: (files) => void attach(files),
    onOpen,
  };
}

/** The message box around its textarea: the tray of attached images above it, the marks
 * behind it, and the image the pointer or cursor is on. It tracks the pointer and cursor
 * itself, so moving them doesn't re-render the transcript. */
export function ImageComposer({
  field,
  text,
  input,
  machineId,
  children,
}: {
  field: ImageField;
  text: string;
  input: RefObject<HTMLTextAreaElement | null>;
  machineId: string | undefined;
  children: ReactNode;
}) {
  const marks = useRef<HTMLDivElement>(null);
  const [cursor, setCursor] = useState<number | null>(null);
  const [pointed, setPointed] = useState<Peek | null>(null);
  const showTray = useThumbnails("conversation.images.message_box");

  useEffect(() => {
    const el = input.current;
    if (!el) return;
    const track = () => {
      const focused = document.activeElement === el && el.selectionStart === el.selectionEnd;
      setCursor(focused ? el.selectionStart : null);
      if (marks.current) marks.current.scrollTop = el.scrollTop;
    };
    const point = (event: MouseEvent) => {
      const hit = [...(marks.current?.querySelectorAll<HTMLElement>("mark[data-n]") ?? [])].find(
        (mark) =>
          [...mark.getClientRects()].some(
            (rect) =>
              event.clientX >= rect.left &&
              event.clientX <= rect.right &&
              event.clientY >= rect.top &&
              event.clientY <= rect.bottom,
          ),
      );
      const image = hit && field.known.get(Number(hit.dataset.n));
      setPointed((before) =>
        image ? (before?.anchor === hit ? before : { image, anchor: hit }) : null,
      );
    };
    const leave = () => setPointed(null);
    const events = ["keyup", "mouseup", "input", "focus", "blur", "scroll", "select"];
    for (const type of events) el.addEventListener(type, track);
    el.addEventListener("mousemove", point);
    el.addEventListener("mouseleave", leave);
    document.addEventListener("selectionchange", track);
    return () => {
      for (const type of events) el.removeEventListener(type, track);
      el.removeEventListener("mousemove", point);
      el.removeEventListener("mouseleave", leave);
      document.removeEventListener("selectionchange", track);
    };
  }, [field.known]);

  // A paste or a token removed changes the text without the events above.
  useLayoutEffect(() => {
    const el = input.current;
    if (el && marks.current) marks.current.scrollTop = el.scrollTop;
  });

  const besideToken = cursor === null ? undefined : tokenAt(text, cursor);
  const besideImage = besideToken && field.known.get(besideToken.number);
  const markOf = (number: number) =>
    marks.current?.querySelector(`mark[data-n="${number}"]`) ?? null;
  const cursorAnchor = besideImage ? markOf(besideImage.number) : null;
  // A thumbnail removed under the pointer never says the pointer left.
  const stillPointed = pointed?.anchor.isConnected ? pointed : null;
  const peek: Peek | null =
    stillPointed ??
    (besideImage && cursorAnchor ? { image: besideImage, anchor: cursorAnchor, fromCursor: true } : null);
  const active = peek?.image.number;

  const [dropping, setDropping] = useState(false);
  const isFiles = (event: DragEvent) => event.dataTransfer.types.includes("Files");

  return (
    <div
      className={`composer-images ${dropping ? "dropping" : ""}`}
      onDragOver={(event) => {
        if (!isFiles(event)) return;
        event.preventDefault();
        setDropping(true);
      }}
      onDragLeave={(event) => {
        if (!(event.relatedTarget instanceof Node && event.currentTarget.contains(event.relatedTarget))) {
          setDropping(false);
        }
      }}
      onDrop={(event) => {
        setDropping(false);
        const files = imageFiles(event.dataTransfer);
        if (files.length === 0) return;
        event.preventDefault();
        field.attach(files);
      }}
    >
      {showTray && field.images.length > 0 && (
        <div className="image-tray">
          {field.images.map((image, index) => (
            <ImageThumb
              key={image.number}
              machineId={machineId}
              image={image}
              active={image.number === active}
              onOpen={() => field.onOpen(field.images, index)}
              onRemove={() => field.remove(image.number)}
              onPeek={setPointed}
            />
          ))}
        </div>
      )}
      <div className="composer-field">
        <div className="composer-marks" ref={marks} aria-hidden="true">
          {splitTokens(text, field.known).map((piece, index) =>
            "image" in piece ? (
              <mark
                key={index}
                data-n={piece.image.number}
                className={piece.image.number === active ? "on" : undefined}
              >
                {piece.text}
              </mark>
            ) : (
              piece.text
            ),
          )}
          {/* A last empty line only takes up room with something on it. */}
          {text.endsWith("\n") ? " " : null}
        </div>
        {children}
      </div>
      {peek && <ImagePeek machineId={machineId} peek={peek} />}
    </div>
  );
}

/** A message you sent: its images above it, and its tokens marked in the text. */
export function PromptBubble({
  machineId,
  text,
  images,
  onOpen,
}: {
  machineId: string;
  text: string;
  images: PromptImage[];
  onOpen: OpenImages;
}) {
  const [peek, setPeek] = useState<Peek | null>(null);
  const showThumbs = useThumbnails("conversation.images.transcript");
  const known = useMemo(() => byNumber(images), [images]);
  const shown = imagesIn(text, known);
  // An image the text doesn't name still went, so it still shows.
  const all = [...shown, ...images.filter((image) => !shown.includes(image))];
  const active = peek?.image.number;

  return (
    <div className="you-group">
      {showThumbs && all.length > 0 && (
        <div className="image-tray sent">
          {all.map((image, index) => (
            <ImageThumb
              key={image.number}
              machineId={machineId}
              image={image}
              active={image.number === active}
              onOpen={() => onOpen(all, index)}
              onPeek={setPeek}
            />
          ))}
        </div>
      )}
      {text && (
        <div className="you">
          {splitTokens(text, known).map((piece, index) =>
            "image" in piece ? (
              <span
                key={index}
                className={`you-token ${piece.image.number === active ? "on" : ""}`}
                onMouseEnter={(event) => setPeek({ image: piece.image, anchor: event.currentTarget })}
                onMouseLeave={() => setPeek(null)}
                onClick={() => onOpen(all, all.indexOf(piece.image))}
              >
                {piece.text}
              </span>
            ) : (
              piece.text
            ),
          )}
        </div>
      )}
      {peek && <ImagePeek machineId={machineId} peek={peek} />}
    </div>
  );
}

/** One image at a time, full size. It has the keyboard until it closes, and gives it back. */
export function ImageViewer({
  machineId,
  images,
  start,
  onClose,
}: {
  machineId: string | undefined;
  images: PromptImage[];
  start: number;
  onClose: () => void;
}) {
  const [index, setIndex] = useState(start);
  const box = useRef<HTMLDivElement>(null);
  const image = images[Math.min(index, images.length - 1)];
  const url = useImageUrl(machineId, image, "full");

  useEffect(() => {
    const back = document.activeElement;
    box.current?.focus();
    // The palette gives the keyboard back as it closes, which can be after this opens.
    const frame = requestAnimationFrame(() => box.current?.focus());
    return () => {
      cancelAnimationFrame(frame);
      if (back instanceof HTMLElement && back.isConnected && back !== document.body) back.focus();
      else focusPane();
    };
  }, []);

  const step = (by: number) => setIndex((at) => (at + by + images.length) % images.length);
  // As big as it is, or as fits, whichever is smaller.
  const width = Math.min(
    image.width,
    window.innerWidth * 0.86,
    (window.innerHeight * 0.78 * image.width) / image.height,
  );
  return (
    <div
      className="viewer"
      ref={box}
      tabIndex={-1}
      role="dialog"
      aria-label={`Image #${image.number}`}
      onKeyDown={(event) => {
        if (act(event, "image.prev", () => step(-1))) return;
        if (act(event, "image.next", () => step(1))) return;
        if (act(event, "image.close", onClose)) return;
        act(event, "composer.open_image", onClose);
      }}
      onClick={onClose}
    >
      <div className="viewer-frame" style={{ width, height: (width * image.height) / image.width }}>
        {url && <img src={url} alt="" />}
      </div>
      <div className="image-caption viewer-caption">
        <b>Image #{image.number}</b>
        <span>
          {image.width} x {image.height}
        </span>
        {images.length > 1 && (
          <span>
            {index + 1} of {images.length}
          </span>
        )}
      </div>
    </div>
  );
}

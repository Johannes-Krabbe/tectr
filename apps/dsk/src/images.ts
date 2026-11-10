// Image bytes on this side: preparing a pasted image to send, giving it to a daemon, and
// fetching sent ones back to show. Events and drafts only carry an image's ID.

import { useEffect, useState } from "react";
import { type PromptImage, request } from "./engine";

/** Bigger pastes are scaled down to this on their long side. Models scale them down further
 * anyway, so this only saves time and memory. */
const MAX_EDGE = 2000;
/** The daemon takes 5 MB, but model APIs count the base64, which is a third bigger. */
const MAX_BYTES = 3_750_000;
const SENT_AS_IS = ["image/png", "image/jpeg", "image/gif", "image/webp"];

/** An image pasted here: its full bytes, which may still have to go to a daemon. */
interface Pasted {
  url: string;
  mime: string;
  /** Base64. */
  data: string;
}

const pasted = new Map<string, Pasted>();
/** Images fetched from a daemon, by `id/size`. */
const fetched = new Map<string, string>();
const loading = new Map<string, Promise<string>>();
/** `machine/id` for every image a daemon is known to have. */
const onDevice = new Set<string>();

/** Thumbnails and previews take the daemon's small copy, and only the full-size viewer
 * fetches the whole image. */
export type ImageSize = "thumbnail" | "full";

export type PreparedImage = Omit<PromptImage, "number">;

/** Scales and re-encodes an image only when it has to, and names it by its SHA-256, as the
 * daemon does. Rejects with a message written to be shown as is. */
export async function prepareImage(file: Blob): Promise<PreparedImage> {
  const bitmap = await createImageBitmap(file).catch(() => {
    throw new Error("That image couldn't be read");
  });
  const scale = Math.min(1, MAX_EDGE / Math.max(bitmap.width, bitmap.height));
  const width = Math.max(1, Math.round(bitmap.width * scale));
  const height = Math.max(1, Math.round(bitmap.height * scale));
  let blob: Blob = file;
  if (scale < 1 || !SENT_AS_IS.includes(file.type) || file.size > MAX_BYTES) {
    const canvas = new OffscreenCanvas(width, height);
    const context = canvas.getContext("2d");
    if (!context) throw new Error("That image couldn't be scaled down");
    context.drawImage(bitmap, 0, 0, width, height);
    blob = await canvas.convertToBlob({ type: "image/png" });
    if (blob.size > MAX_BYTES) {
      // JPEG has no transparency, which would otherwise come out black.
      context.globalCompositeOperation = "destination-over";
      context.fillStyle = "white";
      context.fillRect(0, 0, width, height);
      blob = await canvas.convertToBlob({ type: "image/jpeg", quality: 0.85 });
    }
  }
  bitmap.close();
  if (blob.size > MAX_BYTES) throw new Error("That image is too big to send, even scaled down");
  const bytes = new Uint8Array(await blob.arrayBuffer());
  const id = hex(new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)));
  const mime = blob.type;
  pasted.set(id, { url: URL.createObjectURL(blob), mime, data: toBase64(bytes) });
  return { id, mime, width, height };
}

/** Gives a daemon every image pasted here that it doesn't have yet. An image that wasn't
 * pasted here came from that daemon, so it has it already. `again` sends it even if it was
 * sent before, for images nothing on the daemon names yet, which it removes after a day. */
export async function sendImages(
  machineId: string,
  images: readonly PromptImage[],
  { again = false }: { again?: boolean } = {},
) {
  await Promise.all(
    images.map(async ({ id }) => {
      const kept = pasted.get(id);
      if (!kept || (!again && onDevice.has(`${machineId}/${id}`))) return;
      const added = await request(machineId, { command: "add_image", mime: kept.mime, data: kept.data });
      if (added.output !== "image_added" || added.id !== id) {
        throw new Error("The daemon stored a different image than the one pasted");
      }
      onDevice.add(`${machineId}/${id}`);
    }),
  );
}

/** A URL to show the image with, once its bytes are here. */
export function useImageUrl(
  machineId: string | undefined,
  image: PromptImage,
  size: ImageSize,
): string | undefined {
  const [url, setUrl] = useState(() => ready(image, size));
  useEffect(() => {
    const now = ready(image, size);
    if (now || machineId === undefined) {
      setUrl(now);
      return;
    }
    let live = true;
    load(machineId, image, size).then(
      (loaded) => live && setUrl(loaded),
      // It stays a blank frame of the right shape, which is all there is to show.
      () => {},
    );
    return () => {
      live = false;
    };
  }, [machineId, image.id, size]);
  return url;
}

/** What's here already. The whole image does for a thumbnail too. */
function ready(image: PromptImage, size: ImageSize): string | undefined {
  return (
    pasted.get(image.id)?.url ??
    fetched.get(`${image.id}/full`) ??
    (size === "thumbnail" ? fetched.get(`${image.id}/thumbnail`) : undefined)
  );
}

function load(machineId: string, image: PromptImage, size: ImageSize): Promise<string> {
  const key = `${image.id}/${size}`;
  let pending = loading.get(key);
  if (!pending) {
    const thumbnail = size === "thumbnail";
    pending = request(machineId, { command: "read_image", id: image.id, thumbnail }).then((read) => {
      if (read.output !== "image") throw new Error("The daemon didn't send the image");
      const url = URL.createObjectURL(new Blob([fromBase64(read.data)], { type: read.mime }));
      fetched.set(key, url);
      onDevice.add(`${machineId}/${image.id}`);
      return url;
    });
    pending.catch(() => loading.delete(key));
    loading.set(key, pending);
  }
  return pending;
}

function hex(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

function toBase64(bytes: Uint8Array): string {
  let binary = "";
  // Spreading a whole image into one call would overflow the stack.
  for (let at = 0; at < bytes.length; at += 0x8000) {
    binary += String.fromCharCode(...bytes.subarray(at, at + 0x8000));
  }
  return btoa(binary);
}

function fromBase64(data: string): Uint8Array<ArrayBuffer> {
  const binary = atob(data);
  const bytes = new Uint8Array(binary.length);
  for (let at = 0; at < binary.length; at++) bytes[at] = binary.charCodeAt(at);
  return bytes;
}

/** The image files in a paste or a drop. */
export function imageFiles(data: DataTransfer): File[] {
  return [...data.items]
    .filter((item) => item.kind === "file" && item.type.startsWith("image/"))
    .flatMap((item) => item.getAsFile() ?? []);
}

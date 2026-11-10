// What the bridge sends a terminal pane, and how much of it to acknowledge. Kept apart from
// the pane so it can be tested without a webview.

/** What the terminal printed, to write after what came before. */
export const OUTPUT = 1;
/** Start over: the size, then what recreates the whole screen. */
export const RESET = 2;

export interface FrameRecord {
  kind: number;
  /** Acknowledged in full once drawn, a reset's size included. */
  payload: Uint8Array;
}

/** The bridge batches frames into one message: a kind byte, a little-endian `u32` length, then
 * the payload, for each. Anything cut short is dropped. */
export function readRecords(buffer: ArrayBuffer): FrameRecord[] {
  const bytes = new Uint8Array(buffer);
  const view = new DataView(buffer);
  const records: FrameRecord[] = [];
  let at = 0;
  while (at + 5 <= bytes.length) {
    const kind = bytes[at];
    const length = view.getUint32(at + 1, true);
    const start = at + 5;
    if (start + length > bytes.length) break;
    records.push({ kind, payload: bytes.subarray(start, start + length) });
    at = start + length;
  }
  return records;
}

export function readReset(payload: Uint8Array): { cols: number; rows: number; screen: Uint8Array } {
  const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
  return {
    cols: view.getUint16(0, true),
    rows: view.getUint16(2, true),
    screen: payload.subarray(4),
  };
}

/** `#rrggbb` from a CSS color token, which may carry alpha the daemon has no use for. */
export function opaque(hex: string): string {
  const trimmed = hex.trim();
  return /^#[0-9a-fA-F]{8}$/.test(trimmed) ? trimmed.slice(0, 7) : trimmed;
}

/** Bytes for a string xterm.js reports as binary, one character per byte. */
export function binaryBytes(data: string): Uint8Array {
  return Uint8Array.from(data, (char) => char.charCodeAt(0) & 0xff);
}

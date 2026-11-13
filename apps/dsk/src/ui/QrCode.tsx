import { useMemo } from "react";
import { encode } from "uqr";
import { qrPath } from "../pairing";

export function QrCode({ text, label }: { text: string; label: string }) {
  const { size, path } = useMemo(() => {
    // A quiet zone of four modules is what the QR spec asks for, and scanners find the
    // code faster with it.
    const qr = encode(text, { ecc: "M", border: 4 });
    return { size: qr.size, path: qrPath(qr.data) };
  }, [text]);
  return (
    <svg
      className="qr"
      viewBox={`0 0 ${size} ${size}`}
      shapeRendering="crispEdges"
      role="img"
      aria-label={label}
    >
      <path d={path} />
    </svg>
  );
}

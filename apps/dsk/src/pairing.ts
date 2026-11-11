/** One path of horizontal runs, so a code is a single node however many modules it has. */
export function qrPath(data: boolean[][]): string {
  let path = "";
  data.forEach((row, y) => {
    let x = 0;
    while (x < row.length) {
      if (!row[x]) {
        x++;
        continue;
      }
      const start = x;
      while (x < row.length && row[x]) x++;
      path += `M${start} ${y}h${x - start}v1h${start - x}z`;
    }
  });
  return path;
}

/** Time left as `m:ss`, rounded up, so it shows 0:00 only once it's over. */
export function countdown(ms: number): string {
  const seconds = Math.max(Math.ceil(ms / 1000), 0);
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

/** `482913` as `482 913`, which is easier to read off one screen and type on another. */
export function spacedDigits(digits: string): string {
  const middle = Math.floor(digits.length / 2);
  return `${digits.slice(0, middle)} ${digits.slice(middle)}`;
}

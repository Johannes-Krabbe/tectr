#!/usr/bin/env node
// Builds every logo file from the mark geometry below and the color tokens, then copies the app
// icons into both apps.
//   node design/logo/build.mjs           write SVGs, render PNGs and icons, copy them into the apps
//   node design/logo/build.mjs --check   fail if the SVGs or the Liquid Glass icons are out of date
//
// Rendering uses the Tauri CLI from apps/dsk and macOS sips, so it needs a Mac with pnpm install
// done. --check skips rendered files, because their bytes depend on the renderer's version.

import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { execFileSync } from 'node:child_process';
import { crc32, deflateSync, inflateSync } from 'node:zlib';
import { loadTokens } from '../tokens/build.mjs';

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = join(HERE, '..', '..');
const LOGO = 'design/logo';
const TAURI_ICONS = 'apps/dsk/src-tauri/icons';
// The iOS file name is the icon's name in the build settings. Tauri renames its copy itself.
const GLASS_ICONS = [`${LOGO}/AppIcon.icon`, `${TAURI_ICONS}/icon.icon`, 'apps/ios/Tectr/AppIcon.icon'];
// Debug builds of the iOS app use this one, so a build from Xcode and a TestFlight build can sit
// side by side on a phone and still be told apart.
const DEV_GLASS_ICONS = [`${LOGO}/AppIconDev.icon`, 'apps/ios/Tectr/AppIconDev.icon'];

// The Joint: a T-shaped tenon set into a block, on a 48 unit grid with 12 unit walls and 4 unit
// gaps. Every coordinate is a multiple of 4, so at 12px (a 16px favicon with 2px padding) each
// gap is exactly one pixel.
export const MARK = [
  [[0, 0], [48, 0], [48, 12], [32, 12], [32, 32], [16, 32], [16, 12], [0, 12]],
  [[0, 16], [12, 16], [12, 36], [36, 36], [36, 16], [48, 16], [48, 48], [0, 48]],
];

// "tectr" in Inter SemiBold (the app's UI font) at -0.02em, kerned with HarfBuzz and outlined
// with fontTools, so the files never depend on Inter being installed. 1em is 1000 units, the
// ink starts at x = 0 and the baseline is y = 0.
const WORDMARK = {
  width: 2157.7,
  ascent: 675.8,
  descent: 11,
  d: 'M310 -545.9V-446.4H0V-545.9ZM77.5 -675.8H204.4V-153.7Q204.4 -124.1 217.1 -110.3Q229.8 -96.4 259 -96.4Q267.5 -96.4 281.8 -98.5Q296.2 -100.5 305 -102.5L325 -4.5Q304.1 2 282.3 4.7Q260.4 7.4 240 7.4Q161.7 7.4 119.6 -31.7Q77.5 -70.7 77.5 -142.9ZM621.9 11Q539.8 11 480.3 -23.5Q420.8 -58.1 388.9 -121.1Q356.9 -184 356.9 -269.6Q356.9 -353.8 388.5 -417.5Q420.1 -481.2 477.9 -517Q535.7 -552.7 613.6 -552.7Q663.7 -552.7 708.8 -536.6Q753.8 -520.4 788.7 -486.8Q823.5 -453.3 843.5 -401.2Q863.5 -349.1 863.5 -277.1V-237.5H416.8V-323.7H800.1L740.8 -298.1Q740.8 -344.5 726.5 -379.3Q712.2 -414.2 684.1 -433.7Q656 -453.2 614.3 -453.2Q572.5 -453.2 543.2 -433.4Q513.8 -413.6 498.4 -380.6Q483 -347.6 483 -307.2V-247.5Q483 -195.7 500.6 -160.2Q518.2 -124.6 549.9 -106.6Q581.7 -88.6 623.6 -88.6Q651.4 -88.6 674.2 -96.5Q697 -104.5 713.4 -120.3Q729.8 -136.1 738.5 -159.4L855.1 -135.2Q842 -91.4 810.2 -58.5Q778.3 -25.5 730.6 -7.3Q683 11 621.9 11ZM1188.9 11Q1109.4 11 1050.8 -24.2Q992.3 -59.5 960.2 -122.9Q928.2 -186.2 928.2 -270.2Q928.2 -355.2 960.2 -418.7Q992.3 -482.2 1050.8 -517.5Q1109.4 -552.7 1188.9 -552.7Q1234.6 -552.7 1273.6 -540.9Q1312.6 -529.1 1343.4 -507Q1374.2 -484.9 1394.9 -453.1Q1415.5 -421.3 1424.5 -381.5L1305.5 -356.5Q1300.6 -377.7 1290.4 -394.8Q1280.2 -411.9 1265.6 -424.3Q1251 -436.6 1231.9 -443.1Q1212.9 -449.7 1189.9 -449.7Q1145.6 -449.7 1116.2 -426.1Q1086.9 -402.6 1072.2 -362.2Q1057.5 -321.8 1057.5 -270.6Q1057.5 -220.2 1072.2 -179.8Q1086.9 -139.4 1116.2 -115.7Q1145.6 -92 1189.9 -92Q1213.2 -92 1232.6 -98.8Q1251.9 -105.6 1266.9 -118.3Q1281.9 -131.1 1292.1 -149.1Q1302.3 -167.1 1307.1 -189.1L1425.7 -164.3Q1417.1 -123.3 1396.3 -91Q1375.5 -58.8 1344.6 -35.9Q1313.7 -13 1274.3 -1Q1234.9 11 1188.9 11ZM1767.1 -545.9V-446.4H1457.1V-545.9ZM1534.6 -675.8H1661.5V-153.7Q1661.5 -124.1 1674.2 -110.3Q1686.9 -96.4 1716.1 -96.4Q1724.5 -96.4 1738.9 -98.5Q1753.2 -100.5 1762 -102.5L1782 -4.5Q1761.2 2 1739.4 4.7Q1717.5 7.4 1697.1 7.4Q1618.7 7.4 1576.7 -31.7Q1534.6 -70.7 1534.6 -142.9ZM1847.7 0V-545.9H1970.6V-454.5H1976.4Q1991.4 -502 2027.9 -527.7Q2064.5 -553.4 2111.4 -553.4Q2122.2 -553.4 2135.4 -552.3Q2148.5 -551.3 2157.7 -549.6V-432.9Q2150 -435.5 2132.3 -437.5Q2114.7 -439.6 2098 -439.6Q2062.6 -439.6 2034.5 -424.4Q2006.4 -409.3 1990.5 -382.7Q1974.6 -356.1 1974.6 -321.1V0Z',
};

const dark = loadTokens('color.dark');
const light = loadTokens('color.light');
const COLOR = {
  onDark: dark['color.text.primary'].value,
  onLight: light['color.text.primary'].value,
  base: dark['color.surface.base'].value,
  raised: dark['color.surface.raised'].value,
  edge: dark['color.line.strong'].value,
};

// The dev tile is the online green with its hue and saturation kept and its lightness lowered,
// so the light mark keeps at least 3:1 contrast on it. The top is a step lighter, like the dark tile.
const DEV = {
  raised: withLightness(dark['color.status.online'].value, 0.34),
  base: withLightness(dark['color.status.online'].value, 0.24),
};

function withLightness(hex, lightness) {
  const [r, g, b] = [1, 3, 5].map(i => parseInt(hex.slice(i, i + 2), 16) / 255);
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const l = (max + min) / 2;
  const s = max === min ? 0 : (max - min) / (1 - Math.abs(2 * l - 1));
  const h = max === min ? 0 : max === r ? ((g - b) / (max - min) + 6) % 6 : max === g ? (b - r) / (max - min) + 2 : (r - g) / (max - min) + 4;
  const c = (1 - Math.abs(2 * lightness - 1)) * s;
  const x = c * (1 - Math.abs((h % 2) - 1));
  const m = lightness - c / 2;
  const rgb = [[c, x, 0], [x, c, 0], [0, c, x], [0, x, c], [x, 0, c], [c, 0, x]][Math.floor(h)];
  return `#${rgb.map(v => Math.round((v + m) * 255).toString(16).padStart(2, '0')).join('')}`;
}

const num = (value, places = 2) => String(Number(value.toFixed(places)));

function markPath(x, y, size, pieces = MARK) {
  const k = size / 48;
  return pieces.map(points => `M${points.map(([px, py]) => `${num(x + px * k)} ${num(y + py * k)}`).join('L')}Z`).join('');
}

const svg = (width, height, body, attrs = '') =>
  `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${num(width)} ${num(height)}" width="${num(width)}" height="${num(height)}"${attrs}>${body}</svg>\n`;

// The mark is as tall as the t, sitting on the baseline, so the name and the block share a top
// and a bottom edge. Only the round letters' overshoot hangs below.
const GAP = 20;
const TEXT_SCALE = 48 / WORDMARK.ascent;
const LOCKUP = { width: 48 + GAP + WORDMARK.width * TEXT_SCALE, height: 48 + WORDMARK.descent * TEXT_SCALE };
const lockupBody = `<path d="${markPath(0, 0, 48)}"/><path transform="translate(${48 + GAP} 48) scale(${num(TEXT_SCALE, 6)})" d="${WORDMARK.d}"/>`;

const gradient = ({ raised, base }) => `<defs><linearGradient id="bg" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="${raised}"/><stop offset="1" stop-color="${base}"/></linearGradient></defs>`;
const background = gradient(COLOR);

// macOS draws icons as they are, so this follows Apple's 1024 grid: an 824 tile with a 100
// margin for the shadow. The edge keeps the dark tile visible on a dark Dock.
const macIcon = svg(1024, 1024, [
  background.replace('</defs>', '<filter id="shadow" x="-10%" y="-10%" width="120%" height="130%"><feDropShadow dx="0" dy="10" stdDeviation="12" flood-color="black" flood-opacity="0.3"/></filter></defs>'),
  `<rect x="100" y="100" width="824" height="824" rx="185" fill="url(#bg)" filter="url(#shadow)"/>`,
  `<rect x="102" y="102" width="820" height="820" rx="183" fill="none" stroke="${COLOR.edge}" stroke-width="4"/>`,
  `<path fill="${COLOR.onDark}" d="${markPath(296, 296, 432)}"/>`,
].join(''));

// The flat iOS icon, for web icons. iOS masks the corners itself and wants a full, opaque square.
const iosIcon = tile => svg(1024, 1024, `${gradient(tile)}<rect width="1024" height="1024" fill="url(#bg)"/><path fill="${COLOR.onDark}" d="${markPath(248, 248, 528)}"/>`);

// Liquid Glass icons for macOS 26 and iOS 26, read by Icon Composer, Xcode and Tauri. Each piece of
// the joint is its own glass layer, so the system lights the tenon and the block separately.
// Layers cover the whole 1024 canvas, as Icon Composer expects, which keeps the pieces in register.
const srgb = hex => `extended-srgb:${[1, 3, 5].map(i => (parseInt(hex.slice(i, i + 2), 16) / 255).toFixed(5)).join(',')},1.00000`;
const GLASS_LAYERS = { tenon: [MARK[0]], block: [MARK[1]] };
const glassIcon = tile => ({
  fill: { 'automatic-gradient': srgb(tile.raised) },
  groups: [{
    layers: Object.keys(GLASS_LAYERS).map(name => ({ glass: true, 'image-name': `${name}.svg`, name })),
    shadow: { kind: 'neutral', opacity: 0.5 },
    translucency: { enabled: true, value: 0.5 },
  }],
  'supported-platforms': { squares: 'shared' },
});

const SOCIAL = { width: 1280, height: 640, lockup: 560 };
const socialScale = SOCIAL.lockup / LOCKUP.width;
const social = svg(SOCIAL.width, SOCIAL.height, [
  `<rect width="${SOCIAL.width}" height="${SOCIAL.height}" fill="${COLOR.base}"/>`,
  `<g fill="${COLOR.onDark}" transform="translate(${num((SOCIAL.width - SOCIAL.lockup) / 2)} ${num((SOCIAL.height - 48 * socialScale) / 2)}) scale(${num(socialScale, 6)})">${lockupBody}</g>`,
].join(''));

// Paths are relative to the repo root.
export function outputs() {
  const mark = (attrs = '') => svg(48, 48, `<path d="${markPath(0, 0, 48)}"/>`, attrs);
  const lockup = (attrs = '') => svg(LOCKUP.width, LOCKUP.height, lockupBody, attrs);
  return {
    [`${LOGO}/mark.svg`]: mark(' fill="currentColor"'),
    [`${LOGO}/mark-on-dark.svg`]: mark(` fill="${COLOR.onDark}"`),
    [`${LOGO}/mark-on-light.svg`]: mark(` fill="${COLOR.onLight}"`),
    [`${LOGO}/lockup.svg`]: lockup(' fill="currentColor"'),
    [`${LOGO}/lockup-on-dark.svg`]: lockup(` fill="${COLOR.onDark}"`),
    [`${LOGO}/lockup-on-light.svg`]: lockup(` fill="${COLOR.onLight}"`),
    [`${LOGO}/app-icon-macos.svg`]: macIcon,
    [`${LOGO}/app-icon-ios.svg`]: iosIcon(COLOR),
    [`${LOGO}/app-icon-ios-dev.svg`]: iosIcon(DEV),
    [`${LOGO}/tile.svg`]: svg(64, 64, `<rect width="64" height="64" rx="14" fill="${COLOR.base}"/><path fill="${COLOR.onDark}" d="${markPath(8, 8, 48)}"/>`),
    [`${LOGO}/social.svg`]: social,
    [`${LOGO}/favicon/favicon.svg`]: svg(64, 64, `<style>path{fill:${COLOR.onLight}}@media (prefers-color-scheme:dark){path{fill:${COLOR.onDark}}}</style><path d="${markPath(8, 8, 48)}"/>`),
    ...Object.fromEntries([...GLASS_ICONS.map(dir => [dir, COLOR]), ...DEV_GLASS_ICONS.map(dir => [dir, DEV])].flatMap(([dir, tile]) => [
      [`${dir}/icon.json`, `${JSON.stringify(glassIcon(tile), null, 2)}\n`],
      ...Object.entries(GLASS_LAYERS).map(([name, pieces]) => [
        `${dir}/Assets/${name}.svg`,
        svg(1024, 1024, `<path fill="${COLOR.onDark}" d="${markPath(248, 248, 528, pieces)}"/>`),
      ]),
    ])),
  };
}

export function staleFiles() {
  return Object.entries(outputs())
    .filter(([file, content]) => !existsSync(join(ROOT, file)) || readFileSync(join(ROOT, file), 'utf8') !== content)
    .map(([file]) => file);
}

// [source, output with {s} for the size, sizes]. Wide sources render into a square and are
// cropped back, because the Tauri CLI only writes square PNGs.
const RASTERS = [
  ['app-icon-macos.svg', 'png/app-icon-macos-{s}.png', [16, 32, 64, 128, 256, 512, 1024]],
  ['app-icon-ios.svg', 'png/app-icon-ios-{s}.png', [1024], { opaque: true }],
  ['app-icon-ios-dev.svg', 'png/app-icon-ios-dev-{s}.png', [1024], { opaque: true }],
  ['mark-on-dark.svg', 'png/mark-on-dark-{s}.png', [64, 128, 256, 512, 1024]],
  ['mark-on-light.svg', 'png/mark-on-light-{s}.png', [64, 128, 256, 512, 1024]],
  ['lockup-on-dark.svg', 'png/lockup-on-dark-{s}w.png', [480, 960, 1920], { aspect: LOCKUP.height / LOCKUP.width }],
  ['lockup-on-light.svg', 'png/lockup-on-light-{s}w.png', [480, 960, 1920], { aspect: LOCKUP.height / LOCKUP.width }],
  ['social.svg', 'png/social-1280x640.png', [1280], { aspect: SOCIAL.height / SOCIAL.width }],
  ['tile.svg', 'favicon/favicon-{s}.png', [16, 32, 48]],
  ['app-icon-ios.svg', 'favicon/apple-touch-icon.png', [180], { opaque: true }],
  ['app-icon-ios.svg', 'favicon/icon-{s}.png', [192, 512]],
];

function tauriIcon(source, args) {
  const out = mkdtempSync(join(tmpdir(), 'tectr-logo-'));
  execFileSync('pnpm', ['--dir', 'apps/dsk', 'exec', 'tauri', 'icon', join(ROOT, LOGO, source), '-o', out, ...args], { cwd: ROOT, stdio: 'ignore' });
  return out;
}

function render() {
  for (const [source, pattern, sizes, { opaque = false, aspect } = {}] of RASTERS) {
    const out = tauriIcon(source, ['-p', sizes.join(','), ...(aspect ? ['--fit', 'contain'] : [])]);
    for (const size of sizes) {
      const target = join(ROOT, LOGO, pattern.replace('{s}', size));
      mkdirSync(dirname(target), { recursive: true });
      const rendered = join(out, `${size}x${size}.png`);
      if (aspect) {
        execFileSync('sips', ['-c', String(Math.round(size * aspect)), String(size), rendered, '--out', target], { stdio: 'ignore' });
      } else {
        copyFileSync(rendered, target);
      }
      if (opaque) stripAlpha(target);
    }
    rmSync(out, { recursive: true });
  }

  const mac = tauriIcon('app-icon-macos.svg', []);
  copyFileSync(join(mac, 'icon.icns'), join(ROOT, LOGO, 'app-icon.icns'));
  for (const file of ['32x32.png', '128x128.png', '128x128@2x.png', 'icon.png', 'icon.icns', 'icon.ico']) {
    copyFileSync(join(mac, file), join(ROOT, TAURI_ICONS, file));
  }
  rmSync(mac, { recursive: true });

  const favicon = tauriIcon('tile.svg', []);
  copyFileSync(join(favicon, 'icon.ico'), join(ROOT, LOGO, 'favicon/favicon.ico'));
  rmSync(favicon, { recursive: true });
}

// The App Store rejects an app icon with an alpha channel, even a fully opaque one, and the
// Tauri CLI always writes RGBA. This rewrites an 8-bit RGBA PNG as RGB.
function stripAlpha(file) {
  const png = readFileSync(file);
  let width = 0;
  let height = 0;
  const idat = [];
  for (let pos = 8; pos < png.length;) {
    const length = png.readUInt32BE(pos);
    const type = png.toString('ascii', pos + 4, pos + 8);
    const data = png.subarray(pos + 8, pos + 8 + length);
    if (type === 'IHDR') {
      width = data.readUInt32BE(0);
      height = data.readUInt32BE(4);
      if (data[8] !== 8 || data[9] !== 6 || data[12] !== 0) throw new Error(`${file}: expected 8-bit RGBA without interlacing`);
    }
    if (type === 'IDAT') idat.push(data);
    pos += 12 + length;
  }
  const raw = inflateSync(Buffer.concat(idat));
  const stride = width * 4;
  const rgb = Buffer.alloc((width * 3 + 1) * height);
  let previous = Buffer.alloc(stride);
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)];
    const line = Buffer.from(raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1)));
    for (let i = 0; i < stride; i++) {
      const a = i >= 4 ? line[i - 4] : 0;
      const b = previous[i];
      const c = i >= 4 ? previous[i - 4] : 0;
      const predictor = [0, a, b, (a + b) >> 1, paeth(a, b, c)][filter];
      line[i] = (line[i] + predictor) & 255;
    }
    for (let x = 0; x < width; x++) {
      if (line[x * 4 + 3] !== 255) throw new Error(`${file}: not opaque at ${x},${y}`);
      line.copy(rgb, y * (width * 3 + 1) + 1 + x * 3, x * 4, x * 4 + 3);
    }
    previous = line;
  }
  const chunk = (type, data) => {
    const body = Buffer.concat([Buffer.from(type, 'ascii'), data]);
    const out = Buffer.alloc(12 + data.length);
    out.writeUInt32BE(data.length, 0);
    body.copy(out, 4);
    out.writeUInt32BE(crc32(body), 8 + data.length);
    return out;
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header[8] = 8;
  header[9] = 2;
  writeFileSync(file, Buffer.concat([png.subarray(0, 8), chunk('IHDR', header), chunk('IDAT', deflateSync(rgb, { level: 9 })), chunk('IEND', Buffer.alloc(0))]));
}

function paeth(a, b, c) {
  const p = a + b - c;
  const pa = Math.abs(p - a);
  const pb = Math.abs(p - b);
  const pc = Math.abs(p - c);
  return pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  if (process.argv.includes('--check')) {
    const stale = staleFiles();
    if (stale.length) {
      console.error(`Out of date: ${stale.join(', ')}. Run node design/logo/build.mjs`);
      process.exit(1);
    }
    console.log('Logo files are up to date.');
  } else {
    const files = Object.entries(outputs());
    for (const [file, content] of files) {
      mkdirSync(dirname(join(ROOT, file)), { recursive: true });
      writeFileSync(join(ROOT, file), content);
    }
    render();
    console.log(`Wrote ${files.length} files and rendered the PNGs and icons.`);
  }
}

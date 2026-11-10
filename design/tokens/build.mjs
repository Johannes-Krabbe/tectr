#!/usr/bin/env node
// Builds generated files from the token JSON, fixtures, and icons. No dependencies.
//   node design/tokens/build.mjs           write generated files
//   node design/tokens/build.mjs --check   fail if they are out of date
//
// Token files follow the DTCG shape ($value, $type, {alias}). Dimensions are plain
// numbers, in px on desktop and pt on iOS, and durations are milliseconds.

import { mkdirSync, readFileSync, readdirSync, writeFileSync, existsSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const DESIGN = join(HERE, '..');
const ROOT = join(DESIGN, '..');
const IOS_PACKAGE = 'apps/ios/TectrDesign/Sources/TectrDesign';

const readJson = path => JSON.parse(readFileSync(path, 'utf8'));

function flatten(node, path = [], inheritedType, out = {}) {
  const type = node.$type ?? inheritedType;
  if ('$value' in node) {
    out[path.join('.')] = { type, value: node.$value, extensions: node.$extensions };
    return out;
  }
  for (const [key, child] of Object.entries(node)) {
    if (!key.startsWith('$')) flatten(child, [...path, key], type, out);
  }
  return out;
}

function resolveValue(value, tokens, seen = []) {
  if (typeof value === 'string') {
    const alias = value.match(/^\{(.+)\}$/);
    if (!alias) return value;
    const name = alias[1];
    if (!tokens[name]) throw new Error(`Unknown alias {${name}}`);
    if (seen.includes(name)) throw new Error(`Alias cycle: ${[...seen, name].join(' -> ')}`);
    return resolveValue(tokens[name].value, tokens, [...seen, name]);
  }
  if (value && typeof value === 'object') {
    return Object.fromEntries(Object.entries(value).map(([k, v]) => [k, resolveValue(v, tokens, seen)]));
  }
  return value;
}

export function loadTokens(file) {
  const tokens = flatten(readJson(join(HERE, `${file}.tokens.json`)));
  for (const token of Object.values(tokens)) token.value = resolveValue(token.value, tokens);
  return tokens;
}

const cssName = name => `--${name.replaceAll('.', '-')}`;

function cssDeclarations(tokens) {
  const lines = [];
  for (const [name, { type, value }] of Object.entries(tokens)) {
    if (type === 'shadow') {
      // Only the color differs between themes.
      lines.push(`--color-${name.replace('.', '-')}: ${value.color};`);
      lines.push(`${cssName(name)}: ${value.offsetX}px ${value.offsetY}px ${value.blur}px ${value.spread}px var(--color-${name.replace('.', '-')});`);
    } else if (type === 'dimension') {
      lines.push(`${cssName(name)}: ${value}px;`);
    } else if (type === 'duration') {
      lines.push(`${cssName(name)}: ${value}ms;`);
    } else {
      lines.push(`${cssName(name)}: ${value};`);
    }
  }
  return lines;
}

const block = (selector, lines) => `${selector} {\n${lines.map(line => `  ${line}`).join('\n')}\n}\n`;

// The scale is written in terms of what the desktop app's appearance settings can change, so
// those are the only variables it has to set. --unit is 1px at the default text size, and
// --unit-code is the same for code. Rows, controls and icons grow with the text around them.
function scaleDeclarations(tokens) {
  const ui = tokens['text.body'].value;
  const code = tokens['text.code-body'].value;
  const reading = tokens['text.reading'].value;
  const lines = [
    `--font-ui-stack: ${tokens['font.ui'].value};`,
    `--font-code-stack: ${tokens['font.code'].value};`,
    '--font-ui: var(--font-ui-stack);',
    '--font-code: var(--font-code-stack);',
    `--font-ui-size: ${ui.fontSize}px;`,
    `--font-code-size: ${code.fontSize}px;`,
    `--font-reading-line-height: ${reading.lineHeight};`,
    `--font-code-line-height: ${code.lineHeight};`,
    `--unit: calc(var(--font-ui-size) / ${ui.fontSize});`,
    `--unit-code: calc(var(--font-code-size) / ${code.fontSize});`,
    '--density: 1;',
    '--corners: 1;',
  ];
  for (const [name, { type, value }] of Object.entries(tokens)) {
    const [group, key] = name.split('.');
    if (type === 'typography') {
      const isCode = value.fontFamily === tokens['font.code'].value;
      const size = `calc(${value.fontSize} * var(${isCode ? '--unit-code' : '--unit'}))`;
      const lineHeight = isCode ? 'var(--font-code-line-height)' : key === 'reading' ? 'var(--font-reading-line-height)' : value.lineHeight;
      lines.push(`${cssName(name)}: ${value.fontWeight} ${size}/${lineHeight} var(${isCode ? '--font-code' : '--font-ui'});`);
    } else if (group === 'space') {
      lines.push(`${cssName(name)}: calc(${value}px * var(--density));`);
    } else if (group === 'radius') {
      lines.push(`${cssName(name)}: calc(${value}px * var(--corners));`);
    } else if (name === 'size.icon') {
      lines.push(`${cssName(name)}: calc(${value} * var(--unit));`);
    } else if (group === 'size') {
      lines.push(`${cssName(name)}: calc(${value} * var(--unit) * var(--density));`);
    } else if (group !== 'font') {
      lines.push(...cssDeclarations({ [name]: { type, value } }));
    }
  }
  return lines;
}

function buildCss() {
  const dark = cssDeclarations(loadTokens('color.dark'));
  const light = cssDeclarations(loadTokens('color.light'));
  return [
    '/* Generated by design/tokens/build.mjs. Edit the token files, not this. */\n',
    block(':root', scaleDeclarations(loadTokens('scale.desktop'))),
    block(':root, [data-theme="dark"]', ['color-scheme: dark;', ...dark]),
    block('[data-theme="light"]', ['color-scheme: light;', ...light]),
    `@media (prefers-color-scheme: light) {\n${block(':root:not([data-theme])', ['color-scheme: light;', ...light])}}\n`,
  ].join('\n');
}

// What crates/settings checks its themes and default sizes against, since they have to be
// written there too: the app's themes are data it reads, in one format with everyone else's.
function buildDesktopDefaults() {
  const colors = file => Object.fromEntries(Object.entries(loadTokens(file)).map(([name, { type, value }]) =>
    [name.replace(/^color\./, ''), type === 'shadow' ? value.color : value]));
  const scale = loadTokens('scale.desktop');
  return `${JSON.stringify({
    dark: colors('color.dark'),
    light: colors('color.light'),
    scale: {
      'text.size': scale['text.body'].value.fontSize,
      'text.line_height': scale['text.reading'].value.lineHeight,
      'code.size': scale['text.code-body'].value.fontSize,
      'code.line_height': scale['text.code-body'].value.lineHeight,
    },
  }, null, 2)}\n`;
}

export function loadIcons() {
  const dir = join(DESIGN, 'icons');
  return Object.fromEntries(readdirSync(dir).filter(f => f.endsWith('.svg')).sort()
    .map(f => [f.replace(/\.svg$/, ''), readFileSync(join(dir, f), 'utf8').trim()]));
}

export const loadBoard = () => readJson(join(DESIGN, 'fixtures', 'board.json'));

function buildPreviewData() {
  const values = file => Object.fromEntries(Object.entries(loadTokens(file))
    .map(([name, { type, value, extensions }]) => [name, { type, value, ...(extensions && { extensions }) }]));
  const data = {
    tokens: {
      dark: values('color.dark'),
      light: values('color.light'),
      desktop: values('scale.desktop'),
      ios: values('scale.ios'),
    },
    icons: loadIcons(),
    board: loadBoard(),
  };
  // A script rather than JSON, so the preview also opens from file://, where fetch is blocked.
  return `// Generated by design/tokens/build.mjs. Edit the sources, not this.\nwindow.TECTR = ${JSON.stringify(data, null, 2)};\n`;
}

const SWIFT_HEADER = '// Generated by design/tokens/build.mjs. Edit the token files, not this.\n';

const camel = parts => parts.flatMap(part => part.split('-'))
  .map((word, i) => (i ? word[0].toUpperCase() + word.slice(1) : word)).join('');

// Swift identifiers can't start with a digit, so 2xs becomes xxs.
const swiftKey = key => camel([key.replace(/^2/, 'x')]);

function swiftRgba(hex) {
  return `0x${hex.slice(1)}${hex.length === 7 ? 'ff' : ''}`;
}

function swiftColors() {
  const dark = loadTokens('color.dark');
  const light = loadTokens('color.light');
  // Shadows aren't generated, because iOS overlays are system sheets with system shadows.
  const names = Object.keys(dark).filter(name => dark[name].type === 'color');
  const swift = name => camel(name.split('.').slice(1));
  const palette = tokens => names.map(n => `        ${swift(n)}: Color(rgba: ${swiftRgba(tokens[n].value)})`).join(',\n');
  return [
    '/// Every color a theme has. Views read it through `ThemeColor`, so a theme can change at runtime.',
    'public struct Palette: Equatable, Sendable {',
    ...names.map(n => `    public var ${swift(n)}: Color`),
    '}',
    '',
    'public extension Palette {',
    '    /// Colors by token name without `color.`, as the settings crate resolves a theme. Nil when one is missing or unreadable.',
    '    init?(colors: [String: String]) {',
    ...names.map(n => `        guard let ${swift(n)} = Color(hex: colors[${JSON.stringify(n.split('.').slice(1).join('.'))}]) else { return nil }`),
    `        self.init(${names.map(n => `${swift(n)}: ${swift(n)}`).join(', ')})`,
    '    }',
    '',
    `    static let tectrDark = Palette(\n${palette(dark)}\n    )`,
    '',
    `    static let tectrLight = Palette(\n${palette(light)}\n    )`,
    '',
    '    subscript(role: ColorRole) -> Color {',
    '        switch role {',
    ...names.map(n => `        case .${swift(n)}: ${swift(n)}`),
    '        }',
    '    }',
    '}',
    '',
    'public enum ColorRole: CaseIterable, Sendable {',
    ...names.map(n => `    case ${swift(n)}`),
    '}',
    '',
    'public extension ShapeStyle where Self == ThemeColor {',
    ...names.map(n => `    static var ${swift(n)}: ThemeColor { ThemeColor(.${swift(n)}) }`),
    '}',
  ];
}

function swiftTextRole(token, codeFont) {
  const { fontFamily, fontSize, fontWeight } = token.value;
  const style = token.extensions['tectr.ios'].textStyle;
  if (fontFamily === codeFont) return `.code(size: ${fontSize}, relativeTo: .${style})`;
  const weights = { 400: 'regular', 500: 'medium', 600: 'semibold' };
  if (!weights[fontWeight]) throw new Error(`No Swift weight for ${fontWeight}`);
  return `.system(.${style}, weight: .${weights[fontWeight]})`;
}

function swiftScales() {
  const ios = loadTokens('scale.ios');
  const group = prefix => Object.entries(ios).filter(([name]) => name.startsWith(`${prefix}.`))
    .map(([name, token]) => [name.slice(prefix.length + 1), token]);
  const section = (title, lines) => [`    public enum ${title} {`, ...lines.map(line => `        ${line}`), '    }'];
  return [
    'public enum Tokens {',
    '    static let codeFontName = ' + JSON.stringify(ios['font.code'].value),
    '',
    // Line heights stay out: Dynamic Type text styles bring their own leading.
    ...section('Text', group('text').map(([key, t]) => `public static let ${swiftKey(key)}: Font = ${swiftTextRole(t, ios['font.code'].value)}`)),
    '',
    ...section('Space', group('space').map(([key, t]) => `public static let ${swiftKey(key)}: CGFloat = ${t.value}`)),
    '',
    ...section('Radius', group('radius').map(([key, t]) => `public static let ${swiftKey(key)}: CGFloat = ${t.value}`)),
    '',
    ...section('Size', group('size').map(([key, t]) => `public static let ${swiftKey(key)}: CGFloat = ${t.value}`)),
    '',
    '    /// Durations in seconds. Prefer system springs where SwiftUI offers them.',
    ...section('Motion', group('motion').map(([key, t]) => `public static let ${swiftKey(key)}: Double = ${t.value / 1000}`)),
    '}',
  ];
}

function swiftIcons() {
  return [
    'public enum StatusGlyph: String, CaseIterable, Sendable {',
    ...Object.keys(loadIcons()).map(name => `    case ${name}`),
    '',
    '    public var assetName: String { "status-\\(rawValue)" }',
    '    public var image: Image { Image(assetName, bundle: .module) }',
    '}',
  ];
}

function buildSwift() {
  return [
    SWIFT_HEADER.trimEnd(),
    'import SwiftUI',
    '',
    ...swiftColors(),
    '',
    ...swiftScales(),
    '',
    ...swiftIcons(),
    '',
  ].join('\n');
}

const assetJson = value => `${JSON.stringify(value, null, 2)}\n`;

function iconCatalog() {
  const catalog = `${IOS_PACKAGE}/Resources/Icons.xcassets`;
  const info = { author: 'xcode', version: 1 };
  const files = { [`${catalog}/Contents.json`]: assetJson({ info }) };
  for (const [name, svg] of Object.entries(loadIcons())) {
    const set = `${catalog}/status-${name}.imageset`;
    files[`${set}/${name}.svg`] = `${svg}\n`;
    files[`${set}/Contents.json`] = assetJson({
      images: [{ filename: `${name}.svg`, idiom: 'universal' }],
      info,
      properties: { 'preserves-vector-representation': true, 'template-rendering-intent': 'template' },
    });
  }
  return files;
}

// Paths are relative to the repo root.
export function outputs() {
  return {
    'design/tokens/generated/tokens.css': buildCss(),
    'design/tokens/generated/preview-data.js': buildPreviewData(),
    'design/tokens/generated/desktop-defaults.json': buildDesktopDefaults(),
    [`${IOS_PACKAGE}/Generated/Tokens.swift`]: buildSwift(),
    ...iconCatalog(),
  };
}

export function staleFiles() {
  return Object.entries(outputs())
    .filter(([file, content]) => !existsSync(join(ROOT, file)) || readFileSync(join(ROOT, file), 'utf8') !== content)
    .map(([file]) => file);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  if (process.argv.includes('--check')) {
    const stale = staleFiles();
    if (stale.length) {
      console.error(`Out of date: ${stale.join(', ')}. Run node design/tokens/build.mjs`);
      process.exit(1);
    }
    console.log('Generated files are up to date.');
  } else {
    const files = Object.entries(outputs());
    for (const [file, content] of files) {
      mkdirSync(dirname(join(ROOT, file)), { recursive: true });
      writeFileSync(join(ROOT, file), content);
    }
    console.log(`Wrote ${files.length} files.`);
  }
}

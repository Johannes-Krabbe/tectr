// Run with: node --test design/tokens/tokens.test.mjs
import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { loadBoard, loadIcons, loadTokens, staleFiles } from './build.mjs';

const dark = loadTokens('color.dark');
const light = loadTokens('color.light');
const desktop = loadTokens('scale.desktop');
const ios = loadTokens('scale.ios');
const MODES = { dark, light };

function parse(hex) {
  const m = hex.match(/^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})?$/);
  assert.ok(m, `not a hex color: ${hex}`);
  return { rgb: [m[1], m[2], m[3]].map(h => parseInt(h, 16)), a: m[4] ? parseInt(m[4], 16) / 255 : 1 };
}

function over(top, bottom) {
  const { rgb, a } = parse(top);
  const below = parse(bottom).rgb;
  const mixed = rgb.map((c, i) => Math.round(c * a + below[i] * (1 - a)));
  return `#${mixed.map(c => c.toString(16).padStart(2, '0')).join('')}`;
}

function luminance(hex) {
  const [r, g, b] = parse(hex).rgb.map(c => c / 255).map(c => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(a, b) {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

const SURFACES = ['base', 'panel', 'raised', 'overlay', 'control'];

// [foreground token, background tokens layered bottom to top]
function textPairs() {
  const pairs = [['color.text.on-accent', ['color.accent.solid']]];
  const onSurfaces = ['color.text.primary', 'color.text.secondary', 'color.text.tertiary', 'color.accent.text'];
  for (const fg of onSurfaces) {
    for (const s of SURFACES) pairs.push([fg, [`color.surface.${s}`]]);
  }
  for (const fg of ['color.status.waiting', 'color.status.failed', 'color.status.online']) {
    for (const s of ['base', 'panel', 'raised', 'overlay']) pairs.push([fg, [`color.surface.${s}`]]);
  }
  // text.tertiary is deliberately left out of the strong tint: the spec uses status.waiting there.
  const onTint = {
    'waiting-tint': ['color.text.primary', 'color.text.secondary', 'color.text.tertiary', 'color.status.waiting'],
    'waiting-tint-strong': ['color.text.primary', 'color.text.secondary', 'color.status.waiting'],
  };
  for (const [tint, foregrounds] of Object.entries(onTint)) {
    for (const fg of foregrounds) {
      pairs.push([fg, ['color.surface.base', `color.status.${tint}`]]);
      pairs.push([fg, ['color.surface.panel', `color.status.${tint}`]]);
    }
  }
  pairs.push(['color.diff.add-text', ['color.surface.base', 'color.diff.add-bg']]);
  pairs.push(['color.diff.del-text', ['color.surface.base', 'color.diff.del-bg']]);
  for (const role of ['plain', 'keyword', 'string', 'constant', 'function', 'type', 'comment', 'punctuation']) {
    const fg = `color.syntax.${role}`;
    pairs.push([fg, ['color.surface.base']]);
    pairs.push([fg, ['color.surface.panel']]);
    pairs.push([fg, ['color.surface.base', 'color.diff.add-bg']]);
    pairs.push([fg, ['color.surface.base', 'color.diff.del-bg']]);
  }
  return pairs;
}

test('light and dark define the same tokens', () => {
  assert.deepEqual(Object.keys(light).sort(), Object.keys(dark).sort());
});

test('desktop and iOS define the same tokens', () => {
  assert.deepEqual(Object.keys(ios).sort(), Object.keys(desktop).sort());
});

test('every color is a resolved hex value', () => {
  for (const tokens of Object.values(MODES)) {
    for (const [name, { type, value }] of Object.entries(tokens)) {
      if (type === 'color') assert.doesNotThrow(() => parse(value), name);
    }
  }
});

test('every iOS text role maps to a Dynamic Type style', () => {
  const styles = new Set(['largeTitle', 'title', 'title2', 'title3', 'headline', 'subheadline', 'body', 'callout', 'footnote', 'caption', 'caption2']);
  for (const [name, token] of Object.entries(ios)) {
    if (token.type === 'typography') assert.ok(styles.has(token.extensions?.['tectr.ios']?.textStyle), name);
  }
});

test('iOS controls meet the 44pt touch target', () => {
  for (const name of ['size.row', 'size.control', 'size.input']) assert.ok(ios[name].value >= 44, name);
});

for (const [mode, tokens] of Object.entries(MODES)) {
  test(`${mode}: text pairs meet WCAG AA`, () => {
    const failures = [];
    for (const [fg, layers] of textPairs()) {
      const bg = layers.map(l => tokens[l].value).reduce((below, top) => over(top, below));
      const ratio = contrast(tokens[fg].value, bg);
      if (ratio < 4.5) failures.push(`${fg} on ${layers.join(' + ')}: ${ratio.toFixed(2)}`);
    }
    assert.deepEqual(failures, []);
  });

  test(`${mode}: syntax colors never reuse the signal colors`, () => {
    const signals = [tokens['color.status.waiting'].value, tokens['color.status.failed'].value];
    for (const [name, { value }] of Object.entries(tokens)) {
      if (name.startsWith('color.syntax.')) assert.ok(!signals.includes(value), name);
    }
  });
}

test('status icons are one color and share a grid', () => {
  const icons = loadIcons();
  assert.deepEqual(Object.keys(icons).sort(), ['done', 'failed', 'idle', 'stale', 'waiting', 'working']);
  for (const [name, svg] of Object.entries(icons)) {
    assert.match(svg, /viewBox="0 0 14 14"/, name);
    assert.match(svg, /currentColor/, name);
    assert.doesNotMatch(svg, /#[0-9a-fA-F]{3,8}\b/, name);
  }
});

test('the board fixture is consistent with the domain model', () => {
  const board = loadBoard();
  const icons = loadIcons();
  const devices = new Set(board.devices.map(d => d.id));
  const sessions = new Set();
  for (const workspace of board.workspaces) {
    assert.ok(devices.has(workspace.device), `${workspace.id}: unknown device`);
    for (const topic of workspace.topics) {
      const worktrees = new Set(topic.worktrees.map(w => w.id));
      for (const session of topic.sessions) {
        sessions.add(session.id);
        assert.ok(worktrees.has(session.worktree), `${session.id}: unknown worktree`);
        assert.ok(icons[session.state], `${session.id}: unknown state ${session.state}`);
      }
    }
  }
  assert.ok(sessions.has(board.request.session), 'request: unknown session');
  const roles = new Set(Object.keys(dark).filter(n => n.startsWith('color.syntax.')).map(n => n.split('.')[2]));
  for (const file of board.review.files) {
    for (const hunk of file.hunks) {
      for (const line of hunk.lines) {
        for (const [role] of line.spans) assert.ok(roles.has(role), `unknown syntax role ${role}`);
      }
    }
  }
});

test('app code never contains raw colors', () => {
  const root = fileURLToPath(new URL('../..', import.meta.url));
  const rawColor = /#[0-9a-fA-F]{3,8}\b|\b(rgba?|hsla?|oklch)\(|Color\((red|white|hue):|\bColor\.(white|black|gray|red|orange|yellow|green|blue|purple)\b/;
  const found = [];
  for (const dir of ['apps/dsk/src', 'apps/ios/Tectr', 'apps/ios/TectrDesign/Sources']) {
    for (const file of readdirSync(join(root, dir), { recursive: true, encoding: 'utf8' })) {
      if (!/\.(tsx?|css|swift)$/.test(file) || file.includes('Generated/')) continue;
      readFileSync(join(root, dir, file), 'utf8').split('\n').forEach((line, i) => {
        if (rawColor.test(line)) found.push(`${dir}/${file}:${i + 1}`);
      });
    }
  }
  assert.deepEqual(found, []);
});

test('generated files are up to date', () => {
  assert.deepEqual(staleFiles(), []);
});

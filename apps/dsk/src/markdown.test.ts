import assert from "node:assert/strict";
import { test } from "node:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { Markdown, spansFor } from "./markdown.ts";

const render = (text: string) => renderToStaticMarkup(createElement(Markdown, { text }));

test("a heading shows a marker for its level", () => {
  assert.equal(render("# One"), '<h1><span class="md-mark"># </span>One</h1>');
  assert.equal(render("### Three"), '<h3><span class="md-mark">### </span>Three</h3>');
});

test("inline formatting and code are compiled", () => {
  assert.equal(render("**bold** and `code`"), "<p><strong>bold</strong> and <code>code</code></p>");
});

test("GitHub's extensions are compiled", () => {
  assert.match(render("| a |\n|---|\n| b |"), /<table>/);
  assert.match(render("- [x] done"), /type="checkbox"/);
  assert.match(render("~~gone~~"), /<del>gone<\/del>/);
});

test("a code block with a language waits plain for its colors", () => {
  assert.match(
    render("```rust\nfn main() {}\n```"),
    /<pre><code><span class="syntax-plain">fn main\(\) \{\}\n<\/span><\/code><\/pre>/,
  );
});

test("a code block without a language has no colors", () => {
  assert.match(render("```\nfn main() {}\n```"), /<pre><code>fn main\(\) \{\}\n<\/code><\/pre>/);
});

test("every code block has a copy button outside its code", () => {
  for (const block of ["```rust\nfn main() {}\n```", "```\nplain\n```"]) {
    const html = render(block);
    assert.match(html, /^<div class="md-code"><pre>/);
    assert.match(html, /<\/pre><button type="button" class="md-copy"[^>]*>Copy<\/button><\/div>$/);
  }
});

test("inline code has no copy button", () => {
  assert.doesNotMatch(render("some `code` here"), /md-copy/);
});

test("text that arrived after the colors stays plain", () => {
  const colored = [{ role: "keyword", text: "fn" }] as const;
  assert.deepEqual(spansFor("fn main", [...colored]), [
    { role: "keyword", text: "fn" },
    { role: "plain", text: " main" },
  ]);
  assert.deepEqual(spansFor("fn", [...colored]), [{ role: "keyword", text: "fn" }]);
});

test("colors for different text aren't applied", () => {
  assert.deepEqual(spansFor("let", [{ role: "keyword", text: "fn" }]), [
    { role: "plain", text: "let" },
  ]);
});

test("a web link can't navigate the app, only open the browser", () => {
  const html = render("[docs](https://example.com)");
  assert.equal(html, '<p><a class="md-link" title="https://example.com">docs</a></p>');
});

test("anything but a web link is plain text", () => {
  assert.equal(render("[main](src/main.rs)"), '<p><span title="src/main.rs">main</span></p>');
  assert.equal(render("[mail](mailto:a@b.c)"), '<p><span title="mailto:a@b.c">mail</span></p>');
});

test("an image is never fetched", () => {
  const html = render("![chart](https://example.com/x.png)");
  assert.equal(html, '<p><a class="md-link" title="https://example.com/x.png">chart</a></p>');
});

test("raw HTML from an agent never becomes elements", () => {
  const html = render('<img src="https://example.com/x.png"> <script>alert(1)</script>');
  assert.doesNotMatch(html, /<img|<script/);
});

test("an unfinished message still renders", () => {
  assert.match(render("```js\nconst a ="), /<pre><code><span class="syntax-plain">const a =/);
  assert.equal(render("**not closed"), "<p>**not closed</p>");
});

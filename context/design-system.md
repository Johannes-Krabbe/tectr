# tectr: design system decisions

Last updated: 2026-11-10.

`architecture.md` covers how tectr is built. This file records how the desktop and iOS apps keep one design language while each feels native. The work itself lives in `design/`.

Statuses match `architecture.md`:

- **Settled**: explicitly agreed.
- **Proposed**: recommended and not yet explicitly confirmed. Change it freely.

The desktop and phone layouts aren't settled, so nothing here decides a layout.

## Approach

Share the design decisions, not the components. No UI library renders native SwiftUI and web UI from one codebase without losing the native feel. So both apps use the same tokens and rules, and each builds its own components. (Proposed)

| Layer | Approach | Examples |
|---|---|---|
| Identical everywhere | Generated from one source, or kept in shared Rust | Colors, status meanings and icons, wording, Geist Mono for code, diff and syntax colors |
| Same role, different values | Same token names, set per platform | Type sizes, spacing, radius, motion, row height |
| Platform-owned | No attempt to match | Navigation, glass, gestures, hover, hotkeys |

## Decisions

| Decision | Status | Why |
|---|---|---|
| The iOS app targets iOS 26 and later | Settled | Liquid Glass needs no fallback code. Losing older iOS should matter little for a new developer tool |
| Light and dark mode both exist from the start | Settled | Every token and component is designed for both from day one |
| The desktop UI uses React with Base UI | Settled | Largest ecosystem, and Base UI gives behavior and accessibility without a look to fight. Only the sheets use it so far |
| No shadcn default styling on desktop | Proposed | The original brief was not to look like every other recent app |
| No third-party UI kit on iOS | Proposed | SwiftUI components are what make it feel native. Drop to UIKit only if the streaming transcript or diffs need it, which needs a performance test |
| Tokens use a small dependency-free script, not Style Dictionary | Proposed | About 70 tokens, and light/dark outputs would need custom formats anyway. Revisit if outputs multiply |
| Generated token files are committed, with a check for drift | Proposed | iOS builds then don't need Node, which also avoids the Xcode and mise pitfall in `architecture.md`. UniFFI bindings stay out of git as planned |
| Desktop imports `design/tokens/generated/tokens.css` directly, while iOS gets generated files inside its package | Proposed | Vite can read the file where it is, so no copy can drift. A Swift package can't reach outside its folder |
| iOS colors are generated as code rather than an asset catalog | Proposed | Names are checked by the compiler, and there's no lookup that can fail silently. Icons stay in a catalog, which is what gives them vector scaling and template rendering |
| Highlighting and diffing run in Rust, which sends spans labelled by role (`keyword`, `added`) | Settled | Both apps get the same result, and each maps roles to tokens. This also meets the off-UI-thread rule. Highlighting is built in `crates/highlight`. Diffs come from git on the daemon, as hunks of lines, which the app highlights one side at a time (see Diff tabs in `architecture.md`) |
| Status icons are drawn by hand, and other desktop icons come from Lucide through one `Icon` component | Built | Status icons need the cut-outs, one color for iOS templates, and layers to move between states, which no library has. Lucide covers the rest at a 1.5px stroke, and only `ui/Icon.tsx` imports it, so an icon can be swapped or drawn in one place. iOS uses SF Symbols |
| Components have the same names, parts, and states on both platforms | Proposed | People and agents can talk about one `RequestCard`. See `design/spec/components.md` |
| The logo is the Joint mark, a T-shaped tenon set into a block, in one color | Settled | Tectonics, the craft of joining parts, is where the name comes from. One color lets it follow any theme. Drafts are in `design/mockups/logo/` |
| App icons are the light mark on a dark tile, and the name is Inter SemiBold, outlined | Proposed | The dark tile matches the near-black direction and keeps purple for primary actions. Outlining means no logo file depends on an installed font |
| Both apps use a Liquid Glass app icon from an Icon Composer document, with the flat `.icns` as the fallback on older macOS | Proposed | It's how app icons look on macOS 26 and iOS 26. The build generates the document, so it stays in step with the mark and the tokens. Tauri compiles it into `Assets.car`, and iOS needs no flat icon because it targets iOS 26 |
| Fonts are bundled, not loaded from a CDN | Proposed | Instant startup, and it works offline. Desktop uses Fontsource's Inter and Geist Mono packages, and iOS ships a Geist Mono TTF with its license. The mockups and preview page use Google Fonts only for convenience |
| Both apps follow the system appearance, with a manual override | Proposed | Expected on both platforms. Desktop has the override, in its appearance settings |
| Desktop offers other people's themes alongside its own | Settled | People asked to choose. Tectr's own two stay the default, and everything in Color below that's about the look applies to those two |

## Liquid Glass

The rule is **content is ours, chrome is Apple's**. (Proposed)

- **Glass:** the tab bar, toolbars, floating buttons, and sheets, mostly from standard SwiftUI components.
- **Our design:** transcripts, diffs, and rows sit on our surface tokens. Guides on Liquid Glass also say to use it on controls and navigation, not content backgrounds.
- **Purple:** only on the primary action, not as a tint on every glass element.
- **Desktop stays opaque:** native glass behind a Tauri webview needs the macOS private API, which has App Store implications.
- **Desktop title bar:** the native macOS one, made transparent over a window colored `surface.base`, and never drawn in HTML. macOS draws the bar in a separate window in full screen, so `color_title_bar` in `apps/dsk/src-tauri/src/main.rs` colors it through private AppKit views, as Ghostty does. If a macOS update renames them, the full-screen bar falls back to the system's color. This is the workspace's only `unsafe` code, so `dsk` uses `deny` where other crates use `forbid`.

How glass looks over near-black and over our light surfaces is untested. HTML can't show it, so it has to be judged in a real SwiftUI screen on a device.

## Color

| Decision | Status | Why |
|---|---|---|
| Light mode maps the same roles to new values rather than inverting dark | Proposed | Elevation works differently: dark surfaces get lighter as they rise, while light mode uses white panels on a faint gray base |
| `surface.overlay` is separate from `surface.raised` | Proposed | They match in dark. In light, a sheet is white while a raised block is gray |
| Purple stays out of surfaces in both modes, with the same `accent.solid` in both | Proposed | This is the brand signature, and it extends the near-black direction the design started from |
| `status.waiting` is a dark amber in light mode | Proposed | Bright amber is unreadable on white. Bright amber remains in the light tints so rows still read as warm |
| Amber and red never appear in syntax colors | Proposed | They mean "waiting" and "failed" |
| Text pairs meet WCAG AA (4.5:1), enforced by a test | Proposed | The old dark tertiary text (`#7c7a89`) failed on raised surfaces and controls, so it moved to `#8a8896`. It still fails on `status.waiting-tint-strong`, so the spec bans that pair |
| Other themes keep their published colors, including surfaces, accents, and amber or red in syntax | Proposed | Someone choosing Gruvbox wants Gruvbox, even though it colors keywords red |
| The 16 ANSI terminal colors are `terminal.*` tokens worked out from colors every theme has: red, green and yellow from the status colors, blue, magenta and cyan from syntax, and the bright ones leaning toward the text color. A theme can set them itself | Proposed | Terminals then match each theme without every theme listing 16 more colors. In light themes black is the text color and white a light surface, because that's how programs use them. They reach 3:1 on the main pane, enforced with the other readability checks |
| Other themes still have to be readable: text and status colors reach AA, and syntax colors reach 3:1 | Proposed | Many published palettes fall short, Solarized and Catppuccin Latte most of all. `themes.toml` moves a color only as far as it has to and keeps the published one in a comment. A test in `crates/settings` enforces this, more loosely for syntax because that's what makes a theme recognizable. Themes people write themselves aren't checked |

## Typography

| Decision | Status | Why |
|---|---|---|
| Inter on desktop, SF on iOS | Proposed | SF gives Dynamic Type and a native feel, and the two are close enough to read as one voice |
| Geist Mono for code on both | Proposed | The shared signature, since this is an app about code |
| iOS roles map to Dynamic Type text styles, with code scaling relative to one | Proposed | Text respects the user's size setting. See `design/tokens/scale.ios.tokens.json` |
| Everything in an agent's message is one size, including headings and code | Settled | Headings stand out by weight and a gray `#` marker instead of a larger size. That's at the default sizes: text and code sizes are separate settings on desktop, and code in a message follows the code size |
| Rows, controls and icons grow with the text size on desktop | Proposed | Larger text in rows of a fixed height would clip. Density is a separate setting that scales rows and spacing |

## Component rules

Each component in `design/spec/components.md` is built separately per platform, and these native equivalents are the starting point. (Proposed; the layouts aren't settled.)

| Desktop | iOS |
|---|---|
| Sidebar tree | `NavigationStack` and `List` |
| ⌘K palette | `.searchable` |
| Menu on a button or a right click | System context menu |
| Creation sheet | `.sheet` with detents |
| Tab strip and split | One tab at a time, with a native switcher |

## Repository

Proposed, extending the layout in `architecture.md`.

```
design/
├── tokens/        # source of truth, build script, tests
│   └── generated/ # tokens.css and preview data, committed
├── icons/         # status icons, one color, cut-outs instead of inner colors. Other icons are Lucide
├── logo/          # mark, name, app icons, favicons, and the build that draws them
├── spec/          # component anatomy and wording
├── fixtures/      # the same fake state for both apps
├── preview/       # one page showing everything in both modes
└── mockups/       # earlier explorations, kept for reference
apps/dsk/src/
└── ui/            # base styles, then components
apps/ios/
├── TectrDesign/   # Swift package: generated tokens + icons, fonts, theme, then components
└── TectrCore/     # planned Rust wrapper
```

- `TectrDesign` doesn't depend on `TectrCore`, so SwiftUI previews don't need the slow xcframework build.
- App code never contains raw colors. `design/tokens/tokens.test.mjs` enforces this.
- Both apps render `design/fixtures/`, and their screenshots are reviewed side by side. Pixel diffs between platforms are pointless, because the apps should differ.

## Not done yet

- **Components:** both apps have the tokens, fonts, and root theme. Desktop has first versions of `StatusIcon`, `DeviceHeader`, `WorkspaceRow`, `TopicRow`, `RequestCard`, `ActivityLine`, `NewTab`, `Sheet`, `Button`, `Kbd`, `Sidebar`, `TabBar`, `Menu`, `Toast`, and the ⌘K palette, without the folded states. iOS shows only a placeholder.
- **Appearance on iOS:** it follows the system only, with tectr's own themes. Choosing a theme is desktop only.
- **Automated token check:** nothing runs `mise run tokens:test` or the package tests in CI yet.

## Open questions

- **Terminal colors:** the ANSI palette for both modes isn't defined.
- **Increase Contrast and Reduce Transparency:** no token variants exist yet.

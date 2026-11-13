# design

The design language shared by the desktop and iOS apps. The decisions behind it are in `context/design-system.md`.

- `tokens/`: colors for light and dark, and scales for desktop and iOS. Edit the `*.tokens.json` files, never `generated/`.
- `icons/`: status icons, drawn by hand. Other desktop icons come from Lucide through `apps/dsk/src/ui/Icon.tsx`.
- `logo/`: the mark, the name, app icons and favicons. See Logo below.
- `spec/`: component parts and states, and wording.
- `fixtures/`: fake state that both apps can render.
- `preview/`: open `preview/index.html` to see all of it in both modes.
- `mockups/`: earlier explorations. They don't use these tokens.

After changing tokens, icons, or fixtures, run these from the repo root, or use `mise run tokens` and `mise run tokens:test`:

```
node design/tokens/build.mjs
node --test design/tokens/tokens.test.mjs
```

The tests check that both modes and both platforms define the same tokens, that text meets WCAG AA contrast, that status icons stay one color, that the fixture matches the domain model, that app code has no raw colors, and that generated files are current.

## How the apps use this

**Desktop** (`apps/dsk`): `src/main.tsx` imports `tokens/generated/tokens.css` directly, so there's no copy to keep in step. Use the CSS variables, such as `var(--color-surface-panel)` and `font: var(--text-body)`. `src/appearance.ts` sets the chosen theme and fonts as variables on `<html>`, so everything built on these follows them. Sizes that have to fit text use `--unit`, which is 1px at the default text size.

**iOS** (`apps/ios`): the `TectrDesign` package holds the generated Swift and icon catalog, because a Swift package can't reach outside its folder. Apply `.tectrTheme()` once at the root, then use `.foregroundStyle(.textSecondary)`, `.background(.surfacePanel)`, `.font(Tokens.Text.body)`, `Tokens.Space.md`, and `StatusGlyph.waiting.image`.

To test the package from the command line:

```
cd apps/ios/TectrDesign
xcodebuild test -scheme TectrDesign -destination 'platform=iOS Simulator,name=iPhone 17'
```

If `xcode-select` points at the Command Line Tools, put `DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer` in front of that command.

## Logo

The mark is the Joint: a T-shaped tenon set into a block, after the tectonics in architect. It's one color, so it follows any theme. `logo/build.mjs` draws everything from the mark's geometry and the color tokens. The name is Inter SemiBold, outlined, so no file needs the font.

| File | For |
|---|---|
| `mark.svg`, `lockup.svg` | The mark alone, and the mark with the name, in `currentColor` for inline use |
| `mark-on-dark.svg`, `mark-on-light.svg`, `lockup-on-*.svg` | The same in fixed colors, for places without CSS such as the README |
| `AppIcon.icon` | The Liquid Glass app icon for macOS 26 and iOS 26, an Icon Composer document. The tenon and the block are separate glass layers |
| `AppIconDev.icon`, `app-icon-ios-dev.svg` | The same icon on a green tile, the online green darkened. Debug builds of the iOS app use it, as a separate app called tectr dev, so a build from Xcode sits next to the TestFlight one and the two are easy to tell apart |
| `app-icon-macos.svg`, `app-icon.icns` | The flat macOS app icon, on Apple's 1024 grid with its margin and shadow. Older macOS falls back to it |
| `app-icon-ios.svg` | A flat, opaque full-bleed icon, the source for the web icons |
| `tile.svg` | A rounded tile, the source for the raster favicons |
| `social.svg` | A 1280x640 preview image, for example GitHub's social preview |
| `favicon/` | A web set: `favicon.svg` follows light and dark, `favicon.ico`, PNGs, `apple-touch-icon.png` and `icon-192/512.png` for a manifest |
| `png/` | Every SVG above rendered at common sizes |

After changing the mark, the name or the colors, run `mise run logo`. It rewrites the files here, the desktop app's `src-tauri/icons/` and the iOS app's `AppIcon.icon` and `AppIconDev.icon`. It renders with the Tauri CLI and `sips`, so it needs a Mac and `pnpm install` in `apps/dsk`. `mise run tokens:test` fails when the SVGs fall behind the tokens.

To tune the glass, open `AppIcon.icon` in Icon Composer, which comes with Xcode 26 or later, and copy what you settle on into `glassIcon` in `build.mjs`, or the next build overwrites it. Compiling it takes `actool` from Xcode 26 or later. Xcode builds do that by themselves. `mise run release:build` uses Xcode even when `xcode-select` points at the Command Line Tools, and fails if the app comes out without the icon.

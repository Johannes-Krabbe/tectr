# design

The design language shared by the desktop and iOS apps. The decisions behind it are in `context/design-system.md`.

- `tokens/`: colors for light and dark, and scales for desktop and iOS. Edit the `*.tokens.json` files, never `generated/`.
- `icons/`: status icons.
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

**Desktop** (`apps/dsk`): `src/main.tsx` imports `tokens/generated/tokens.css` directly, so there's no copy to keep in step. Use the CSS variables, such as `var(--color-surface-panel)` and `font: var(--text-body)`. Set `data-theme="light"` or `"dark"` on `<html>` to override the system appearance.

**iOS** (`apps/ios`): the `TectrDesign` package holds the generated Swift and icon catalog, because a Swift package can't reach outside its folder. Apply `.tectrTheme()` once at the root, then use `.foregroundStyle(.textSecondary)`, `.background(.surfacePanel)`, `.font(Tokens.Text.body)`, `Tokens.Space.md`, and `StatusGlyph.waiting.image`.

To test the package from the command line:

```
cd apps/ios/TectrDesign
xcodebuild test -scheme TectrDesign -destination 'platform=iOS Simulator,name=iPhone 17'
```

If `xcode-select` points at the Command Line Tools, put `DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer` in front of that command.

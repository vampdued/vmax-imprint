# Caption Studio — VMAX brand + UI/UX polish plan

**Status:** DRAFT — awaiting his approval. No code changed.
**Date:** 2026-11-17
**Reviewed:** 6 screenshots from his Windows run (1920×1080, app maximised) +
`src/theme.rs`, `src/render.rs`, `src/ui.rs`, `src/icons.rs` + VMAX Brand Kit v1.1
(`~/workspace/user/files/vmax-brand-kit.zip`, extracted to
`~/workspace/work/vmax-brand-kit/`).
**Skill used:** `design-critique` (Anthropic Design plugin v1.2.0, same as the
2026-11-17 code-grounded critique). Screenshots are the ground truth for
rendered visuals; the code is the ground truth for structure and tokens.

## 1. What works already (don't touch)

- Left panel scrolls; OCR Threshold Tuning collapsed by default — the
  2026-11-17 fix pass held up on his machine.
- "Uniform — plates grow" is the default and the plates do grow; the bottom
  block is back on one line where it used to wrap.
- View switcher (Rendered / OCR Boxes / Original) reads as one choice; the
  Original view is genuinely useful for comparing against the source.
- OCR Boxes view: green = kept, orange = filtered, with the legend next to
  the image it describes. Boxes hug the text tightly.
- Big blue workflow buttons (Scan / Render) are the clear primary actions;
  Stop is correctly disabled when idle.
- JSON editor has syntax highlighting and wraps long lines; Activity Log
  gives per-file kept/filtered counts.
- The app is already fast and stable on his run: 18 files scanned, all OK.

## 2. Findings

### 2.1 🔴 Critical — rendered plates clip text at the right image edge

**Evidence (screenshots 4 & 6, Rendered view, image `_3.jpg`):**

- Top plate: "Appdiye kajal M TECH mudichitu oru lecturer ah job vaangita"
  — the text is cut mid-word at the image's right edge.
- Bottom plate: "Kaju vum engayum purushanoda designation pathi sonnadhu
  illa" — the first line is cut at the right edge and "illa" spills onto a
  second line below the plate.

**This is in the rendered output files, not just the preview.** Root cause,
verified in `src/render.rs`:

- `UniformGrow` passes `max_w = wi` (the full image width) to
  `fit_text_to_box`, so a line may be up to `wi` wide.
- But the text is drawn at `start_x = (pad_x + 12).max(x1)` — roughly 26–40 px
  in from the left edge. A line `wi` wide starting there overruns the right
  edge by exactly that offset.
- The pill rect is clamped by `clamp_box`, but the glyphs are not — tiny-skia
  clips them at the pixmap edge. So the plate looks complete while the text
  inside it is cut.

**Fix (recommended, option A):** the fit width must be the width actually
available from the text origin: `max_w = wi - start_x - pad_x`, computed
after `start_x` is known. The wrap/shrink then accounts for where the text
starts, and a long translation wraps to a second line (or shrinks) instead of
running off the image. One-line behaviour is unchanged for translations that
fit.

- Option B (not recommended): centre plates horizontally. Changes the look —
  plates are currently anchored to the original block's position, which
  matches the source captions.
- Option C (not recommended): shrink-to-fit only, never wrap. Loses the
  grow behaviour he just picked.

**Also:** add the same guard to the shared path so no mode can draw past the
right edge (`start_x + line_width + pad_x ≤ wi`, shrink if it ever would),
plus a regression test: a long translation at uniform size in a 1440 px image
must render with all glyphs inside the image (assert no ink in the rightmost
column band beyond the plate, or assert fitted width + origin ≤ image width).

### 2.2 🟡 Preview — file picker doesn't scale to 18 near-identical names

**Evidence (screenshot 5):** the file dropdown lists 18 files whose names
differ only in the trailing `_1` / `_2` / `_10`. Scanning it is slow, and the
open list covers the preview image.

**Fix:** a filter field inside the dropdown (egui's ComboBox has no built-in
filter, so a small custom popup with a text field + filtered list), and show
short names in the list with the full name as tooltip. The prev/next chevrons
already exist and stay. Keyboard: `j`/`k` or arrows to step files while the
preview has focus (cheap, egui already handles arrows in the combo).

### 2.3 🟡 Detected Blocks is one monospace blob

**Evidence (screenshots 2–4):** block #2's OCR + NEW text runs as a wrapped
paragraph; confidence, OCR text and translation blur together, and there is no
way to tell which box on the image a block is.

**Fix:** structured rows per block — `[#1]` + confidence chip (green ≥ 0.90,
amber below) + OCR text (dim) → NEW text. Clicking a row highlights that
block's box in the image (selection sync both ways: clicking a box in OCR
Boxes view selects the row). Keep the monospace verify text as a fallback
behind a "copy" action, not as the primary display.

### 2.4 🟡 JSON editor can't tell "not translated" from "translated to empty"

**Evidence (screenshot 1):** file `_1.jpg` has `"1": ""`, `"4": ""`,
`"5": ""`, `"6": ""` — empty strings for blocks with no translation.

**Fix:** status-bar count — "18 files · N blocks without a translation" —
and dim empty-string values in the editor (already syntax-highlighted; empty
strings get the faint colour). Plus JSON validity in the status bar: show
"JSON valid" or the parse error + line when it isn't, instead of failing
silently at render time.

### 2.5 🟢 Left panel — path fields truncate, no full path on hover

**Evidence (screenshots 1–6):** input/output paths show as
`C:\Users\VampDued\Downloads\!!!!!!` — truncated mid-path.

**Fix:** tooltip with the full path on hover (the Browse-button tooltips added
in the fix pass cover the buttons, not the fields). One line, no layout change.

### 2.6 🟢 Preview zoom

The preview fits the image to the pane (max 1.5×) and pans via ScrollArea —
adequate. Add a Fit / 100% toggle in the View row so he can check plate
edges at native resolution without scrolling. Small.

### 2.7 🟢 Status bar

Currently: left "Imported translations from Translated texts.json", right a
blue progress bar at 100%. Keep, and add the file/untranslated counts from
2.4. No redesign.

## 3. VMAX brand application

Brand kit facts (v1.1, `vmax-brand-guidelines.html`):

- **Palette:** Black `#000000`, White `#FFFFFF`, Max Blue `#2F5BFF`,
  Graphite `#17181B`, Steel `#9AA1AD`, blue-text-on-dark `#8FA8FF`.
  Proportions: black 55, white 25, graphite 10, steel 5, Max Blue 5.
  Max Blue in small, deliberate amounts — never as a background for long text.
- **Type:** Chakra Petch (display — headlines, titles) + IBM Plex Sans
  (everything people read: body, forms, captions, tables). Inter is the
  approved fallback. Both free on Google Fonts (OFL).
- **Logo:** the wordmark is fixed artwork — never retype "VMAX", never redraw.
  The interlock icon (V + upside-down V) is used alone where the name is
  already clear: app icons, avatars, favicons. Clear space = ¼ icon height;
  icon minimum 32 px on screen (16 px inside a tile).
- **Voice:** direct, exact, brief. "Shipped in 2 days." not
  "Blazing-fast results." One idea per sentence.
- **Release themes:** midnight (black), graphite, daylight (white), max-blue
  — approved background/text/logo pairings.
- **Accessible pairs:** white on black, black on white, white on Max Blue
  pass WCAG AA for body text. Max Blue on black passes only for large text —
  use `#8FA8FF` for small blue text on dark.

### 3.1 Tokens (current → VMAX)

| Token | Now | VMAX |
|---|---|---|
| `BRAND` / accent | indigo `#1659c4` | Max Blue `#2F5BFF` |
| `accent_ink` (blue text on dark) | `#74aaff` | `#8FA8FF` |
| `accent_soft` | `#2d81ff` @ 44 | `#2F5BFF` @ ~40 |
| `chrome` (title bar / panels) | `#101014` | `#000000` (midnight) |
| `canvas` | `#151619` | Graphite `#17181B` |
| `card` / `raised` / `field` | `#1b1c1f` / `#222325` / `#26272b` | Graphite + white overlays at 4% / 8% / 12% (derived — the kit defines only six colours; the derivation rule is documented in `theme.rs`) |
| `text` / `text_dim` / `text_faint` | `#f2f1ee` / `#b3b3b2` / `#8d8d8d` | `#FFFFFF` / Steel `#9AA1AD` / Steel @ 60% |
| `separator` / `field_border` | `#242527` / `#6c6c6c` | white @ 8% / Steel @ 70% |
| `danger` / `warning` | `#ff6259` / `#e0a23a` | unchanged (functional colours, not brand) |
| `code_bg` | `#060607` | `#000000` |

Contrast check (to verify in code, not by eye): white on Max Blue ≈ 6.2:1
(passes AA); Steel `#9AA1AD` on Graphite ≈ 6.8:1 (passes AA for body);
Max Blue on black ≈ 3.4:1 — large text / fills only, never small text.

### 3.2 Type

- **Chakra Petch 600:** app title, section headings, dialog titles only.
- **IBM Plex Sans 400/500/600:** everything else — body, labels, buttons,
  tables, captions. Replaces Figtree throughout `theme.rs`
  (Figtree stays out; the kit's approved fallback is Inter, not Figtree).
- **JetBrains Mono:** stays for the JSON editor, Activity Log and code.
- Bundle the fonts into `assets/fonts/` with OFL texts, exactly as Figtree /
  JetBrains Mono were (same `install_fonts` pattern, new faces).
- Type scale stays as-is (TITLE 26 / SECTION 18 / BODY 15 / …) — only the
  families change. Chakra Petch is a display face, so it is used at heading
  sizes only, never for body copy.

### 3.3 Logo, icon, app icon

- **Title bar:** VMAX interlock icon (white SVG, 24 px, embedded via the
  existing `icons.rs` pattern) + "Caption Studio" in Chakra Petch semibold +
  the existing subtitle in IBM Plex Sans. Clear space respected (icon height
  ÷ 4 on every side).
- **App icon (Windows exe + taskbar):** the interlock on a black tile
  (`png/vmax-icon-1024.png` → `.ico`; eframe `icon_data` at startup).
  Black tile is the primary form per the guidelines; the blue tile is for
  campaigns, not the app.
- **About dialog (new, small):** the horizontal lockup (icon + wordmark,
  `lockup/vmax-lockup-white.svg`) + version + "Caption Studio" line. The
  wordmark is used as supplied artwork only — never retyped.
- **No other logo use:** one mark per surface, per the guidelines. The
  caption plates themselves are untouched (they render the user's text, not
  the brand).

### 3.4 Light theme → VMAX "daylight"

`StudioLight` is port-complete but inactive. Re-map its tokens to the kit's
daylight theme (white bg, black text, Max Blue accent, black logo) and wire a
theme switch. The switch itself is Phase 3 (see §4); the token mapping is
done in Phase 1 so the two themes never drift again.

### 3.5 Copy — VMAX voice

Audit UI strings against "direct, exact, brief": the current strings mostly
pass ("Scan & Extract Text", "Render Translated Images", "OCR engine ready.").
Two to fix: the render summary sentence and any error that doesn't name the
next step (the fix pass already rewrote several; this pass finishes the set).
No new adjectives, no "blazing-fast" language anywhere.

### 3.6 Release graphics (out of scope for the app build)

The kit's `release/` pipeline (`vmax_release_v2.py`) generates release-post
graphics in the four themes. It is for the v1.0.4 release announcement, not
the app — noted here so it isn't forgotten at tag time.

## 4. Phased implementation plan

### Phase 1 — Correctness + brand foundation (build first)

1. **Clip fix (§2.1):** `render.rs` — fit width accounts for `start_x` +
   padding; shared-path guard; regression test
   (`uniform_grow_keeps_text_inside_image`).
2. **Tokens (§3.1):** `theme.rs` — VMAX palette; derived-surface rule
   documented; contrast assertions in tests (white-on-Max-Blue, steel-on-
   graphite).
3. **Fonts (§3.2):** download Chakra Petch + IBM Plex Sans (OFL), add to
   `assets/fonts/` + licence files, wire in `theme.rs`; Figtree removed.
4. **Title bar + app icon (§3.3):** embed `vmax-mark-white.svg` in
   `icons.rs`; title bar layout in `ui.rs`; `.ico` from
   `png/vmax-icon-1024.png`; eframe window icon.
5. **Daylight token mapping (§3.4):** `StudioLight` → daylight values (no
   switch yet).
6. **Copy audit (§3.5):** finish the next-step error strings.

**Verify:** `cargo test` (22 tests + new clip + contrast tests, 0 new
warnings); his Windows eyeball of the v1.0.4-tagged exe on the `_3.jpg`
image — top and bottom plates must read fully, nothing cut at the edge.

### Phase 2 — Preview UX (§2.2–2.4)

1. Filterable file dropdown + short names + tooltips (`ui.rs` preview tab).
2. Structured Detected Blocks rows + click-to-highlight sync
   (`ui.rs` + preview state).
3. JSON validity + untranslated-block count in the status bar; empty strings
   dimmed in the editor.
4. Full-path tooltips on the folder fields (§2.5).

**Verify:** `cargo test`; his eyeball — stepping through 18 files, clicking a
block highlights its box, status bar shows the counts.

### Phase 3 — Polish + accessibility

1. Theme switch (Studio midnight ↔ daylight) in Settings; persisted.
2. Fit / 100% preview toggle (§2.6).
3. Contrast + focus-ring pass against the VMAX pairs (§3.1); the a11y
   agent's pending findings folded in when they land.
4. About dialog with the VMAX lockup (§3.3).
5. Release graphics for the v1.0.4 tag via the kit's `release/` pipeline.

**Verify:** `cargo test`; his eyeball at 125% scaling (the standing
crispness check); release post generated from the pipeline.

## 5. Decisions needed from him (before building)

- **Q1 — Plate fix:** option A (fit width accounts for text origin) is
  recommended. B (centred plates) and C (shrink-only) are in §2.1.
- **Q2 — Phasing:** build Phase 1 first, then 2, then 3 — or Phase 1 only
  for now?
- **Q3 — App name:** keep "Caption Studio" with the VMAX icon in the title
  bar (recommended), or rename the title to "VMAX Caption Studio"?
- **Q4 — Daylight theme:** ship the switch in Phase 3 (recommended), or
  keep the app dark-only and leave daylight tokens dormant?

## 6. What this plan deliberately does NOT change

- OCR engine, detection, inpainting, JSON format, render fonts for captions
  (DejaVu / cosmic-text), batch workflow order, settings file format.
- No new dependencies except the two OFL fonts and the VMAX SVG/PNG assets.
- No push/tag until he says so (standing rule) — Phase 1 verification needs
  a tag (e.g. v1.0.4) for the Windows exe, which is his call.

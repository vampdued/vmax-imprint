# Changelog

## 1.1.0 — 2026-11-17 (Full UI, UX Copy, Accessibility, Design System & Pixel Audit Fixes)

Resolves all 27 findings from the comprehensive UI/UX, accessibility, copy, design system, and pixel-level audit:

### Critical Fixes
- **Opaque Modal Surfaces & Full-Viewport Scrim (`11`–`14`)** — Added a full-viewport dim backdrop (`rgba(0,0,0,196)`) and an opaque elevated `modal_frame` (`Order::Foreground`, `t.card` fill, `t.field_border` stroke, deep shadow, inner bordered `t.field` code frames) on all four modals (`Paste translated JSON`, `Bulk edit all JSON`, `Import summary`, and `No translated captions yet`), completely eliminating background log/JSON text bleed-through and `×` close-button collisions with underlying inspector cards.
- **Font Sizing Dropdown Auto-Dismissal & Padding (`15`–`17`)** — Explicitly closes the `Font sizing` popup whenever a batch job (`self.busy`), modal dialog (`modal_open`), or subsequent shot-test state begins, and increases menu padding (`Margin::symmetric(8, 6)`) so option text never clips at the right edge or stays stuck over the Activity drawer during Render states.
- **Contextual Primary Action CTA (`03`–`19`)** — Standardized sentence-case primary header CTAs (`Scan images`, `Render N images`, `Open output folder`) so users can trigger rendering directly from any post-scan step without getting stuck on a static `Review in Preview` button.

### High-Severity Fixes
- **Stepper Failure Visuals & Deduplicated Render Logs (`17`, `19`)** — Added `StepState::Failed` to the 4-step workflow header (renders a high-contrast `!` danger circle and red border on Step 4 when any image fails), cleared stale render summaries when staging empty-image states so Step 1 and Step 4 image totals never conflict, and deduplicated consecutive staged `Complete —` log entries.
- **WCAG 2.1 AA Disabled & Faint Text Contrast (`02`–`19`)** — Raised `Visuals::disabled_alpha` to `0.65` and brightened `text_faint` (`mix(STEEL, P1, 0.10)` = `#8A919E`, `>= 5.75:1` on `card`, `6.22:1` on `canvas`, `6.67:1` on `chrome`) so disabled `Stop`/`Scan images` buttons and secondary metadata pass WCAG AA contrast.
- **Unified Button & Modal Action Hierarchy (`11`–`14`)** — Created `primary_modal_button` (solid Max Blue `#2F5BFF` fill, white text) and `secondary_modal_button` (`t.raised` fill, `t.field_border` stroke) with right-aligned primary confirmation actions across all dialogs.
- **Inspector Header & Numbered Filtered Cards (`01`–`19`)** — Styled `DETECTED BLOCKS` in uppercase Chakra Petch 13px `accent_ink` to match `SOURCE & OUTPUT` and `ACTIVITY`, placed `Copy block list` right-aligned on the same header row with a copy icon, and numbered dropped OCR cards (`Filtered #1`, `Filtered #2`) so every card is uniquely identifiable.

### Medium & Minor Fixes
- **UX Copy & Terminology Alignment** — Renamed `Minimum text length` to `Min. unboxed text length (chars)` with explanatory tooltip; replaced internal `Fits plate` jargon with `Fits box`; clarified `Copy all JSON for AI` vs. per-image `Showing: <file>` in the JSON tab; replaced raw `failed: render error (staged)` with actionable failure copy; and reformatted import warnings to `"<file>: skipped block #N (not in scan)."` to prevent mid-line hanging em-dashes.
- **Path Normalization & Scrollbar/Target Polish** — Normalized `..` segments in folder paths (`normalize_display_path`), switched scrollbars to non-floating `10px` allocated tracks with a `6px` right inset on the thumbnail rail so selection borders never collide with the scrollbar thumb, unified `small_button` corner radii to `6px`, enforced a `12px` text floor (`TextStyle::Small = CAPTION`), added `− / +` zoom step buttons to the preview toolbar, wrapped the OCR diagnostic legend in a bordered dark strip (`■ Solid green = Kept | ▨ Dashed orange = Filtered out`), thickened OCR diagnostic strokes, and made the empty-image inspector message non-selectable so no stray vertical text cursor (`|`) appears.

## Unreleased — UI redesign (`feature/redesign-handoff`)

Rebuilds the UI to the SPEC/reference mockup (1920×1080) while keeping
every feature and fixing the 7 logged defects. Dark only — the Daylight
theme and the About dialog are removed. UI fonts are vendored:
Chakra Petch (display), IBM Plex Sans (body), IBM Plex Mono (code)
in `assets/fonts/`.

- **Phase 0** — SPEC tokens, vendored fonts, natural file sort, 1280×720
  minimum window.
- **Phase 1** — Layout shell: 60px header with clickable stepper and one
  primary per step, 316px sidebar, 380px inspector, 188px log, 32px
  status bar.
- **Phase 2** — Sidebar cards (Source & Output / OCR Thresholds /
  Project); the sizing select opens upward (defect 1); debounced instant
  re-filter with Reset; Scan-again confirm.
- **Phase 3** — Workspace: toolbar (prev/next, N of M, Rendered / OCR
  Boxes / Original + Fit/100%), 230px file list with lazy 44px
  thumbnails and status chips, stage with checkerboard, wheel zoom and
  drag pan, keyboard nav.
- **Phase 4** — Inspector block cards (Filter out / Keep per block,
  editable translations, fit indicator) + per-image read-only JSON tab
  with Paste / Import / Bulk edit.
- **Phase 5** — Structured log (newest first, zebra rows, All/Warnings
  filters, Copy log) + status bar final form (render counts, last save
  path, progress bar only while a job runs — defect 6).
- **Phase 6** — Workflow state machine final (a failed render leaves
  Render current, failed count in the subtitle) + empty-image handling
  ("No text found. This image will be copied unchanged.", empty images
  export `{}`).
- **Phase 7** — Final pass: full test suite green (44), `cargo fmt` on
  the seven touched files, this changelog, README touch-ups.

## 1.0.3 — 2026-11-17

- VMAX brand + clip fix, preview UX, theme switch (Midnight/Daylight),
  About dialog. (The theme switch and About dialog are removed again by
  the redesign above.)

## 1.0.2

- Shipped as code only; CI failed on a pyke CDN hash mismatch — no
  release.

## 1.0.1

- Fixed the double-render bug in 1.0.0.

## 1.0.0 — 2026-11-16

- First release. Carries the double-render bug — do not use.

# Changelog

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

Not built (pending his confirm): the inspector as an overlay drawer
below 1280px wide (item 19).

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

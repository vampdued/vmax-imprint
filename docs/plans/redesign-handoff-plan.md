# Caption Studio — Redesign Handoff Plan (SPEC.md + reference HTML)

**Status: PLAN ONLY — awaiting his approval. No code changes made.**
**Sources studied 2026-11-17:** `~/workspace/user/files/SPEC.md` (8.8 KB),
`~/workspace/user/files/caption-studio-handoff.zip` → extracted to
`~/workspace/work/caption-studio-handoff/h/` (15 files: 5 original screenshots,
`brand/tokens.css`, `brand/colors.json`, `brand/vmax-mark-white.svg`,
`brand/favicon.svg`, `reference/caption-studio-redesign.html` 26 KB, `h/SPEC.md`
— identical to the uploaded SPEC.md).
**Target repo:** `~/workspace/caption-studio`, local main `cebd449` (Phase 3,
4 commits ahead of origin `a4b4aab`). Stack is **Rust + eframe/egui 0.37**
— the app already exists in this stack; no Python/Electron/Tauri rewrite
(SPEC §13's stack question is answered by the repo itself).

## 1. What the materials say (one-to-one summary)

- **Mission (SPEC §0):** rebuild the UI to match `reference/*.html` at
  1920×1080, keep every existing feature, fix the 7 listed defects. The HTML
  is the visual truth; SPEC is the behavioural truth; `original-screenshots/`
  show the OLD UI, for feature parity only.
- **Layout (§3):** title bar 34 · header 60 · main flexible · log 188 ·
  status bar 32. Columns: **316 sidebar | flexible workspace | 380 inspector**.
  Workspace = toolbar over [**230 file list** | stage]. Gutter 12, card
  padding 12, radius 8 (inputs 6), controls 36, buttons 40 (small 32).
  Min window 1280×720 — below that the inspector collapses to a drawer
  (not specified in the mockup; SPEC says confirm with owner).
- **Tokens (§4):** bg `#000`, p1 `#0b0c0e`, p2 `#17181B`, p3 `#202228`,
  ln `#2b2e35`, tx `#FFF`, mu `#9AA1AD`, bl `#2F5BFF`, blt `#8FA8FF`,
  ok `#4ADE80`, wn `#FBBF24`, er `#F87171`. Fonts: Chakra Petch 500/600,
  IBM Plex Sans 400/500/600, **IBM Plex Mono** (OCR text, log, JSON, values).
  Bundle locally, no CDN. Mark: `brand/vmax-mark-white.svg`, never redrawn.
  **Dark theme only.**
- **Workflow (§2):** Scan → Translate → Review → Render. Step states
  `todo | current | done`; header shows ONE primary button for the current
  step (Scan images → Review → Render N images); step subtitles show live counts.
- **Components (§5):** folder field (full path in `title`, tail on truncate);
  sizing select (popup on its own top layer, never clips/covers neighbours);
  threshold sliders (conf 0–1 step .01 default .60; length 1–40 default 12;
  drag updates preview instantly, 50 ms debounce; Reset; Inspect boxes);
  stepper items (clickable, `aria-current="step"`); segmented
  Rendered / OCR boxes / Original; file-list rows (44 px thumb, name,
  "N blocks", status chip, **natural sort**, current = blue border,
  Up/Down move, Enter opens); block cards (title, mono conf green ≥ threshold
  else amber, kept/filtered chip, Filter out / Keep toggle, read-only OCR
  mono text, editable translation textarea, "Fits plate · N lines",
  char count); inspector tabs Blocks (N) | JSON (arrow-key nav); JSON tab
  (Copy for AI, Paste translated, Import JSON file…, **read-only**
  syntax-coloured JSON of the current image); activity log (table: time,
  status icon, file, result; filters All / Warnings with counts; newest
  first; auto-scroll off when user scrolls; Copy log; Stop job, disabled
  when idle); status bar (counts images / rendered / copied unchanged /
  failed, last save path, progress bar **only while a job runs**).
- **Stage (§6):** checkerboard dark bg, image centred, fit-to-window,
  mouse-wheel zoom, drag to pan, `0` resets. OCR boxes: green kept /
  orange filtered, label `#n · conf · kept|filtered` **outside the box,
  top-left, 11 px**, plus solid vs dashed border so colour is never the
  only cue. Clicking a box selects its block card and scrolls it into view
  (and vice versa).
- **Data model (§7):** `Block {id, bbox:[x,y,w,h], conf, ocr, text|null,
  kept}`; `ImageItem {file, blocks, status:
  untranslated|translated|rendered|unchanged|failed}`; `Project {input,
  output, font?, sizing: uniform-grow|per-block|uniform-min, minConf,
  minLen, images}`. Exchange JSON unchanged:
  `{"<filename>": {"1": "text"}}`, **block ids 1-based among KEPT blocks**,
  empty images export `{}`. Import validation: unknown filenames, missing
  ids, extra ids, non-string values → per-file error list, import the valid
  ones, never silently drop.
- **Behaviours (§8):** threshold change re-derives kept/filtered without
  re-running OCR; warn if an already-translated block becomes filtered;
  editing a translation marks the image `translated` and re-renders the
  preview; render skips no-kept-block images (copy unchanged, status
  `unchanged`, "no text"); Stop cancels cleanly between images, partial
  output kept; long jobs off the UI thread, log appends in batches; persist
  window size, last folders, sizing mode, thresholds; errors as
  *what happened · why · how to fix*.
- **Copy deck (§9):** as written — Scan images · Render {n} images ·
  Review · Save captions · Load captions · Scan again · Open output ·
  Copy for AI · Paste translated · Import JSON file… · Filter out / Keep ·
  Stop job · Reset · Inspect boxes. Empty: "No text found. This image will
  be copied unchanged." Done: "Rendered 62 · 13 copied unchanged · 0 failed."
  Confirm (destructive, e.g. Scan again with edits): "Re-scan 75 images?
  Your edited translations will be cleared." — **Re-scan** / **Keep edits**.
- **A11y (§10, WCAG 2.1 AA):** contrast ≥ 4.5:1 text / 3:1 UI & boxes,
  visible 2 px focus ring (`--blt`) on every control, full keyboard
  operation in order sidebar → toolbar → file list → stage → inspector →
  log, accessible names on all icon buttons, colour never the only cue,
  Windows text scaling to 200% (rem/relative sizing in the real app),
  screen-reader live region for job progress.
- **Defects that must NOT reappear (§11):** (1) sizing popup covering the
  OCR section, (2) lexicographic file order, (3) duplicate Live Preview
  entry points, (4) two equal blue primary buttons, (5) hidden OCR
  thresholds, (6) always-full "100%" status bar, (7) raw-text-only
  translation editing.
- **Definition of done (§12):** side-by-side screenshot at 1920×1080
  matches `reference/` within 2 px on layout, exact on colours/fonts;
  all of §5–§8 works against real data on a 75-image folder, no UI freeze;
  keyboard-only pass + contrast check pass; deliverables: source, build
  instructions, short CHANGELOG, screenshots of each state (idle,
  scanning, review, rendering, error).

## 2. Cross-check — what the current app already has vs what changes

Verified against local main `cebd449` (files: `src/ui.rs`, `src/theme.rs`,
`src/model.rs`, `src/parser.rs`, `src/fonts.rs`, `src/render.rs`).

### Already matches (keep, do not rewrite)
- Backend untouched per §1: OCR engine (`ocr.rs`), plate renderer
  (`render.rs`, `layout.rs`, `inpaint.rs`), image I/O (`image_io.rs`).
  Render batch already copies no-kept-block images unchanged and reports
  `rendered / copied / failed` totals (`ui.rs:815-819`, `954-998`).
- Threshold re-derive without re-OCR exists (`reapply_thresholds`,
  `ui.rs:313`, needs `raw_lines` — already stored since the raw-OCR work).
- Exchange JSON shape is exactly the compact map (`parser.rs:292`
  `compact_map_from_data`); `merge_translations` already returns
  per-file warnings for ids with no matching text (`parser.rs:217-290`).
- Copy for AI / Paste Translated JSON / Import Translated JSON buttons
  exist in the JSON tab (`ui.rs:1389`).
- Sizing modes + labels are exactly the SPEC strings
  (`model.rs`: UniformGrow / PerBlock / UniformSmallest).
- Stop between images exists (cancel flag, worker checks per image).
- Settings persist folders, sizing, thresholds, panel width, theme to
  `~/.caption_replacer.json` (`model.rs:131-164`).
- Focus rings exist on every control (Phase 3); contrast test
  `vmax_contrast_pairs_pass_aa` exists (values will change with the new
  tokens — see Phase 0).

### Must change (verified deltas)
| Area | Current state (evidence) | Redesign change |
|---|---|---|
| File order | `file_names()` does `names.sort()` — plain lexicographic (`ui.rs:696-704`). Screenshot 4 shows `1, 10, 11…` | Natural sort (1, 2, 3…10) everywhere: file list, scan order, log order |
| Primary buttons | Scan AND Render are both full-width blue primaries (screenshots 1, 5) | One primary only, in the header, per current step (§2/§5) |
| Thresholds | Collapsed section, DragValues + "Apply Thresholds (Instant)" button (`ui.rs:1263`) | Always-visible sliders, drag applies instantly (50 ms debounce), Reset button; keep instant re-derive, drop the Apply button |
| Sizing popup | egui `ComboBox` popup covers the OCR section (screenshot 5) | Popup on its own top layer that never covers neighbours (§5) — custom `Popup::menu`-style layer or repositioned ComboBox; verify by screenshot |
| Live Preview | Two entry points: tab + "Live Preview & OCR Check" button (`ui.rs`) | Preview IS the stage — remove the button and the tab; the stage is always present |
| Status bar | `ProgressBar` always rendered (`ui.rs:1111`), screenshots show "100%" full at idle | Progress only while a job runs; idle shows counts + last save path only |
| File picker | Phase 2 dropdown (button + popup menu) | 230 px file list: 44 px thumbnails (new small-decode cache), name, "N blocks", status chip, filter box, keyboard nav |
| Block editing | Structured rows: OCR dim → NEW text, Copy button, click-to-highlight (Phase 2). No per-block editing, no toggle | Block CARDS per §5: editable translation textarea, Keep / Filter out toggle, fit indicator ("Fits plate · N lines"), char count; keep click-to-highlight both ways |
| JSON tab | Full-document EDITABLE editor (`editor_text`, `ui.rs:366-449`) | Per-image READ-ONLY syntax-coloured JSON + Copy for AI / Paste translated / Import; Paste opens a modal textarea (clipboard paste stays supported). Full-document editor goes away |
| Activity log | Plain text lines, `LogLine {text, level}` — no timestamp, no file field (`ui.rs:57`) | Structured log rows: time, status icon, file, result; filters All / Warnings with counts; newest first; Copy log; Stop here (not in the workflow row) |
| Stepper / steps | No step state machine at all | New `WorkflowStep` state (todo/current/done) derived from app state; header stepper (4 items, clickable) + one header primary per step; Scan-again confirm modal (§9 copy) |
| Tokens | Midnight theme: canvas `#17181B`, card +4% white, danger `#ff6259`-ish, warning `#e0a23a`, conf green `#22C55E` (`theme.rs:111-140`) | SPEC token set (§4): bg `#000`, p1 `#0b0c0e`, p2 `#17181B`, p3 `#202228`, ln `#2b2e35`, ok `#4ADE80`, wn `#FBBF24`, er `#F87171`; contrast test re-measured and re-pinned |
| Mono font | JetBrains Mono only (`assets/fonts/`) | **IBM Plex Mono** per §4 (not bundled — download, OFL, add to `assets/fonts/`); Chakra Petch Medium (500) also missing — only Bold/SemiBold bundled |
| Stage zoom/pan | Fit / 100% segmented toggle only (Phase 3); ScrollArea pan; no wheel zoom, no drag-pan, no `0` reset | Checkerboard bg, wheel zoom, drag pan, `0` reset, fit-to-window default; keep Fit/100% as zoom presets |
| OCR box labels | Label drawn at the box top edge, clipped/overlapping the caption (screenshot 3: "#1 · 0.98 · boxed" cut off) | Label OUTSIDE the box, top-left, 11 px; kept = solid green border, filtered = dashed orange border |
| Per-image status | Only totals kept after render; no per-file outcome map | New per-file status map (`rendered`/`unchanged`/`failed` + `translated`/`untranslated` derived from blocks) feeding file-list chips, stepper counts, status bar |
| Window size | No `ViewportBuilder` size in the launch (`ui.rs:1929` `NativeOptions`); size not persisted | Set initial + min size (1280×720), persist size in settings; <1280 px wide → inspector becomes an overlay drawer (§3, confirm) |
| Import validation | Warnings exist but are shown as a status line; no per-file error list | Per-file error list UI on import (unknown file / missing id / extra id / non-string), valid files still imported |
| Translated-block filtered | No warning | Warn when a threshold change filters a block that already has a translation (§8) |

### Mockup chrome that is NOT app UI (do not build)
- `.desk` / `.win` 1920×1080 scaling wrapper, `.tk` desktop taskbar strip,
  and the fake 34 px `.tb` title bar are presentation chrome for the HTML
  mockup. The real app keeps the native OS title bar (eframe); the app's
  own header is the 60 px row with brand + stepper + primary button.
- The stage's white "page" placeholder is a stand-in; the real stage shows
  the actual image (as today).
- The mockup footer shows a 100% progress bar in the DONE state — that
  contradicts SPEC §5 (progress only while a job runs) and §11 defect 6.
  Resolution: follow the SPEC text, not the static frame — no bar when idle.
- File-list rows in the mockup are generated sample data (24 of 75); the
  real list shows all scanned files.

## 3. Build plan (one commit per work item; push/tag only on his word)

**Phase 0 — Groundwork (verify + fonts + tokens)**
1. Download IBM Plex Mono (OFL, IBM/plex releases) + Chakra Petch Medium
   500; add to `assets/fonts/` with OFL texts; wire into `fonts.rs`
   (mono → IBM Plex Mono; display 500 → Chakra Petch Medium).
2. Add the SPEC token set to `theme.rs` (new `Tokens` values; keep the
   VMAX theme kinds — the redesign is the Midnight token set, renamed
   surfaces p1/p2/p3/ln). Re-measure and re-pin
   `vmax_contrast_pairs_pass_aa` against the new values (ok `#4ADE80`,
   wn `#FBBF24`, er `#F87171` on p1/p2/p3 + field).
3. Add `natural_sort` helper + unit test (1, 2, 3…10; "13 of 75.png"
   ordering); switch `file_names()` to it.
4. Verify launch options at `ui.rs:1929` (initial size, min size) before
   touching them.

**Phase 1 — Layout shell**
5. Restructure `ui.rs` top-level layout into the grid: header (60 px:
   brand mark + name + 4-step stepper + one primary button) / main row
   (316 sidebar | workspace | 380 inspector) / log (188 px) / status bar
   (32 px). Keep the existing panels' contents working inside the new
   shell; sidebar stays resizable? — No: fixed 316/380 per §3 (panel-width
   persistence key retires; note in CHANGELOG).
6. Header primary button drives the current step (label per §2); the old
   Scan/Render full-width primaries leave the sidebar.

**Phase 2 — Sidebar**
7. Source & Output card: folder fields with full path in `title`,
   tail-ellipsis on overflow, 36 px browse buttons; custom-font field;
   sizing select on a top layer (fix defect 1, verify by screenshot).
8. OCR Thresholds card: always visible; two sliders (conf .01 step,
   length 1–40) with mono value outputs; drag → debounced (50 ms) instant
   `reapply_thresholds`; Reset restores .60/12; Inspect boxes switches
   stage to OCR mode. Warn when a translated block becomes filtered.
9. Project card: Save captions / Load captions / Scan again / Open output
   (2×2 grid, secondary buttons). Scan again with edits → confirm modal
   with the §9 copy (**Re-scan** / **Keep edits**).

**Phase 3 — Workspace: file list + stage**
10. File list (230 px): filter box on top; rows with 44 px thumbnail
    (new small-decode cache, decoded lazily, capped), name, "N blocks",
    status chip (rendered ok / no text warn / failed error /
    untranslated muted); current row blue border; Up/Down move selection,
    Enter opens; natural sort.
11. Toolbar: prev/next chevrons, "N of M", segmented Rendered / OCR
    boxes / Original (blue fill on selected).
12. Stage: checkerboard dark background; image centred, fit-to-window;
    wheel zoom, drag pan, `0` reset; Fit/100% presets kept as zoom
    controls; OCR mode draws boxes with labels outside top-left
    (`#n · conf · kept|filtered`, 11 px) + solid kept / dashed filtered
    borders; click box ↔ select block card (both ways, scroll into view).

**Phase 4 — Inspector: block cards + JSON tab**
13. Blocks tab: one card per block (kept + filtered): title, mono conf
    (green ≥ threshold else amber), kept/filtered chip, Filter out / Keep
    toggle (manual override — new per-block `kept` override stored on the
    block; thresholds re-derive only non-overridden blocks), read-only OCR
    text (mono), editable translation textarea (edits mark the image
    `translated`, re-render the preview, update the compact JSON),
    "Fits plate · N lines" + char count (reuse `layout.rs` fit estimate).
14. JSON tab: per-image read-only syntax-coloured JSON (filename → its
    blocks only) + Copy for AI / Paste translated (modal textarea, plus
    clipboard paste) / Import JSON file…; import shows the per-file error
    list, imports the valid files. **Plus a "Bulk edit" action (his
    amendment 2026-11-17): the full-document editable Translation JSON
    editor is kept, opened from the JSON tab as a modal — the per-image
    read-only view is the default, bulk edit is one click away.**

**Phase 5 — Log + status bar**
15. Structured `LogLine {time, level, file, result}`; log table (newest
    first, zebra rows, auto-scroll off when the user scrolls up); filters
    All / Warnings with counts; Copy log; Stop job here (disabled when
    idle).
16. Status bar: `N images · N rendered · N copied unchanged · N failed`,
    last save path, progress bar ONLY while a job runs (with the
    `accesskit` live region from Phase 3).

**Phase 6 — Workflow state machine + edge cases**
17. `WorkflowStep` derivation (proposed): Scan done once extraction
    exists; Translate done when every kept block has a non-empty
    translation differing from the original; Review is current while
    translations exist and render hasn't completed; Render done when a
    render run finished (0 failed → done, else current with the failed
    count). Stepper items clickable (Scan → run scan; Translate → open
    JSON tab; Review → focus stage; Render → run render). Header primary
    label follows the current step.
18. Empty-image handling: "No text found. This image will be copied
    unchanged." in the inspector; empty images export `{}`.
19. Below 1280 px wide: inspector becomes a right-side overlay drawer
    (toggled from the toolbar) — per §3, pending his confirm.

**Phase 7 — Verification + delivery**
20. `cargo test` (all suites green, incl. re-pinned contrast test +
    natural-sort test); `cargo fmt` on touched files only (repo is not
    fmt-clean — do not reformat the world).
21. His Windows eyeball at 1920×1080 (the gate — no Linux GUI smoke test
    possible here): side-by-side vs `reference/`, states idle / scanning /
    review / rendering / error; keyboard-only pass; contrast check.
22. Deliverables per §12: source (local commits, push only on his word +
    fresh token), build instructions, short CHANGELOG, screenshots of each
    state.

## 4. What does NOT change
- `ocr.rs`, `render.rs`, `layout.rs`, `inpaint.rs`, `image_io.rs`
  (OCR, plate rendering, inpaint, I/O) — UI-only work behind the existing
  thin interface (`scan()`, `render()`, `loadCaptions()`, `saveCaptions()`,
  `importTranslations()`, `applyThresholds()`, `openFolder()`, `stop()`).
- Exchange JSON shape and `captions.json` full-document format (except
  the block-id question below, Q3).
- `SizingMode` values/labels, threshold defaults (.60 / 12), the
  UniformGrow default.

## 5. Decisions — LOCKED 2026-11-17 ("yes to all" + bulk-JSON amendment)

- **Q1 — Stack: A — stay on Rust + eframe/egui.** No rewrite.
- **Q2 — Daylight theme: A — REMOVE it, dark only** per SPEC §4. The
  Phase 3 Midnight/Daylight switch and Appearance section go away.
- **Q3 — Block ids: A — KEEP current OCR-sequence ids.** Renumbering
  among kept would break existing `captions.json` / translated JSONs.
- **Q4 — About button + Appearance section: A — REMOVE both.** The
  mockup has neither. (The VMAX lockup stays in the title bar as the
  brand mark — the About *dialog* goes; the mark itself is the header
  brand per the mockup.)
- **Q5 — Pixel-perfect DoD: A — accept the side-by-side reading.**
  Same layout / tokens / spacing / behaviour, checked by screenshots
  at 1920×1080; a literal 2 px HTML diff is not achievable in egui.
- **Q6 — Minimum window: A — 1280×720**, inspector becomes an overlay
  drawer below that width.
- **Q7 — JSON tab: A — per-image read-only JSON, PLUS a "Bulk edit"
  action** (his amendment): the full-document editable Translation JSON
  editor is kept and opens from the JSON tab as a modal. Default view is
  per-image read-only; bulk edit is one click away.
- **Q8 — Step derivation: A — accept the Phase 6 §17 rules as proposed.**
- **Q9 — Fonts download: A — YES.** Download IBM Plex Mono + Chakra
  Petch Medium 500 (OFL, official repos) into `assets/fonts/`.

## 6. Open questions from SPEC §13 — answered from the repo
- Target stack/runtime: **Rust + eframe/egui 0.37** (in-process backend).
- Existing backend API/CLI: in-process functions + worker threads over
  channels; headless `--ocr-test` exists. No HTTP/CLI boundary to keep.
- Minimum window: proposed 1280×720 (Q6).
- Light theme: SPEC says never; Q2 is the decision.
- File-name scheme: real filenames, natural-sorted; "N of 75.png" is the
  mockup's sample data.
- Settings persist: `~/.caption_replacer.json` (add window size keys).

## 7. Build record — Phase 0 (2026-11-17, branch `feature/redesign-handoff`, commit `2db9d02`)

- Fonts downloaded (google/fonts raw): `IBMPlexMono-Regular.ttf` + OFL,
  `ChakraPetch-Medium.ttf`. The IBM/plex raw path 404'd; google/fonts
  `ofl/ibmplexmono/` is the working source.
- SPEC token set applied to `theme.rs` (measured, not guessed): faint
  5.13:1 canvas / 5.5:1 chrome, dim 7.52/6.83/8.08, ok 11.23 on canvas,
  field_border 3.62 on card / 4.28 on chrome. `ln` measured 1.31:1 on
  card — gutter only, never a control edge (documented in the file).
- Daylight theme, Appearance switch, About dialog + lockup icon removed
  (Q2/Q4). `Settings.theme` retired; old settings files still load.
- Natural sort + test; kept-box stroke/fill now #4ADE80 (tag stays dark
  green — white label text; black-on-ok label is Phase 3 stage work).
- Window: initial 1600×900, min 1280×720.
- Tests 31/31 green (was 32: deleted `light_theme_applies` +
  `theme_ids_round_trip`, added `natural_sort_orders_numbers_by_value`);
  same 6 pre-existing warnings, no new ones.
- **Pushed 2026-11-17** — main `94116a4` + branch `adfb8aa` (his token,
  used once, remote verified token-free, helper cleaned; see §9).


## 8. Build record — Phase 1 (2026-11-17, branch `feature/redesign-handoff`, commit `aff26b9`)

- **Bulk-edit placement — CONFIRMED (his "A", 2026-11-17):** the full
  bulk JSON editor opens as a modal from the JSON tab (Q7 as locked).
- Header 60px: brand | 4-step stepper | one primary. Stepper derivation
  is pure (`derive_workflow_state`) + unit-tested; a copy of the original
  does not count as translated; zero-block Translate is done only after a
  scan (the test caught the vacuous-true bug before commit).
- Sidebar fixed 316px, inspector fixed 380px (both non-resizable;
  `Settings.panel_width` retired). Detected Blocks moved to the inspector;
  log is a full-width 188px row with ACTIVITY + Copy log + Stop.
- "Verify in Preview" is now secondary (defect 4). "Scan again" is NOT
  in the Project card yet — it arrives in Phase 2 with its confirm modal.
- egui 0.37 notes: `Panel` has `exact_size`, not `exact_height` /
  `exact_width`; left/right panels default `resizable(true)`; `Response::
  accessible_name` consumes and returns the Response.
- Tests 32/32 green (+workflow test); same 6 pre-existing warnings.
- **Not visually verified:** the GUI can't run on this VM (missing
  `libxkbcommon-x11`) — the 1920×1080 eyeball is his Windows run, as
  planned. Aria-current on the stepper is an egui limitation (no
  AccessKit equivalent surfaced); the current step is marked by the blue
  border + blue circle + focus ring instead.
- **Pushed 2026-11-17** — main `94116a4` + branch `adfb8aa` (his token,
  used once, remote verified token-free, helper cleaned; see §9).

## 9. Build record — Phase 2 (2026-11-17, branch `feature/redesign-handoff`, commit `64dc1c6`)

- **Pushed 2026-11-17 (his token, used once, remote verified token-free,
  credential helper cleaned):** main `94116a4` + branch
  `feature/redesign-handoff` at `adfb8aa` (Phase 0 + Phase 1 + records).
  The branch keeps being pushed only on his word.
- Sidebar is now three cards (SOURCE & OUTPUT / OCR THRESHOLDS /
  PROJECT) instead of CollapsingHeaders. Folder rows show the path tail
  with a leading "…" + full path on hover + 36px browse buttons; the path
  is set via Browse, not typed (a TextEdit can't show a leading ellipsis).
- Defect 1 fixed by construction: the sizing popup is a `Popup::menu`
  aligned `TOP_START` (opens upward, `BOTTOM_START` fallback) — the old
  ComboBox opened downward over the OCR card. egui's `Popup::menu`
  already renders on the top layer (`Order::Foreground`); direction was
  the defect, not layering.
- Thresholds are real sliders (conf .01 step, length 1–40) with mono
  outputs; drag → 50 ms-debounced instant re-filter (repaint requested
  at 60 ms so it fires after the drag ends); Apply button removed; Reset
  (.60/12) + Inspect boxes kept; re-filter warns when a translated block
  becomes filtered (log + status) and saves settings.
- Scan again is in the Project grid with the §9 confirm modal
  ("Re-scan N images? Your edited translations will be cleared." /
  Re-scan / Keep edits); no edits → scans straight away.
- New `rotate-ccw` Lucide icon (Reset). `resolved_data()` shared by the
  stepper and the Scan-again edit check.
- Tests 33/33 green (+tail-ellipsis test); same 6 pre-existing warnings.
- `cargo fmt` was run once and reformatted 12 files — reverted everywhere
  except the two touched files, whose fmt churn was then removed by
  re-applying the edits to pristine files. Formatting the touched files
  is a Phase 7 step, not a per-phase one.

## 10. Build record — Phase 3 (2026-11-17, branch `feature/redesign-handoff`, commit `f45f0c7`)

- Workspace restructured: toolbar (prev/next chevrons, "N of M", segmented
  Rendered / OCR Boxes / Original with solid accent on the selected button,
  Fit/100% segmented at the right end) + file list (230px) | stage.
- File list: filter box ("Filter N images…"), 44px thumbnails via a lazy
  small-decode cache (4 decodes/frame budget, 128 FIFO cap, undecodable
  files cached as `None` so they aren't retried; budget-exhausted misses
  are NOT cached, so they retry next frame), name (truncated), "N blocks",
  status chip. Chips: rendered (ok) / no text (warn) / failed (error) /
  untranslated (muted) / ready (accent-ink — all translated, not rendered
  yet; the plan's four-chip set doesn't cover that state, so "ready" was
  added and is noted here). Per-file outcomes arrive on a new
  `WorkerMsg::FileRendered` from the render worker; `clear_render_state()`
  drops the summary + both sets on every dataset change and at run start.
- Stage: checkerboard background (canvas base + card cells, 16px),
  wheel zoom around the cursor (`smooth_scroll_delta`, ×1.105/notch,
  clamped 0.05–8×), drag pan, `0` resets to Fit. Wheel/drag exit Fit into
  a manual zoom — Fit is a preset, not a mode. Pan is clamped so at least
  40px of the image stays visible. Zoom/pan are display-only: the preview
  key is untouched (the texture is full-res; no re-render on zoom/pan).
- OCR diagnostic labels (SPEC §5): kept label is now `#n · conf · kept`,
  black text on SPEC ok (11.23:1), outside the box top-left; dropped
  boxes are DASHED (solid kept / dashed filtered) with a matching dashed
  halo, so colour is not the only cue. The dropped label keeps "DROP
  (reason)" (dropped items have no id/conf for the `#n · conf` format).
  The 11px label size is the mockup's display size; the baked-in label
  keeps the existing ∝-image-width sizing so it stays legible at any zoom.
- Click box ↔ block row both ways is preserved through the stage rewrite
  (click uses the pan/scale-aware image rect; the row click sets
  `selected_block`, which re-renders the diagnostic with the Max Blue
  highlight). The old file dropdown + `short_file_names` are deleted; the
  filter box is now always visible in the list column.
- Keyboard nav: Up/Down move within the (filtered) list, Enter focuses the
  stage. Active while nothing has focus, or while the filter has it — the
  filter is the only text field in the Preview tab.
- Two compile errors fixed along the way: `InputState` has no
  `raw_scroll_delta` in egui 0.37 (use `smooth_scroll_delta`); the render
  worker's `outcome` was partially moved by the `Err(e)` arm (restructured
  into one `match &outcome` that returns the `FileOutcome`).
- Tests 35/35 green (was 33/33): +`file_status_covers_all_five_states`,
  +`step_in_names_wraps_and_ignores_an_empty_list`,
  +`clamp_pan_keeps_forty_pixels_visible`; the `short_file_names` test went
  with the dropdown. Same 6 pre-existing warnings, no new ones.
- NOT pushed (branch is 1 commit ahead of origin at `5a5ed3d`); push only
  on his word.

## 11. Build record — Phase 4 (2026-11-17, branch `feature/redesign-handoff`, commit `d6f5fd3`)

- Item 13 — inspector block cards (replaces `show_block_rows`): one card
  per block (kept + manually filtered) and one per dropped item. A block
  card has a title row ("Block #n" + mono conf, green at/above the conf
  threshold else amber, kept/filtered chip, Filter out / Keep toggle), the
  read-only OCR text, an editable translation textarea, and a fit
  indicator. Clicking the title selects the block (the stage highlights
  it); focusing the textarea selects it too. The selected card carries a
  Max Blue border. A dropped card has the "Filtered" title + reason + the
  OCR text + a Keep button.
- Per-block Keep/Filter override: new `Block.kept_override: Option<bool>`
  (`#[serde(default, skip_serializing_if = "Option::is_none")]`, so old
  JSON loads as `None` and the wire shape is unchanged for un-overridden
  blocks) + `Block::kept()`. Filter out sets `Some(false)`; the block
  keeps its translation in the JSON but renders nothing. Keep on a
  dropped item promotes it via `layout::promote_dropped` (rebuilt from
  the raw lines inside the dropped box, id = max + 1, override
  `Some(true)`); the Keep button is disabled with a tooltip when the raw
  lines are gone (old captions.json). The grouping's block construction
  was factored into `block_from_lines` (shared by the grouping and the
  promotion) — the grouping tests still pass.
- `reapply_thresholds` carries overrides over: an overridden block is
  kept as-is (id, translation, override stable) and re-added if the
  thresholds would drop it; the dropped list drops any item inside an
  overridden block's bbox (matched by containment, not text); the final
  blocks are re-sorted by (y, x). The "translated block became filtered"
  warning no longer fires for a manually-filtered block (it stays in
  `blocks`). The status counts kept vs filtered-out blocks.
- Render + diagnostic honour the override: `render_entry` skips filtered
  blocks in the live set and the jobs loop; the diagnostic draws a
  manually-filtered block dashed orange with a `#n · conf · filtered`
  tag (white on dark orange), and the highlight tag names the state.
  The fit bounds were factored into shared helpers
  (`uniform_size_for`, `smallest_size_for`, `fit_bounds_for`) so the
  inspector's fit indicator uses the exact bounds the render uses.
- Counts honour the override: `counts_for` (untranslated), `file_status`
  (a file with no kept blocks reads "no text"), and
  `derive_workflow_state` (Translate counts kept blocks only; an image
  with no kept blocks is "unchanged"). The file-list "N blocks" stays the
  all-blocks count.
- The translation textarea writes back to `extracted` (local clone +
  write-back, since the inspector draws the resolved snapshot) via
  `set_block_text` → `sync_editor_from_extracted` (regenerates the
  compact JSON, clears the render state + preview key). The fit indicator
  fits the effective text (translation, else the original) and reads
  "Fits plate · N lines · M chars" (ok) or "Shrinks to Npx · N lines ·
  M chars" (warning), cached per (file, block, sizing, font, uniform /
  smallest size, text) so a stable card is shaped once.
- Item 14 — JSON tab: per-image read-only JSON (the preview file's blocks
  only, `per_image_json`, syntax-coloured, `interactive(false)`); Copy for
  AI keeps the full-map copy; Paste Translated JSON is a modal (textarea
  + "Paste from clipboard" + Apply/Cancel); Import Translated JSON runs a
  trial `merge_translations` and shows the per-file error list in a modal
  (the valid files import via the editor text; invalid JSON reports
  "isn't valid JSON — nothing was imported"); "Bulk edit…" opens the
  full-document editor as a modal (binds to `editor_text` directly, edits
  live). All three modals join `modal_open` (Escape dismisses). The old
  `paste_clipboard` (direct clipboard → editor) is deleted.
- Corrections to the plan: the fit estimate lives in render.rs, not
  layout.rs (the plan's item 13 said "reuse layout.rs fit estimate") —
  the bounds are now shared helpers in render.rs. "Edits mark the image
  translated" is the editor-text regeneration + preview re-render (there
  is no separate per-image flag; the resolved data is the mark).
- Tests 40/40 green (was 35/35): +2 layout (`promote_dropped_*`),
  +1 render (`filtered_block_renders_nothing_and_diagnostic_marks_it`),
  +1 ui (`filtered_blocks_skip_the_workflow_counts`,
  `per_image_json_shows_one_file_only` — two new tests in the ui module
  plus the filtered case inside `file_status_covers_all_five_states`).
  Same 6 pre-existing warnings, no new ones.
- NOT pushed; push only on his word (a fresh token is needed — the
  remote is verified token-free after the 16:04 push).

## 12. Build record — Phase 5 (2026-11-17, branch `feature/redesign-handoff`, commit `ae522bc`)

- Item 15 — structured log: `LogLine` is now `{time, level, file,
  result}` (time = local HH:MM:SS via new `chrono` dep, `clock` + `std`
  only — `iana-time-zone` was already in the local registry cache, so
  the build stays offline; `Cargo.lock` updated). The table is newest
  first (pure `log_visible`), zebra rows (card fill on even rows,
  transparent on odd), filters All / Warnings with counts (warnings =
  warn + error), Copy log (display order, `HH:MM:SS LEVEL [file] —
  result`), Stop (disabled at idle, as before). Auto-scroll follows the
  newest line until the user scrolls away from the top (`log_follow` +
  `log_len_seen`; the first row scrolls to the top on new lines while
  following). Per-file render outcomes now log a line each (`rendered` /
  `copied unchanged` / `failed`, error level on failure) — that is what
  fills the file column.
- Item 16 — status bar final form: exact 32px. Left: the render counts
  (`N images · N rendered · N copied unchanged · N failed`, pure
  `render_counts`, 0s until a run finishes) or "JSON error" (danger,
  full error on hover) when the editor JSON is broken; then the last save
  path (the output folder after a render, the file after Save captions,
  full path on hover); then the status message (the accesskit polite live
  region, kept). The progress bar shows ONLY while a job runs (defect 6
  — at idle it read as a second status line). The old "N files · N
  untranslated" counts are gone from the bar; the untranslated count
  still lives in the Translate stepper subtitle.
- One compile fix: `ScrollAreaOutput.state.offset` is private in egui
  0.37 and `offset()` is deprecated — `clamped_offset()` is the read.
- Tests 42/42 green (was 40/40): +`log_visible_is_newest_first_and_
  warnings_filter_info`, +`render_counts_reads_the_last_run`. Same
  pre-existing warnings, no new ones.
- NOT pushed; push only on his word (a fresh token is needed — the
  remote is verified token-free after the 16:18 Phase 4 push).

## 13. Build record — Phase 6 (2026-11-17, branch `feature/redesign-handoff`, commit `f26404d`)

- Item 17 — workflow state machine, final: the derivation rules were
  already in place from Phase 1 (Scan done once extracted; Translate
  done when every kept block has a real translation; Review current while
  translations exist and no render has completed; `run_step` clicks =
  Scan → run scan, Translate → JSON tab, Review → Preview tab, Render →
  run render; the header primary follows the current step). What was
  missing was the failed-render rule, now in: Review is done once a
  render was attempted, but Render is done only at 0 failed — a run with
  failures leaves Render current with the failed count in its subtitle
  (`N rendered · M failed`), so the user re-renders instead of moving
  on. (Review → Preview tab is the "focus stage" action: the stage is
  the Preview tab's content and its keyboard nav is active whenever
  nothing else has focus.)
- Item 18 — empty-image handling: the inspector's empty line is now the
  plan's copy, "No text found. This image will be copied unchanged."
  (`verify_text_for`); and `compact_map_from_data` keeps empty images
  as `{}` instead of dropping them, so a file that was seen and is copied
  unchanged stays visible in the AI JSON and the importer instead of
  vanishing. The render worker already copies empty images unchanged
  (no live blocks → `Copied`).
- Item 19 — NOT built: the inspector-as-overlay-drawer below 1280px is
  still pending his confirm (the plan §3 calls for it); it is a separate
  decision, not part of this commit.
- Tests 44/44 green (was 42/42): +`empty_image_verify_text_names_the_
  copy`, +`compact_map_keeps_empty_images`, and the failed-render case
  extends the existing workflow-steps test. Same pre-existing warnings,
  no new ones.
- NOT pushed; push only on his word (a fresh token is needed — the
  remote is verified token-free after the 16:28 Phase 5 push).

## 14. Build record — Phase 7 (2026-11-17, branch `feature/redesign-handoff`, commits `d26a0e7` + `2a7c1c0`)

- Final pass. `cargo test --offline` green: 44/44, same 5 pre-existing
  warnings (test build), no new ones.
- `cargo fmt` on the seven redesign-touched files ONLY
  (`src/icons.rs`, `src/layout.rs`, `src/model.rs`, `src/parser.rs`,
  `src/render.rs`, `src/theme.rs`, `src/ui.rs`) — commit `d26a0e7`,
  formatting only (1,100 + / 561 -). Note: `cargo fmt` on its own
  formats the whole crate (it showed diffs in `build.rs` and
  `src/fonts.rs` too), so the files were formatted by invoking `rustfmt
  --edition 2021` on them directly. The rest of the crate is still not
  fmt-clean and is left alone. Tests re-run after the fmt: still 44/44.
- Deliverables: `CHANGELOG.md` (new — Phases 0–7 + the 1.0.0–1.0.3
  history) and README touch-ups (the Fonts bullet now names the vendored
  UI fonts; the layout table gains `theme.rs`/`icons.rs` and the
  three-column `ui.rs` description) — commit `2a7c1c0`. The README's
  Build section was already accurate for this branch (Windows release
  target, Linux dev/test with the ORT 1.32.0 vendor note) and needed no
  rewrite.
- NOT pushed; push only on his word (a fresh token is needed — the
  remote is verified token-free after the 16:52 Phase 6 push).
- The gate is his Windows 1920×1080 eyeball (side-by-side vs the
  reference, states idle / scanning / review / rendering / error, plus a
  keyboard-only pass). State screenshots could NOT be produced here:
  the Linux GUI needs `libxkbcommon-x11`, which this VM does not have
  and whose apt mirror is unreachable, so only the headless paths were
  verified on Linux (see the README's Linux GUI note). No screenshot in
  this repo is claimed as a visual verification.

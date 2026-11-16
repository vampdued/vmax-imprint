# Caption Studio

Batch image caption extractor, translator and renderer — a complete,
production-ready Rust port of the Python `caption_replacer.py`
(tkinter + RapidOCR + OpenCV + Pillow) into a single standalone Windows
`.exe` with **zero external DLL or model dependencies at runtime**.

## What it does

1. **Scan & Extract Text** — RapidOCR (PP-OCRv4, DBNet detection +
   CTC recognition) finds caption text in every image in a folder.
   All raw detections with confidence ≥ 0.10 are kept in memory, so
   changing the confidence / min-chars thresholds re-filters instantly
   without re-running the neural nets.
2. **Translate** — copy the compact `{file: {block_id: text}}` JSON for
   an AI translator, paste the translated JSON back (fences, `x = {...}`
   assignments and Python-literal dicts are all accepted).
3. **Render Translated Images** — the old captions are inpainted away
   and the new text is rendered on smooth Bezier-rounded plates, with
   cosmic-text shaping for Indic (Tamil, Hindi, Telugu, Kannada,
   Malayalam, Bengali) and CJK (Japanese, Korean, Chinese) scripts.

The UI is the Python app's dual-column layout: folders & font, threshold
tuning, batch workflow and activity log on the left; Translation JSON
editor and Live Preview & OCR Check (Rendered / OCR Boxes / Original)
on the right. Settings live in `~/.caption_replacer.json`, the same file
(and keys) as the Python app, so settings carry over.

## Single-exe packaging

`cargo build --release` on Windows (x86_64-pc-windows-msvc) produces one
`caption-studio.exe`:

- **GUI:** `eframe` + `egui` (+ `egui_extras`).
- **OCR:** `ort` 2.0.0-rc.15 (ONNX Runtime 1.31). The build downloads the
  prebuilt ONNX Runtime distribution once and links it **statically**
  (`cargo:rustc-link-lib=static=onnxruntime`). `copy-dylibs` and
  `load-dynamic` are deliberately *not* enabled, so no `onnxruntime.dll`
  is copied or loaded at runtime.
- **Models:** `ch_PP-OCRv4_det_infer.onnx` (4.6 MB),
  `ch_PP-OCRv4_rec_infer.onnx` (10.4 MB) and `ppocr_keys_v1.txt`
  (6,623 chars) are embedded with `include_bytes!()` / `include_str!()`.
- **Fonts:** rendering uses the system fonts (Segoe UI / Nirmala UI /
  Yu Gothic / Malgun Gothic / Microsoft YaHei on Windows), with a
  DejaVu Sans Bold copy embedded as a last-resort fallback.
- **CRT note:** the ONNX Runtime static lib is built with `/MD`
  (dynamic CRT), so the exe intentionally does **not** use
  `+crt-static`. The UCRT ships with Windows 10/11, so the exe still
  needs no app DLLs of its own.

## Build

### Windows (the release target)

```powershell
# install Rust: https://rustup.rs (stable, MSVC toolchain)
git clone <this repo>
cd caption-studio
.\scripts\build-windows.ps1   # cargo build --release, prints the exe path
# or: cargo build --release
# exe: target\release\caption-studio.exe
```

A GitHub Actions workflow (`.github/workflows/windows-release.yml`)
builds the same exe on every tag push (`v*`) and attaches it as a
release asset. The workflow also runs `cargo test` on Linux first.

### Linux (development / verification)

```sh
cargo test            # unit tests (layout, parser, inpaint, render, OCR geometry)
cargo run -- --version
cargo run -- --ocr-test path/to/image.png   # prints raw OCR lines as JSON, writes ocr-test.json
xvfb-run cargo run    # GUI under a virtual display
```

**Linux ONNX Runtime note.** The `ort` crate downloads its prebuilt
binaries from pyke's CDN (`cdn.pyke.io`) at build time. That CDN returns
HTTP 403 from some networks (observed 2026-11-16 on this VM). The
workaround for Linux dev builds is the official ONNX Runtime release
(1.32.0, API 32 ≥ the `ort` 2.0.0-rc.15 minimum of 31), linked
dynamically:

```sh
# one-time: extract onnxruntime-linux-x64-1.32.0.tgz into vendor/
export ORT_LIB_LOCATION=$PWD/vendor/onnxruntime-linux-x64-1.32.0/lib
export ORT_PREFER_DYNAMIC_LINK=1
cargo check   # or test / build
LD_LIBRARY_PATH=$ORT_LIB_LOCATION ./target/debug/caption-studio --ocr-test img.png
```

`vendor/` is gitignored. This affects Linux dev builds only: the
Windows release build keeps static linking via pyke's CDN (which the
GitHub Actions runner can reach), so the shipped `.exe` still has zero
runtime DLL dependencies. A Linux binary built this way *does* depend on
`libonnxruntime.so` at runtime — that is expected and Linux-only.

**Linux GUI note.** Running the GUI on Linux needs the system library
`libxkbcommon-x11` (winit's X11 backend). This VM does not have it and
its apt mirror is unreachable, so the GUI could not be smoke-tested
here — only the headless paths (`--version`, `--ocr-test`, unit tests)
were verified on Linux. Windows needs no such library.

## Project layout

| File | Contents |
|---|---|
| `src/main.rs` | Entry point, `--version` / `--ocr-test` helpers, no-console on Windows release |
| `src/model.rs` | `captions.json` data model (wire-compatible with Python), settings |
| `src/ocr.rs` | RapidOCR pipeline: DBNet thresholding, min-area boxes, Vatti unclip, perspective-warp recognition, CTC greedy decode, 1600px downscale |
| `src/layout.rs` | Colour sampling (14% ring, 92nd-percentile text colour), block grouping & threshold filtering |
| `src/inpaint.rs` | Mask dilation + diffusion inpainting over the masked ROI |
| `src/render.rs` | `fit_text_to_box` (fast path + binary search), smooth caption plates (tiny-skia), `render_entry`, OCR diagnostic overlay |
| `src/parser.rs` | Flexible JSON / Python-dict parsing, translation merging |
| `src/fonts.rs` | cosmic-text font system, script font chains, custom font loading |
| `src/image_io.rs` | Unicode-safe image load/save (JPEG q95, WebP q95) |
| `src/ui.rs` | eframe/egui dual-column UI, scan/render workers, live preview |
| `assets/models/` | Embedded ONNX models + dictionary (see `assets/models/README.md`) |

## Behaviour ported from Python (exact)

- `MAX_OCR_DIM = 1600` downscale, coordinates mapped back to native res.
- Colour sampling: 14% outer ring (≤1,500 px), median bg, `is_boxed =
  bg_std < 34.0`, dark boxed bg snaps to `[18, 18, 18]`; text colour =
  median of inner pixels ≥ 92nd percentile distance from bg (≤2,000 px).
- Grouping: `score >= min_conf` (dropped lines recorded as
  `"conf X.XX < Y.YY"`), sort top-to-bottom/left-to-right, merge when
  `color_dist < 75` and same-row (overlap > 0.45·avg_h, gap < 3·avg_h)
  or stacked (gap −0.3…0.85·avg_h, overlap > −1.5·avg_h); unboxed blocks
  under `min_chars` dropped as `"unboxed N < M chars"`.
- Rendering: `start_size = max(16, avg_h·1.05)`, `max_w =
  min(w−56, max(0.86·w if wide else box_w, box_w))`,
  `max_h = max(1.3·box_h, 1.5·avg_h)`, fast-path fit then binary search
  13…start−1, fallback 12; pads `pad_x = max(14, 0.45·size)`,
  `pad_y = max(7, 0.22·size)`; anchor top if `y1 < 0.35h`, bottom if
  `y2 > 0.65h`; boxed plates use the old box padded by
  `max(36, 0.95·avg_h)` / `max(20, 0.60·avg_h)` for inpainting, unboxed
  text masks pixels within RGB distance 90 of the text colour; plate
  fill alpha 232, radius `max(10, 0.32·size)`, unboxed stroke width 2.
- Batch render: up to 8 workers (rayon), unchanged blocks are copied,
  not re-rendered; `captions.json` is written next to the input images.

## Deliberate deviations (no OpenCV in a single Rust exe)

- **Inpainting:** `cv2.inpaint(TELEA)` is replaced by iterative
  diffusion (Laplace) inpainting over the same ROI with the same 5×5
  dilation. On the flat/graded caption plates this removes, the result
  is visually identical; on busy photos it is a smooth blur rather
  than Telea's edge-following fill.
- **OCR downscale filter:** Python used OpenCV `INTER_AREA`; Rust uses
  the `image` crate's triangle (bilinear) filter for the 1600px and
  960px resizes.
- **Font measurement:** PIL `textbbox` metrics are replaced by
  cosmic-text layout metrics (ascent+descent of `Agjpqy`, per-line
  widths from shaped runs). Positions match to within a few pixels.
- **CTC dictionary:** the embedded `ppocr_keys_v1.txt` is the PaddleOCR
  release/2.8 file (6,623 lines). The rec model outputs **6,625 classes**
  whose rows are already softmax probabilities: blank=0, character `i` =
  `dict[i-1]` for `i` in 1..=6623, and class 6624 = space. All three facts
  were verified empirically against the model (2026-11-16).
- **WebP output:** the `image` crate 0.25 only supports *lossless* WebP
  encoding, so Rust saves WebP as lossless instead of Python's q95
  lossy. Files are larger; visual quality is equal or better.

## Model provenance

- Detection/recognition: [SWHL/RapidOCR](https://huggingface.co/SWHL/RapidOCR)
  (`PP-OCRv4/`), ONNX conversions of PaddleOCR models, Apache-2.0.
  SHA-256 verified (see `assets/models/README.md`).
- Dictionary: [PaddlePaddle/PaddleOCR](https://github.com/PaddlePaddle/PaddleOCR)
  `release/2.8`, `ppocr/utils/ppocr_keys_v1.txt`, Apache-2.0.
- Embedded fallback font: DejaVu Sans Bold (Bitstream Vera licence,
  see `assets/fonts/`).

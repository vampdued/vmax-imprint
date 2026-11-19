# VMAX Imprint

[![CI](https://github.com/vampdued/vmax-imprint/actions/workflows/ci.yml/badge.svg)](https://github.com/vampdued/vmax-imprint/actions/workflows/ci.yml)
[![GitHub Release](https://img.shields.io/github/v/release/vampdued/vmax-imprint?color=blue&label=release)](https://github.com/vampdued/vmax-imprint/releases/latest)
[![Platform](https://img.shields.io/badge/platform-Windows%20x64-lightgrey.svg)](https://github.com/vampdued/vmax-imprint/releases/latest)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE)

**VMAX Imprint** is a fast, standalone desktop application for batch image text extraction, translation, and rendering. It detects on-image captions, provides an interactive editor for translation workflows, cleanly inpaints old text backgrounds, and renders new text with subpixel typographic precision.

It ships as a **single standalone Windows `.exe`** with **zero external dependencies** — the ONNX Runtime engine, RapidOCR (PP-OCRv4) neural models, font engines, and vector renderers are fully bundled inside the executable.

---

## Key Features

- **Embedded Neural OCR**: Integrated RapidOCR (PP-OCRv4 detection and CTC recognition) runs entirely locally via embedded ONNX models. Adjust confidence and character thresholds in real time without re-running models.
- **3-Stage Workflow**:
  1. **Scan**: Ingest folders of images (`JPG`, `PNG`, `WEBP`, `BMP`), detect text blocks, inspect bounding boxes, and adjust filter thresholds.
  2. **Translate**: View and edit translations directly. Copy structured JSON to clipboard for AI translators (e.g. ChatGPT, Claude), or paste translated JSON back with automatic format parsing.
  3. **Review & Render**: Telea-style inpainting clears the original background; smooth subpixel Bezier plates render the new translated text with customizable font sizing modes.
- **Advanced Typographic Shaping**: Powered by `cosmic-text` and `tiny-skia` with full shaping support for Latin, CJK (Chinese, Japanese, Korean), and complex Indic scripts (Tamil, Hindi, Telugu, Kannada, Malayalam, Bengali).
- **Interactive Stage Canvas**: Full zoom (`Fit view`, `100%`, incremental zoom up to 800%), smooth panning, checkerboard background, and toggleable preview modes (`Rendered`, `OCR Boxes`, `Original`).
- **Completely Self-Contained**: No Python environment, no CUDA toolkit, and no external runtime DLLs required.

---

## Download

Prebuilt standalone binaries are available on the [Releases](https://github.com/vampdued/vmax-imprint/releases/latest) page.

1. Download **`vmax-imprint.exe`** (or the `.zip` package).
2. Run `vmax-imprint.exe` directly on Windows 10 or 11 (64-bit). No installation needed.

---

## Quickstart

### 1. Scan Images
- Click **Select input folder** or drag and drop images directly into the window.
- Click **Scan images**. RapidOCR will process each image and display the detected text blocks.
- Fine-tune **Min confidence** and **Min characters** sliders in the sidebar if needed.

### 2. Translate
- Switch to the **Translate** tab.
- Click **Copy JSON** to copy all detected captions in a compact format for your translation workflow.
- Translate the text and click **Paste JSON** to import your translations, or edit blocks individually in the inspector panel.

### 3. Review & Render
- Switch to the **Render** tab to preview the rendered image side-by-side with the original.
- Choose your **Font sizing** mode:
  - `Adaptive shrink`: Automatically fits text into the original box bounding size.
  - `Uniform step`: Scales all captions uniformly.
  - `Fixed pt`: Uses an exact point size across all blocks.
- Click **Render all images** to save the output files.

---

## Command-Line Interface (CLI)

In addition to the graphical interface, `vmax-imprint` can be invoked from the command line:

```powershell
# Launch the graphical application
vmax-imprint.exe

# Run OCR diagnostics on a single image and output detected lines as JSON
vmax-imprint.exe --ocr-test path\to\image.png

# Display version information
vmax-imprint.exe --version

# Display command-line help
vmax-imprint.exe --help
```

---

## Building from Source

### Prerequisites
- [Rust toolchain](https://rustup.rs) (stable, MSVC target `x86_64-pc-windows-msvc`).
- Windows 10 or 11 with the standard MSVC C++ build tools.

### Build Steps

```powershell
# Clone the repository
git clone https://github.com/vampdued/vmax-imprint.git
cd vmax-imprint

# Run tests
cargo test

# Build optimized release executable
cargo build --release
```

The resulting standalone executable will be located at:
```text
target\release\vmax-imprint.exe
```

---

## Architecture & Core Crates

| Component | Implementation |
| :--- | :--- |
| **GUI Framework** | [`eframe`](https://github.com/emilk/egui) / [`egui`](https://github.com/emilk/egui) v0.37 |
| **Neural Inference Engine** | [`ort`](https://github.com/pykeio/ort) statically linking ONNX Runtime |
| **OCR Models** | Embedded PP-OCRv4 (DBNet detection + CTC recognition) |
| **Text Shaping** | [`cosmic-text`](https://github.com/pop-os/cosmic-text) + [`fontdb`](https://github.com/RazrFalcon/fontdb) |
| **2D Vector Rendering** | [`tiny-skia`](https://github.com/RazrFalcon/tiny-skia) |
| **Image Processing & IO** | [`image`](https://github.com/image-rs/image) + custom Telea-style inpainter |

---

## License

This project is licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

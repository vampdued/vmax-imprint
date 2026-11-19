# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.0.0] — 2026-11-19

### Initial Public Release of VMAX Imprint

- **Complete Rust Desktop App**: Full standalone port of image caption extraction, editing, and rendering into a high-performance Windows native executable.
- **Embedded RapidOCR Engine**: Embedded PP-OCRv4 ONNX neural detection and recognition with static ONNX Runtime linkage (zero external DLL or Python requirements).
- **Interactive 3-Stage Workflow**:
  - **Stage 1 (Scan)**: Directory batch processing, instant debounced threshold filtering (confidence & minimum character length), and OCR bounding box inspection.
  - **Stage 2 (Translate)**: Built-in dual-mode JSON editor (per-image and batch modes), clipboard format parser for AI translation workflows, and direct in-place translation editing.
  - **Stage 3 (Review & Render)**: Subpixel anti-aliased Bezier-rounded caption plates, separable Telea-style inpainter, interactive zoom/pan canvas, and batch image export.
- **Advanced Text Shaping**: Full complex Indic and CJK script shaping support via `cosmic-text` and `fontdb`.
- **Performance & Caching**: Dirty-checked per-frame UI caches and zero-allocation preview hashing for 60 FPS interaction.
- **Standalone Single-Executable Packaging**: Bundled fonts, neural models, vector renderer, and GUI engine into a single `vmax-imprint.exe`.

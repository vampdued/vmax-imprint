//! Caption Studio — batch image caption extractor, translator, renderer.
//!
//! Rust port of `caption_replacer.py`: RapidOCR (PP-OCRv4, embedded ONNX)
//! extracts caption text, the user translates the compact JSON (e.g. with
//! an AI), and the app inpaints the old captions away and renders the new
//! text on smooth Bezier-rounded plates.
//!
//! Run the GUI with no arguments. Headless helpers:
//!   caption-studio --version
//!   caption-studio --ocr-test <image>   (prints raw OCR lines as JSON to
//!                                        stdout and writes ocr-test.json)
//!   caption-studio --shot-test <input> <output> [--json <file>]
//!                                        (drives the app through every
//!                                        reviewable state, saving a PNG per
//!                                        state into <output>)

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod fonts;
mod icons;
mod image_io;
mod inpaint;
mod layout;
mod model;
mod ocr;
mod parser;
mod render;
// Design-system scale (theme.rs): a library surface ported from
// craft-anything. Most of its constants/helpers are not used yet by this
// small app; that is intentional, not dead code to strip.
#[allow(dead_code)]
mod theme;
mod ui;

use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("--version") => {
            println!("caption-studio {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("--help") | Some("-h") => {
            println!("VMAX Imprint — batch image text extractor, translator & renderer");
            println!();
            println!("Usage:");
            println!("  caption-studio                 Launch the GUI");
            println!("  caption-studio --ocr-test <image>  Run OCR on one image, print raw lines as JSON");
            println!("  caption-studio --shot-test <input> <output> [--json <file>]");
            println!("      Drive the app through every reviewable state, saving a PNG per state into <output>");
            println!("  caption-studio --version       Print version");
            Ok(())
        }
        Some("--ocr-test") => {
            let path = args
                .get(2)
                .map(PathBuf::from)
                .ok_or_else(|| anyhow::anyhow!("--ocr-test needs an image path"))?;
            let img = image_io::load_image(&path)?;
            let engine = ocr::OcrEngine::new()?;
            let lines = engine.recognize_image(&img)?;
            let json = serde_json::to_string_pretty(&lines)?;
            println!("{json}");
            let out = PathBuf::from("ocr-test.json");
            std::fs::write(&out, &json)?;
            eprintln!("wrote {}", out.display());
            Ok(())
        }
        #[cfg(feature = "shot-test")]
        Some("--shot-test") => {
            let input = args
                .get(2)
                .map(PathBuf::from)
                .ok_or_else(|| anyhow::anyhow!("--shot-test needs an input folder"))?;
            let output = args
                .get(3)
                .map(PathBuf::from)
                .ok_or_else(|| anyhow::anyhow!("--shot-test needs an output folder"))?;
            let mut json = None;
            let mut i = 4;
            while i < args.len() {
                match args[i].as_str() {
                    "--json" => {
                        json = args.get(i + 1).map(PathBuf::from);
                        i += 2;
                    }
                    other => {
                        anyhow::bail!("--shot-test: unknown argument: {other}");
                    }
                }
            }
            if json.is_none() && args.len() > 4 {
                anyhow::bail!("--shot-test: --json needs a file");
            }
            ui::run_shot_test(input, output, json)
        }
        #[cfg(not(feature = "shot-test"))]
        Some("--shot-test") => {
            anyhow::bail!("--shot-test requires building with `--features shot-test`")
        }
        Some(other) => {
            eprintln!("unknown argument: {other} (see --help)");
            std::process::exit(2);
        }
        None => ui::run_app().map_err(|e| anyhow::anyhow!("{e}")),
    }
}

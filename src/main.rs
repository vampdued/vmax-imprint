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

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod fonts;
mod image_io;
mod inpaint;
mod layout;
mod model;
mod ocr;
mod parser;
mod render;
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
            println!("Caption Studio — batch image caption extractor, translator & renderer");
            println!();
            println!("Usage:");
            println!("  caption-studio                 Launch the GUI");
            println!("  caption-studio --ocr-test <image>  Run OCR on one image, print raw lines as JSON");
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
        Some(other) => {
            eprintln!("unknown argument: {other} (see --help)");
            std::process::exit(2);
        }
        None => ui::run_app().map_err(|e| anyhow::anyhow!("{e}")),
    }
}

// Build script: rebuild when the embedded models / dictionary change.
fn main() {
    println!("cargo:rerun-if-changed=assets/models/ch_PP-OCRv4_det_infer.onnx");
    println!("cargo:rerun-if-changed=assets/models/ch_PP-OCRv4_rec_infer.onnx");
    println!("cargo:rerun-if-changed=assets/models/ppocr_keys_v1.txt");
    println!("cargo:rerun-if-changed=assets/fonts/DejaVuSans-Bold.ttf");
    println!("cargo:rerun-if-changed=assets/vmax-icon.ico");

    // Windows exe icon (VMAX black tile). Only when targeting Windows —
    // the Linux test/dev builds never invoke winres, so no rc.exe is ever
    // needed there. The Windows release workflow builds on a Windows
    // runner, where winres finds rc.exe via the MSVC toolchain.
    let target_windows =
        std::env::var("CARGO_CFG_TARGET_OS").map(|v| v == "windows").unwrap_or(false);
    if target_windows {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/vmax-icon.ico");
        res.compile().expect("winres: embed vmax-icon.ico");
    }
}

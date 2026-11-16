// Build script: rebuild when the embedded models / dictionary change.
fn main() {
    println!("cargo:rerun-if-changed=assets/models/ch_PP-OCRv4_det_infer.onnx");
    println!("cargo:rerun-if-changed=assets/models/ch_PP-OCRv4_rec_infer.onnx");
    println!("cargo:rerun-if-changed=assets/models/ppocr_keys_v1.txt");
    println!("cargo:rerun-if-changed=assets/fonts/DejaVuSans-Bold.ttf");
}

# OCR model provenance
# ----------------------
# Detection and recognition models are the ONNX conversions of PaddleOCR
# PP-OCRv4 published by RapidAI / SWHL, Apache-2.0:
#   https://huggingface.co/SWHL/RapidOCR (PP-OCRv4/)
# Dictionary is PaddleOCR release/2.8 ppocr/utils/ppocr_keys_v1.txt.
#
# Verified SHA-256 (2026-11-16):
#   ch_PP-OCRv4_det_infer.onnx  d2a7720d45a54257208b1e13e36a8479894cb74155a5efe29462512d42f49da9  (4,745,517 bytes)
#   ch_PP-OCRv4_rec_infer.onnx  48fc40f24f6d2a207a2b1091d3437eb3cc3eb6b676dc3ef9c37384005483683b  (10,857,958 bytes)
#   ppocr_keys_v1.txt            a1c84d9bdb9ab29043c58896224d32941783eb821629618416dcb08f12886492  (6,623 lines)
#
# Re-download:
#   curl -L -o ch_PP-OCRv4_det_infer.onnx https://huggingface.co/SWHL/RapidOCR/resolve/main/PP-OCRv4/ch_PP-OCRv4_det_infer.onnx
#   curl -L -o ch_PP-OCRv4_rec_infer.onnx https://huggingface.co/SWHL/RapidOCR/resolve/main/PP-OCRv4/ch_PP-OCRv4_rec_infer.onnx
#   curl -L -o ppocr_keys_v1.txt https://raw.githubusercontent.com/PaddlePaddle/PaddleOCR/release/2.8/ppocr/utils/ppocr_keys_v1.txt

# Modelli OCR

`text-detection.onnx` e `text-recognition.onnx` sono i modelli pre-addestrati di ocrs, incorporati
in ToolsPDF.exe (`include_bytes!` in `src/tools/form_text.rs`) senza modifiche.

Ocrs models (https://github.com/robertknight/ocrs-models), trained on the HierText dataset by
Google Research. Licensed under CC BY-SA 4.0 (https://creativecommons.org/licenses/by-sa/4.0/).

Il codice di ocrs e rten e' sotto licenza MIT o Apache-2.0, a scelta.

# Modello delle foto tessera

`u2netp.onnx` e' il modello U-2-Net piccolo (u2netp), incorporato in ToolsPDF.exe
(`include_bytes!` in `src/tools/portrait.rs`) senza modifiche; esportato in ONNX dal progetto rembg
(https://github.com/danielgatis/rembg/releases/download/v0.0.0/u2netp.onnx).

U-2-Net: Going Deeper with Nested U-Structure for Salient Object Detection, Xuebin Qin, Zichen Zhang,
Chenyang Huang, Masood Dehghan, Osmar R. Zaiane, Martin Jagersand (https://github.com/xuebinqin/U-2-Net).
Licensed under the Apache License 2.0.

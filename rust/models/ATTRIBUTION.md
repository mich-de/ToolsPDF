# Modelli OCR

`text-detection.onnx` e `text-recognition.onnx` sono i modelli pre-addestrati di ocrs, incorporati
in ToolsPDF.exe (`include_bytes!` in `src/tools/form_text.rs`) senza modifiche.

Ocrs models (https://github.com/robertknight/ocrs-models), trained on the HierText dataset by
Google Research. Licensed under CC BY-SA 4.0 (https://creativecommons.org/licenses/by-sa/4.0/).

Il codice di ocrs e rten e' sotto licenza MIT o Apache-2.0, a scelta.

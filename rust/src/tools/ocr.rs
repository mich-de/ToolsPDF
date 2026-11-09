//! Riconoscimento del testo con l'OCR integrato in Windows: nessun file o modello da portarsi dietro.

use std::sync::OnceLock;

use image::RgbImage;
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Security::Cryptography::CryptographicBuffer;
use windows::Win32::System::Com::CoIncrementMTAUsage;

use super::text_layer::Word;

const MAX_DIMENSION: u32 = 10000;

/// Vero se Windows ha almeno una lingua con riconoscimento del testo.
pub fn available() -> bool {
    in_com_thread(|| OcrEngine::TryCreateFromUserProfileLanguages().is_ok()).unwrap_or(false)
}

/// Parole trovate, con coordinate normalizzate da 0 a 1 rispetto all'immagine.
pub fn recognize(image: &RgbImage) -> Result<Vec<Word>, String> {
    in_com_thread(|| recognize_inner(image)).unwrap_or_else(|| Err("OCR non avviabile".into()))
}

/// L'OCR gira in un thread MTA: il worker resta STA per i dialoghi di salvataggio.
/// L'MTA resta vivo fino all'uscita: la libreria tiene in cache gli oggetti WinRT e, se
/// venisse chiuso, quei riferimenti punterebbero a una DLL gia' scaricata.
fn in_com_thread<T: Send>(action: impl FnOnce() -> T + Send) -> Option<T> {
    static MTA_READY: OnceLock<bool> = OnceLock::new();
    let ready = *MTA_READY.get_or_init(|| unsafe { CoIncrementMTAUsage() }.is_ok());
    if !ready {
        return None;
    }
    std::thread::scope(|scope| scope.spawn(action).join().ok())
}

fn recognize_inner(image: &RgbImage) -> Result<Vec<Word>, String> {
    let (width, height) = image.dimensions();
    if width.max(height) > MAX_DIMENSION {
        return Err("pagina troppo grande per l'OCR".into());
    }
    let describe = |e: windows::core::Error| e.message();

    let engine = OcrEngine::TryCreateFromUserProfileLanguages().map_err(describe)?;
    let mut bgra = Vec::with_capacity(width as usize * height as usize * 4);
    for pixel in image.pixels() {
        bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
    }
    let buffer = CryptographicBuffer::CreateFromByteArray(&bgra).map_err(describe)?;
    let bitmap = SoftwareBitmap::CreateWithAlpha(BitmapPixelFormat::Bgra8, width as i32, height as i32, BitmapAlphaMode::Premultiplied)
        .map_err(describe)?;
    bitmap.CopyFromBuffer(&buffer).map_err(describe)?;

    let result = engine.RecognizeAsync(&bitmap).map_err(describe)?.join().map_err(describe)?;
    let mut words = Vec::new();
    for line in result.Lines().map_err(describe)? {
        for word in line.Words().map_err(describe)? {
            let text = word.Text().map_err(describe)?.to_string();
            let rect = word.BoundingRect().map_err(describe)?;
            if text.trim().is_empty() || rect.Width <= 0.0 || rect.Height <= 0.0 {
                continue;
            }
            words.push(Word {
                text,
                x: rect.X / width as f32,
                y: rect.Y / height as f32,
                w: rect.Width / width as f32,
                h: rect.Height / height as f32,
            });
        }
    }
    Ok(words)
}

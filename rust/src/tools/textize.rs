//! Zona Testo: rende selezionabili le scansioni, restando nel limite di peso.

use lopdf::Document;

use super::keep_text::{self, ShrinkError};
use super::raster::{self, render_rgb, OCR_DPI};
use super::text_layer::{add_layers, Word, FONT_NAME};
use super::{imaging, limit, limit_label, ocr, Callbacks, Error, Progress};

const NO_OCR: &str = "OCR non disponibile: aggiungi in Windows una lingua con riconoscimento del testo";

pub fn from_image(data: &[u8], cb: Callbacks) -> Result<(Vec<u8>, String), Error> {
    if !ocr::available() {
        return Err(Error::msg(NO_OCR));
    }
    (cb.progress)(0.1, "Riconoscimento testo…");
    let image = imaging::decode_rgb(data)?;
    let words = ocr::recognize(&image).map_err(|e| Error::msg(format!("OCR non riuscito: {e}")))?;
    drop(image);

    let reserve = words.len() * 16 + 4096;
    let count = words.len();
    let plain = imaging::compress_image(data, reserve, cb.progress)?;
    let out = raster::with_text_layer(&plain, std::iter::once(words))?;
    if out.len() > limit() {
        return Err(Error::msg(format!("impossibile scendere sotto {} con il testo", limit_label())));
    }
    Ok((out, recognized_note(count)))
}

fn recognized_note(words: usize) -> String {
    match words {
        0 => "nessun testo trovato".to_string(),
        n => format!("{n} parole riconosciute"),
    }
}

/// PDF con il testo OCR sulle pagine che non ne avevano, ancora senza limite di peso.
pub struct WithText {
    pub bytes: Vec<u8>,
    pub note: String,
    /// Almeno una pagina aveva gia' testo proprio.
    pub any_native: bool,
}

pub fn from_pdf(data: Vec<u8>, cb: Callbacks) -> Result<(Vec<u8>, String), Error> {
    let WithText { bytes: with_text, note, any_native } = add_text(&data, cb.progress)?;
    if with_text.len() <= limit() {
        return Ok((with_text, note));
    }

    (cb.progress)(0.6, &format!("Riduzione a {}…", limit_label()));
    let kept = match keep_text::shrink(&with_text, cb.progress) {
        Ok(out) if out.len() <= limit() => return Ok((out, note)),
        Ok(out) => out.len() as u64,
        Err(ShrinkError::Password) => return Err(Error::msg("PDF protetto da password")),
        Err(ShrinkError::Other) => 0,
    };

    // Le pagine di sole immagini non perdono nulla: la conferma serve solo per il testo originale.
    if any_native && !(cb.confirm)(kept) {
        return Err(Error::Cancelled);
    }
    let out = raster::rasterize(data, true, cb.progress)?;
    let note = out.note();
    Ok((out.bytes, note))
}

/// Vero se almeno una pagina ha testo vero, non solo lo strato OCR aggiunto da questa app.
pub fn has_original_text(data: &[u8]) -> Result<bool, Error> {
    let doc = Document::load_mem(data).map_err(|e| keep_text::load_error(&e))?;
    if doc.is_encrypted() {
        return Err(Error::msg("PDF protetto da password"));
    }
    let original = |font: &&lopdf::Dictionary| !matches!(font.get(b"BaseFont"), Ok(lopdf::Object::Name(name)) if name == FONT_NAME.as_bytes());
    Ok(doc.get_pages().values().any(|&id| doc.get_page_fonts(id).is_ok_and(|fonts| fonts.values().any(|f| original(f)))))
}

/// Riconosce il testo delle pagine senza testo proprio e lo aggiunge invisibile.
pub fn add_text(data: &[u8], progress: Progress) -> Result<WithText, Error> {
    if !ocr::available() {
        return Err(Error::msg(NO_OCR));
    }
    (progress)(0.03, "Lettura PDF…");
    let mut doc = Document::load_mem(data).map_err(|e| keep_text::load_error(&e))?;
    if doc.is_encrypted() {
        return Err(Error::msg("PDF protetto da password"));
    }
    let page_ids: Vec<_> = doc.get_pages().values().copied().collect();
    let has_native_text: Vec<bool> = page_ids
        .iter()
        .map(|&id| doc.get_page_fonts(id).is_ok_and(|fonts| !fonts.is_empty()))
        .collect();
    let any_native = has_native_text.iter().any(|&native| native);

    let pdf = raster::open(data.to_vec())?;
    let total = pdf.pages().len();
    if total != page_ids.len() {
        return Err(Error::msg("PDF danneggiato o non leggibile"));
    }
    let mut layers: Vec<(lopdf::ObjectId, Vec<Word>)> = Vec::new();
    for (i, page) in pdf.pages().iter().enumerate() {
        if has_native_text[i] {
            continue;
        }
        (progress)(0.05 + 0.5 * i as f64 / total as f64, &format!("Riconoscimento testo: pagina {} di {total}…", i + 1));
        let words = ocr::recognize(&render_rgb(page, OCR_DPI))
            .map_err(|e| Error::msg(format!("OCR non riuscito: {e}")))?;
        layers.push((page_ids[i], words));
    }
    let words_found: usize = layers.iter().map(|(_, words)| words.len()).sum();
    add_layers(&mut doc, &layers).map_err(Error::msg)?;
    let mut with_text = Vec::new();
    doc.save_modern(&mut with_text).map_err(|e| Error::msg(format!("scrittura del testo non riuscita: {e}")))?;
    drop(doc);

    let note = if layers.is_empty() { "testo già presente".to_string() } else { recognized_note(words_found) };
    Ok(WithText { bytes: with_text, note, any_native })
}

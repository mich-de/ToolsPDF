//! Zona Dividi: un PDF per pagina, ognuno nel limite di peso.

use lopdf::{Document, Object, ObjectId};

use super::keep_text::{self, ShrinkError};
use super::{limit, raster, Callbacks, Error};

/// Voci del catalogo che puntano a tutte le pagine: terrebbero nel file l'intero documento.
pub(super) const DOCUMENT_WIDE: [&[u8]; 9] = [
    b"Outlines", b"Dests", b"Names", b"PageLabels", b"StructTreeRoot", b"OpenAction", b"AcroForm", b"Threads", b"Metadata",
];

/// Restituisce le pagine e quante sono state convertite in immagini. Tutto resta in memoria:
/// se l'utente rifiuta una conversione non viene creato nessun file.
pub fn split(data: &[u8], cb: Callbacks) -> Result<(Vec<Vec<u8>>, usize), Error> {
    (cb.progress)(0.02, "Lettura PDF…");
    let doc = Document::load_mem(data).map_err(|e| keep_text::load_error(&e))?;
    if doc.is_encrypted() {
        return Err(Error::msg("PDF protetto da password"));
    }
    let numbers: Vec<u32> = doc.get_pages().keys().copied().collect();
    let total = numbers.len();
    if total == 0 {
        return Err(Error::msg("il PDF non ha pagine"));
    }

    let no_progress = |_: f64, _: &str| {};
    let mut pages = Vec::with_capacity(total);
    let mut converted = 0;
    let mut conversion_allowed = false;
    for (i, &number) in numbers.iter().enumerate() {
        (cb.progress)(0.05 + 0.9 * i as f64 / total as f64, &format!("Pagina {} di {total}…", i + 1));
        let single = single_page(&doc, number, &numbers)?;
        if single.len() <= limit() {
            pages.push(single);
            continue;
        }
        let kept = match keep_text::shrink(&single, &no_progress) {
            Ok(out) if out.len() <= limit() => {
                pages.push(out);
                continue;
            }
            Ok(out) => out.len() as u64,
            Err(ShrinkError::Password) => return Err(Error::msg("PDF protetto da password")),
            Err(ShrinkError::Other) => 0,
        };
        // Una sola domanda per documento, anche se le pagine pesanti sono molte.
        if !conversion_allowed {
            if !(cb.confirm)(kept) {
                return Err(Error::Cancelled);
            }
            conversion_allowed = true;
        }
        pages.push(raster::rasterize(single, true, &no_progress)?.bytes);
        converted += 1;
    }
    Ok((pages, converted))
}

fn single_page(doc: &Document, keep: u32, all: &[u32]) -> Result<Vec<u8>, Error> {
    let mut copy = doc.clone();
    let others: Vec<u32> = all.iter().copied().filter(|&n| n != keep).collect();
    let page_id = copy.get_pages().get(&keep).copied();
    if let Ok(catalog) = copy.catalog_mut() {
        for key in DOCUMENT_WIDE {
            catalog.remove(key);
        }
    }
    if let Some(id) = page_id {
        drop_internal_links(&mut copy, id);
    }
    copy.delete_pages(&others);
    copy.prune_objects();
    copy.compress();
    let mut out = Vec::new();
    copy.save_modern(&mut out).map_err(|e| Error::msg(format!("pagina {keep} non scrivibile: {e}")))?;
    Ok(out)
}

/// I collegamenti verso altre pagine terrebbero vive pagine che qui non esistono piu'.
pub(super) fn drop_internal_links(doc: &mut Document, page: ObjectId) {
    let Ok(dict) = doc.get_dictionary(page) else { return };
    let Ok(annots) = dict.get(b"Annots") else { return };
    let Ok((_, Object::Array(items))) = doc.dereference(annots) else { return };
    let kept: Vec<Object> = items.iter().filter(|item| !is_internal_link(doc, item)).cloned().collect();
    if let Ok(dict) = doc.get_dictionary_mut(page) {
        dict.set("Annots", Object::Array(kept));
    }
}

fn is_internal_link(doc: &Document, item: &Object) -> bool {
    let Ok((_, Object::Dictionary(annot))) = doc.dereference(item) else { return false };
    if !matches!(annot.get(b"Subtype"), Ok(Object::Name(name)) if name == b"Link") {
        return false;
    }
    if annot.has(b"Dest") {
        return true;
    }
    let Ok(action) = annot.get(b"A") else { return false };
    let Ok((_, Object::Dictionary(action))) = doc.dereference(action) else { return false };
    !matches!(action.get(b"S"), Ok(Object::Name(name)) if name == b"URI")
}

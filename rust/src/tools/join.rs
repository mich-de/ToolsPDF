//! Zona Unisci: piu' documenti in un solo PDF, nell'ordine dato.

use std::collections::BTreeMap;

use lopdf::{dictionary, Document, Object, ObjectId};

use super::text_layer::inherited;
use super::{imaging, office, Callbacks, Destination, Error, Kind, Loaded};

/// Attributi che una pagina puo' ereditare dai nodi che qui vengono scartati.
const INHERITABLE: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

pub fn merge(files: &[Loaded], dest: &Destination, cb: Callbacks) -> Result<Vec<u8>, Error> {
    let no_progress = |_: f64, _: &str| {};
    let mut merged = Document::with_version("1.5");
    let mut objects: BTreeMap<ObjectId, Object> = BTreeMap::new();
    let mut kids: Vec<Object> = Vec::new();
    let mut next_id = 1;

    for (i, file) in files.iter().enumerate() {
        (cb.progress)(0.02 + 0.5 * i as f64 / files.len() as f64, &format!("Lettura di {}…", file.name));
        let with_name = |error: Error| match error {
            Error::Message(text) => Error::msg(format!("{}: {text}", file.name)),
            other => other,
        };
        let bytes = match file.kind {
            Kind::Pdf => file.data.clone(),
            Kind::Image => imaging::compress_image(&file.data, 0, &no_progress).map_err(with_name)?,
            Kind::Word | Kind::Excel => office::to_pdf(file, &dest.work_dir(), &no_progress).map_err(with_name)?,
        };
        let mut doc = Document::load_mem(&bytes)
            .map_err(|e| with_name(super::keep_text::load_error(&e)))?;
        if doc.is_encrypted() {
            return Err(Error::msg(format!("{}: PDF protetto da password", file.name)));
        }

        doc.renumber_objects_with(next_id);
        next_id = doc.max_id + 1;

        let pages: Vec<ObjectId> = doc.get_pages().values().copied().collect();
        for &page in &pages {
            let values: Vec<(&[u8], Object)> = INHERITABLE
                .iter()
                .filter_map(|&key| inherited(&doc, page, key).map(|value| (key, value)))
                .collect();
            if let Ok(dict) = doc.get_dictionary_mut(page) {
                for (key, value) in values {
                    dict.set(key, value);
                }
            }
            kids.push(Object::Reference(page));
        }

        // Catalogo e albero delle pagine si ricreano; segnalibri e moduli non si fondono.
        for (id, object) in std::mem::take(&mut doc.objects) {
            let keep = match object.type_name() {
                Ok(name) => name != b"Catalog" && name != b"Pages",
                Err(_) => true,
            };
            if keep {
                objects.insert(id, object);
            }
        }
    }

    let root = (next_id, 0);
    let catalog = (next_id + 1, 0);
    for kid in &kids {
        if let Object::Reference(id) = kid {
            if let Some(Object::Dictionary(dict)) = objects.get_mut(id) {
                dict.set("Parent", Object::Reference(root));
            }
        }
    }
    let count = kids.len() as i64;
    objects.insert(root, dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count }.into());
    objects.insert(catalog, dictionary! { "Type" => "Catalog", "Pages" => root }.into());

    merged.objects = objects;
    merged.max_id = next_id + 1;
    merged.trailer.set("Root", Object::Reference(catalog));
    merged.prune_objects();
    merged.compress();

    let mut out = Vec::new();
    merged.save_modern(&mut out).map_err(|e| Error::msg(format!("unione non riuscita: {e}")))?;
    Ok(out)
}

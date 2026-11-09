//! Ottimizza il PDF e riduce le immagini interne senza toccare il testo.

use image::{DynamicImage, GrayImage, ImageFormat, RgbImage};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

use super::imaging::{Raster, MAX_PIXELS};
use super::pdf_write::jpeg_info;
use super::{limit, Error, Progress};

pub enum ShrinkError {
    Password,
    Other,
}

/// Livello di riduzione delle immagini interne.
#[derive(Clone, Copy)]
struct Rung {
    side: u32,
    quality: u8,
}

/// Dal piu' fedele al piu' leggero.
const RUNGS: [Rung; 8] = [
    Rung { side: 2400, quality: 75 },
    Rung { side: 2000, quality: 70 },
    Rung { side: 1600, quality: 65 },
    Rung { side: 1280, quality: 60 },
    Rung { side: 1024, quality: 50 },
    Rung { side: 800, quality: 45 },
    Rung { side: 640, quality: 40 },
    Rung { side: 480, quality: 30 },
];

/// Restituisce il risultato migliore, anche se ancora sopra il limite.
pub fn shrink(data: &[u8], progress: Progress) -> Result<Vec<u8>, ShrinkError> {
    progress(0.05, "Ottimizzazione PDF…");
    let base = rewrite(data, None)?;
    if base.len() <= limit() {
        return Ok(base);
    }

    // Il livello piu' basso dice subito se le immagini possono bastare.
    progress(0.15, "Riduzione immagini…");
    let last = RUNGS.len() - 1;
    let smallest = rewrite(&base, Some(RUNGS[last]))?;
    if smallest.len() > limit() {
        return Ok(if smallest.len() < base.len() { smallest } else { base });
    }

    // Ricerca binaria del livello piu' fedele che sta sotto il limite.
    let mut best = smallest;
    let (mut lo, mut hi) = (0i32, last as i32 - 1);
    let mut step = 0.0;
    while lo <= hi {
        step += 1.0;
        progress(0.15 + 0.75 * step / 4.0, "Riduzione immagini…");
        let mid = (lo + hi) / 2;
        match rewrite(&base, Some(RUNGS[mid as usize])) {
            Ok(out) if out.len() <= limit() => {
                best = out;
                hi = mid - 1;
            }
            _ => lo = mid + 1,
        }
    }
    Ok(best)
}

fn rewrite(data: &[u8], rung: Option<Rung>) -> Result<Vec<u8>, ShrinkError> {
    let mut doc = Document::load_mem(data).map_err(|e| {
        if is_password_error(&e) {
            ShrinkError::Password
        } else {
            ShrinkError::Other
        }
    })?;
    if doc.is_encrypted() {
        return Err(ShrinkError::Password);
    }
    doc.prune_objects();
    doc.compress();
    if let Some(rung) = rung {
        let ids: Vec<ObjectId> = doc.objects.keys().copied().collect();
        for id in ids {
            reencode(&mut doc, id, rung);
        }
    }
    let mut out = Vec::new();
    doc.save_modern(&mut out).map_err(|_| ShrinkError::Other)?;
    Ok(out)
}

/// Messaggio per un PDF che non si riesce ad aprire.
pub fn load_error(error: &lopdf::Error) -> Error {
    if is_password_error(error) {
        Error::msg("PDF protetto da password")
    } else {
        Error::msg("PDF danneggiato o non leggibile")
    }
}

fn is_password_error(error: &lopdf::Error) -> bool {
    matches!(error, lopdf::Error::InvalidPassword | lopdf::Error::Decryption(_))
}

/// Sostituisce un'immagine solo se sa decodificarla e il risultato e' piu' piccolo;
/// ogni altro oggetto resta intatto.
fn reencode(doc: &mut Document, id: ObjectId, rung: Rung) {
    let Some(raster) = decodable_image(doc, id) else { return };
    let (w, h) = raster.dimensions();
    let resized = (w.max(h) > rung.side).then(|| raster.resized(rung.side));
    let current = resized.as_ref().unwrap_or(&raster);
    let Some(encoded) = current.encode_jpeg(rung.quality) else { return };
    let (nw, nh) = current.dimensions();
    let space: &[u8] = if matches!(current, Raster::Gray(_)) { b"DeviceGray" } else { b"DeviceRGB" };

    let Ok(Object::Stream(stream)) = doc.get_object_mut(id) else { return };
    if encoded.len() >= stream.content.len() {
        return;
    }
    stream.dict.remove(b"DecodeParms");
    stream.dict.set("Filter", Object::Name(b"DCTDecode".to_vec()));
    stream.dict.set("Width", nw as i64);
    stream.dict.set("Height", nh as i64);
    stream.dict.set("BitsPerComponent", 8i64);
    stream.dict.set("ColorSpace", Object::Name(space.to_vec()));
    stream.allows_compression = false;
    stream.set_content(encoded);
}

fn decodable_image(doc: &Document, id: ObjectId) -> Option<Raster> {
    let Object::Stream(stream) = doc.get_object(id).ok()? else { return None };
    if !name_is(stream.dict.get(b"Subtype").ok()?, b"Image") || !can_reencode(doc, &stream.dict) {
        return None;
    }
    let components = color_components(doc, &stream.dict)?;
    decode_image(doc, stream, components)
}

/// Esclude cio' che un JPEG cambierebbe: maschere stencil o a chiave di colore,
/// curve Decode, Matte (richiede le stesse dimensioni dell'immagine).
fn can_reencode(doc: &Document, dict: &Dictionary) -> bool {
    if matches!(dict.get(b"ImageMask"), Ok(Object::Boolean(true))) {
        return false;
    }
    if dict.has(b"Decode") || dict.has(b"Matte") {
        return false;
    }
    if let Ok(mask) = dict.get(b"Mask") {
        match doc.dereference(mask) {
            Ok((_, Object::Array(_))) | Err(_) => return false,
            _ => {}
        }
    }
    if let Ok(mask) = dict.get(b"SMask") {
        match doc.dereference(mask) {
            Ok((_, Object::Stream(s))) if s.dict.has(b"Matte") => return false,
            Err(_) => return false,
            _ => {}
        }
    }
    true
}

fn name_is(object: &Object, name: &[u8]) -> bool {
    matches!(object, Object::Name(n) if n == name)
}

/// 1 o 3 per gli spazi colore convertibili, altrimenti None.
fn color_components(doc: &Document, dict: &Dictionary) -> Option<u8> {
    let (_, space) = doc.dereference(dict.get(b"ColorSpace").ok()?).ok()?;
    match space {
        Object::Name(n) if n == b"DeviceRGB" => Some(3),
        Object::Name(n) if n == b"DeviceGray" => Some(1),
        Object::Array(items) if items.len() >= 2 => {
            let (_, family) = doc.dereference(&items[0]).ok()?;
            if name_is(family, b"CalRGB") {
                Some(3)
            } else if name_is(family, b"CalGray") {
                Some(1)
            } else if name_is(family, b"ICCBased") {
                let (_, profile) = doc.dereference(&items[1]).ok()?;
                let Object::Stream(profile) = profile else { return None };
                match profile.dict.get(b"N").ok()?.as_i64().ok()? {
                    1 => Some(1),
                    3 => Some(3),
                    _ => None,
                }
            } else {
                None
            }
        }
        _ => None,
    }
}

fn decode_image(doc: &Document, stream: &Stream, components: u8) -> Option<Raster> {
    let filters = stream_filters(doc, &stream.dict);
    let names: Vec<&[u8]> = filters.iter().map(Vec::as_slice).collect();
    if names.iter().any(|f| matches!(*f, b"JPXDecode" | b"JBIG2Decode" | b"CCITTFaxDecode")) {
        return None;
    }
    if names == [b"DCTDecode".as_slice()] {
        // Il numero di componenti deve coincidere: i JPEG CMYK restano intatti.
        if jpeg_info(&stream.content)?.components != components {
            return None;
        }
        let decoded = image::load_from_memory_with_format(&stream.content, ImageFormat::Jpeg).ok()?;
        return Some(match decoded {
            DynamicImage::ImageLuma8(gray) => Raster::Gray(gray),
            other => Raster::Rgb(other.to_rgb8()),
        });
    }
    if names.contains(&b"DCTDecode".as_slice()) {
        return None;
    }

    let bits = stream.dict.get(b"BitsPerComponent").ok()?.as_i64().ok()?;
    let width = int_entry(doc, &stream.dict, b"Width")?;
    let height = int_entry(doc, &stream.dict, b"Height")?;
    if bits != 8 || width == 0 || height == 0 || width as u64 * height as u64 > MAX_PIXELS {
        return None;
    }
    let plain = if names.is_empty() {
        stream.content.clone()
    } else {
        stream.decompressed_content().ok()?
    };
    let expected = width as usize * height as usize * components as usize;
    if plain.len() < expected {
        return None;
    }
    let pixels = plain[..expected].to_vec();
    if components == 1 {
        GrayImage::from_raw(width, height, pixels).map(Raster::Gray)
    } else {
        RgbImage::from_raw(width, height, pixels).map(Raster::Rgb)
    }
}

fn int_entry(doc: &Document, dict: &Dictionary, key: &[u8]) -> Option<u32> {
    let (_, value) = doc.dereference(dict.get(key).ok()?).ok()?;
    u32::try_from(value.as_i64().ok()?).ok()
}

fn stream_filters(doc: &Document, dict: &Dictionary) -> Vec<Vec<u8>> {
    let Ok(filter) = dict.get(b"Filter") else { return Vec::new() };
    let Ok((_, filter)) = doc.dereference(filter) else { return Vec::new() };
    match filter {
        Object::Name(name) => vec![name.clone()],
        Object::Array(items) => items
            .iter()
            .filter_map(|o| doc.dereference(o).ok())
            .filter_map(|(_, o)| o.as_name().ok().map(<[u8]>::to_vec))
            .collect(),
        _ => Vec::new(),
    }
}

//! Aggiunge alle pagine un testo invisibile allineato all'immagine, per selezionarlo e cercarlo.
//!
//! Stessa tecnica di Tesseract: un font TrueType incorporato con un solo glifo e i codici
//! Unicode usati direttamente. Copre ogni alfabeto e pesa poco. Il glifo e' un rettangolo alto
//! come il font perche' i lettori ne ricavano l'altezza della selezione: non si vede
//! comunque, il testo e' in modalita' 3 (invisibile).

use std::fmt::Write as _;

use lopdf::{dictionary, Dictionary, Document, Object, ObjectId, Stream};

pub(super) const FONT_NAME: &str = "TPDF";
/// Metriche del glifo su 1000 unita'.
const GLYPH_WIDTH: i16 = 500;
const ASCENT: i16 = 800;
const DESCENT: i16 = -200;

/// Parola riconosciuta; coordinate da 0 a 1 sulla pagina diritta, origine in alto a sinistra.
pub struct Word {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Riquadro visibile e rotazione della pagina, per passare da pagina diritta a coordinate PDF.
pub(super) struct PageGeometry {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    rotate: i64,
}

impl PageGeometry {
    pub(super) fn read(doc: &Document, page: ObjectId) -> Option<Self> {
        let boxed = inherited(doc, page, b"CropBox").or_else(|| inherited(doc, page, b"MediaBox"))?;
        let Object::Array(values) = boxed else { return None };
        let mut numbers = Vec::with_capacity(4);
        for value in &values {
            let (_, value) = doc.dereference(value).ok()?;
            numbers.push(value.as_float().ok()? as f64);
        }
        if numbers.len() != 4 {
            return None;
        }
        let rotate = inherited(doc, page, b"Rotate").and_then(|o| o.as_i64().ok()).unwrap_or(0).rem_euclid(360);
        Some(PageGeometry {
            x0: numbers[0].min(numbers[2]),
            y0: numbers[1].min(numbers[3]),
            x1: numbers[0].max(numbers[2]),
            y1: numbers[1].max(numbers[3]),
            rotate,
        })
    }

    /// Origine in punti e matrice del testo per un punto della pagina diritta.
    fn place(&self, u: f64, v: f64) -> (f64, f64, [f64; 4]) {
        let (w, h) = (self.x1 - self.x0, self.y1 - self.y0);
        match self.rotate {
            90 => (self.x0 + v * w, self.y0 + u * h, [0.0, 1.0, -1.0, 0.0]),
            180 => (self.x1 - u * w, self.y0 + v * h, [-1.0, 0.0, 0.0, -1.0]),
            270 => (self.x1 - v * w, self.y1 - u * h, [0.0, -1.0, 1.0, 0.0]),
            _ => (self.x0 + u * w, self.y1 - v * h, [1.0, 0.0, 0.0, 1.0]),
        }
    }

    /// Larghezza e altezza in punti della pagina diritta.
    pub(super) fn upright_size(&self) -> (f64, f64) {
        let (w, h) = (self.x1 - self.x0, self.y1 - self.y0);
        if matches!(self.rotate, 90 | 270) { (h, w) } else { (w, h) }
    }

    /// Matrice cm che porta le coordinate della pagina diritta (punti, origine in basso a
    /// sinistra) in quelle del PDF: stessa trasformazione di place.
    pub(super) fn upright_matrix(&self) -> [f64; 6] {
        let w = self.x1 - self.x0;
        match self.rotate {
            90 => [0.0, 1.0, -1.0, 0.0, self.x0 + w, self.y0],
            180 => [-1.0, 0.0, 0.0, -1.0, self.x1, self.y1],
            270 => [0.0, -1.0, 1.0, 0.0, self.x0, self.y1],
            _ => [1.0, 0.0, 0.0, 1.0, self.x0, self.y0],
        }
    }
}

/// Attributo della pagina, anche se ereditato dai nodi superiori.
pub fn inherited(doc: &Document, page: ObjectId, key: &[u8]) -> Option<Object> {
    let mut current = page;
    for _ in 0..64 {
        let dict = doc.get_dictionary(current).ok()?;
        if let Ok(value) = dict.get(key) {
            return doc.dereference(value).ok().map(|(_, object)| object.clone());
        }
        current = dict.get(b"Parent").ok()?.as_reference().ok()?;
    }
    None
}

/// Scrive lo strato di testo sulle pagine indicate. Le altre restano intatte.
pub fn add_layers(doc: &mut Document, pages: &[(ObjectId, Vec<Word>)]) -> Result<(), String> {
    let mut contents = Vec::new();
    for (page, words) in pages {
        if words.is_empty() {
            continue;
        }
        let geometry = PageGeometry::read(doc, *page).ok_or("pagina senza dimensioni")?;
        contents.push((*page, page_content(words, &geometry)));
    }
    if contents.is_empty() {
        return Ok(());
    }

    let font = add_font(doc);
    let open = doc.add_object(Stream::new(Dictionary::new(), b"q\n".to_vec()));
    let close = doc.add_object(Stream::new(Dictionary::new(), b"\nQ\n".to_vec()));
    for (page, content) in contents {
        let mut stream = Stream::new(Dictionary::new(), content);
        let _ = stream.compress();
        let layer = doc.add_object(stream);
        append_layer(doc, page, open, close, layer, &[(b"Font", FONT_NAME.as_bytes(), font)])?;
    }
    Ok(())
}

fn page_content(words: &[Word], geometry: &PageGeometry) -> Vec<u8> {
    let (page_w, page_h) = geometry.upright_size();
    let advance = f64::from(GLYPH_WIDTH) / 1000.0;
    let below = -f64::from(DESCENT) / 1000.0;
    let mut out = String::from("BT\n3 Tr\n");
    for word in words {
        let units: Vec<u16> = word.text.chars().filter(|c| (*c as u32) < 0x10000).map(|c| c as u16).collect();
        if units.is_empty() {
            continue;
        }
        let size = (word.h as f64 * page_h).max(1.0);
        let width = word.w as f64 * page_w;
        let scale = (width / (units.len() as f64 * advance * size) * 100.0).clamp(1.0, 1000.0);
        // La linea di base sta sopra il fondo del riquadro, per lo spazio dei discendenti.
        let baseline = word.y as f64 + word.h as f64 * (1.0 - below);
        let (x, y, m) = geometry.place(word.x as f64, baseline);
        let _ = writeln!(out, "/{FONT_NAME} {size:.2} Tf {scale:.1} Tz {:.0} {:.0} {:.0} {:.0} {x:.2} {y:.2} Tm", m[0], m[1], m[2], m[3]);
        out.push('<');
        // Lo spazio finale separa le parole quando si copia il testo.
        for unit in units.iter().copied().chain(std::iter::once(0x20)) {
            let _ = write!(out, "{unit:04X}");
        }
        out.push_str("> Tj\n");
    }
    out.push_str("ET\n");
    out.into_bytes()
}

fn compressed(content: Vec<u8>, dict: Dictionary) -> Stream {
    let mut stream = Stream::new(dict, content);
    let _ = stream.compress();
    stream
}

fn add_font(doc: &mut Document) -> ObjectId {
    let program = invisible_truetype();
    let program_len = program.len() as i64;
    let file = doc.add_object(compressed(program, dictionary! { "Length1" => program_len }));
    let descriptor = doc.add_object(dictionary! {
        "Type" => "FontDescriptor",
        "FontName" => Object::Name(FONT_NAME.as_bytes().to_vec()),
        "Flags" => 4,
        "FontBBox" => vec![0.into(), i64::from(DESCENT).into(), i64::from(GLYPH_WIDTH).into(), i64::from(ASCENT).into()],
        "ItalicAngle" => 0,
        "Ascent" => i64::from(ASCENT),
        "Descent" => i64::from(DESCENT),
        "CapHeight" => i64::from(ASCENT),
        "StemV" => 80,
        "FontFile2" => file,
    });
    // Ogni codice Unicode punta all'unico glifo disegnato.
    let map = doc.add_object(compressed([0u8, 1].repeat(65536), Dictionary::new()));
    let descendant = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "CIDFontType2",
        "BaseFont" => Object::Name(FONT_NAME.as_bytes().to_vec()),
        "CIDSystemInfo" => dictionary! {
            "Registry" => Object::string_literal("Adobe"),
            "Ordering" => Object::string_literal("Identity"),
            "Supplement" => 0,
        },
        "FontDescriptor" => descriptor,
        "DW" => i64::from(GLYPH_WIDTH),
        "CIDToGIDMap" => map,
    });
    let unicode = doc.add_object(compressed(identity_cmap(), Dictionary::new()));
    doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type0",
        "BaseFont" => Object::Name(FONT_NAME.as_bytes().to_vec()),
        "Encoding" => "Identity-H",
        "DescendantFonts" => vec![Object::Reference(descendant)],
        "ToUnicode" => unicode,
    })
}

/// I codici del testo sono gia' UTF-16: la mappa verso Unicode e' l'identita'.
fn identity_cmap() -> Vec<u8> {
    b"/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo <</Registry (Adobe) /Ordering (UCS) /Supplement 0>> def\n/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n1 beginbfrange\n<0000> <FFFF> <0000>\nendbfrange\nendcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n".to_vec()
}

fn be16(values: &[i16]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_be_bytes()).collect()
}

/// Font TrueType minimo: glifo 0 vuoto e glifo 1 rettangolare.
fn invisible_truetype() -> Vec<u8> {
    let mut head = vec![0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0x5F, 0x0F, 0x3C, 0xF5, 0, 0x0B];
    head.extend(be16(&[1000]));
    head.extend([0u8; 16]);
    head.extend(be16(&[0, DESCENT, GLYPH_WIDTH, ASCENT, 0, 8, 2, 0, 0]));

    let mut hhea = vec![0, 1, 0, 0];
    hhea.extend(be16(&[ASCENT, DESCENT, 0, GLYPH_WIDTH, 0, 0, GLYPH_WIDTH, 1, 0, 0, 0, 0, 0, 0, 0, 2]));

    let mut maxp = vec![0, 1, 0, 0];
    maxp.extend(be16(&[2, 4, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0]));

    let hmtx = be16(&[GLYPH_WIDTH, 0, GLYPH_WIDTH, 0]);

    let height = ASCENT - DESCENT;
    let mut glyph = be16(&[1, 0, DESCENT, GLYPH_WIDTH, ASCENT, 3, 0]);
    glyph.extend([1u8; 4]);
    glyph.extend(be16(&[0, 0, GLYPH_WIDTH, 0, DESCENT, height, 0, -height]));
    if glyph.len() % 2 == 1 {
        glyph.push(0);
    }
    let loca = be16(&[0, 0, (glyph.len() / 2) as i16]);

    // I tag devono stare in ordine alfabetico.
    let mut tables: Vec<(&[u8; 4], Vec<u8>)> =
        vec![(b"glyf", glyph), (b"head", head), (b"hhea", hhea), (b"hmtx", hmtx), (b"loca", loca), (b"maxp", maxp)];

    let count = tables.len() as u16;
    let mut font = vec![0, 1, 0, 0];
    font.extend(count.to_be_bytes());
    let selector = 15 - count.leading_zeros() as u16;
    let range = (1u16 << selector) * 16;
    font.extend(range.to_be_bytes());
    font.extend(selector.to_be_bytes());
    font.extend((count * 16 - range).to_be_bytes());

    let mut offset = 12 + 16 * tables.len();
    let mut head_offset = 0;
    let mut body = Vec::new();
    for (tag, data) in &mut tables {
        let length = data.len();
        data.resize(length.div_ceil(4) * 4, 0);
        font.extend(tag.as_slice());
        font.extend(checksum(data).to_be_bytes());
        font.extend((offset as u32).to_be_bytes());
        font.extend((length as u32).to_be_bytes());
        if tag.as_slice() == b"head" {
            head_offset = offset;
        }
        offset += data.len();
        body.extend_from_slice(data);
    }
    font.extend(body);
    let adjustment = 0xB1B0_AFBAu32.wrapping_sub(checksum(&font));
    font[head_offset + 8..head_offset + 12].copy_from_slice(&adjustment.to_be_bytes());
    font
}

fn checksum(data: &[u8]) -> u32 {
    data.chunks(4).fold(0u32, |sum, chunk| {
        let mut word = [0u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum.wrapping_add(u32::from_be_bytes(word))
    })
}

/// Isola il contenuto originale con q/Q, cosi' non altera la posizione del livello aggiunto.
/// resources: (categoria, nome, oggetto), per esempio (Font, TPDF, id) o (XObject, Im1, id).
pub(super) fn append_layer(doc: &mut Document, page: ObjectId, open: ObjectId, close: ObjectId, layer: ObjectId, resources_added: &[(&[u8], &[u8], ObjectId)]) -> Result<(), String> {
    let mut resources = match inherited(doc, page, b"Resources") {
        Some(Object::Dictionary(dict)) => dict,
        _ => Dictionary::new(),
    };
    for (category, name, id) in resources_added {
        let mut entries = match resources.get(category).ok().and_then(|f| doc.dereference(f).ok()) {
            Some((_, Object::Dictionary(dict))) => dict.clone(),
            _ => Dictionary::new(),
        };
        entries.set(name.to_vec(), Object::Reference(*id));
        resources.set(category.to_vec(), Object::Dictionary(entries));
    }

    let dict = doc.get_dictionary(page).map_err(|e| e.to_string())?;
    let original = dict.get(b"Contents").ok().cloned();
    let original_items = match original {
        Some(Object::Array(items)) => items,
        Some(Object::Reference(id)) => match doc.get_object(id) {
            Ok(Object::Array(items)) => items.clone(),
            _ => vec![Object::Reference(id)],
        },
        _ => Vec::new(),
    };
    let mut contents = vec![Object::Reference(open)];
    contents.extend(original_items);
    contents.push(Object::Reference(close));
    contents.push(Object::Reference(layer));

    let dict = doc.get_dictionary_mut(page).map_err(|e| e.to_string())?;
    dict.set("Resources", Object::Dictionary(resources));
    dict.set("Contents", Object::Array(contents));
    Ok(())
}

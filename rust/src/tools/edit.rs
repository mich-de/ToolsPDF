//! Zona Strumenti: ruota, estrai ed elimina pagine, numeri, timbro, firma, password, JPG.
//! Tutto con lopdf e hayro, in memoria: niente Office e niente programmi esterni.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Cursor;
use std::sync::Arc;

use image::imageops::{self, FilterType};
use image::{DynamicImage, ImageReader};
use lopdf::encryption::crypt_filters::{Aes256CryptFilter, CryptFilter};
use lopdf::{dictionary, Dictionary, Document, EncryptionState, EncryptionVersion, Object, ObjectId, Permissions, Stream};

use super::imaging::{self, Raster};
use super::keep_text;
use super::split::{drop_internal_links, DOCUMENT_WIDE};
use super::text_layer::{append_layer, inherited, PageGeometry};
use super::{guarded, raster, save_as, save_files, Callbacks, Destination, Error, Kind, Loaded, Outcome, Progress};

/// Risoluzione delle pagine esportate in JPG.
pub const JPG_DPI: u32 = 150;
const JPG_QUALITY: u8 = 85;
/// Lato lungo massimo dell'immagine della firma incorporata.
const SIGNATURE_MAX_SIDE: u32 = 1200;
/// Larghezze della firma S, M, L in punti (1 pt = 1/72 di pollice).
const SIGNATURE_WIDTHS: [f64; 3] = [110.0, 160.0, 230.0];
const PAGE_MARGIN: f64 = 36.0;
const STAMP_OPACITY: f64 = 0.3;

const FONT_REGULAR: &[u8] = b"TPDFh";
const FONT_BOLD: &[u8] = b"TPDFb";
const STATE_STAMP: &[u8] = b"TPDFgs";
const SIGNATURE_IMAGE: &[u8] = b"TPDFsig";

/// Firma come immagine su una pagina.
#[derive(Clone)]
pub struct Signature {
    /// File immagine originale (PNG, JPG...): la trasparenza del PNG resta.
    pub image: Vec<u8>,
    /// Indice tra le pagine che restano; None = ultima.
    pub page: Option<usize>,
    /// Griglia 3x3 letta per righe dall'alto: 0 in alto a sinistra, 8 in basso a destra.
    pub position: u8,
    /// 0 = S, 1 = M, 2 = L.
    pub size: u8,
}

/// Modifiche in sospeso, applicate tutte insieme da Salva.
#[derive(Clone, Default)]
pub struct Plan {
    /// Per ogni pagina originale: resta nel documento?
    pub keep: Vec<bool>,
    /// Per ogni pagina originale: rotazione da aggiungere in gradi (multipli di 90).
    pub rotation: Vec<u16>,
    /// "Pagina 1 di N" in basso al centro.
    pub numbers: bool,
    /// Testo diagonale semitrasparente su ogni pagina.
    pub stamp: Option<String>,
    pub signature: Option<Signature>,
    /// Protegge il risultato con questa password (AES-256).
    pub password: Option<String>,
}

impl Plan {
    /// Piano vuoto per un documento di pages pagine.
    pub fn new(pages: usize) -> Self {
        Plan { keep: vec![true; pages], rotation: vec![0; pages], ..Plan::default() }
    }

    /// Solo pagine e rotazioni: serve a Estrai e ai JPG.
    fn pages_only(&self) -> Plan {
        Plan { keep: self.keep.clone(), rotation: self.rotation.clone(), ..Plan::default() }
    }
}

/// Il PDF chiede una password? Anche se si apre senza (solo password del proprietario),
/// si chiede: chi lo ha protetto non voleva modifiche.
pub fn is_protected(data: &[u8]) -> bool {
    match Document::load_mem(data) {
        Ok(doc) => doc.is_encrypted(),
        Err(error) => matches!(error, lopdf::Error::InvalidPassword | lopdf::Error::Decryption(_)),
    }
}

/// Toglie la password: restituisce lo stesso PDF in chiaro.
pub fn unlock(data: &[u8], password: &str) -> Result<Vec<u8>, Error> {
    // Il caricamento con password prova prima quella vuota e toglie la cifratura: la password
    // data si verifica sul documento ancora cifrato, caricato senza.
    let locked = Document::load_mem(data).map_err(|e| keep_text::load_error(&e))?;
    if locked.is_encrypted() && locked.authenticate_password(password).is_err() {
        return Err(Error::msg("password errata"));
    }
    let mut doc = Document::load_mem_with_options(data, lopdf::LoadOptions::with_password(password)).map_err(|_| Error::msg("password errata"))?;
    strip_encryption(&mut doc);
    save_doc(&mut doc, false)
}

fn strip_encryption(doc: &mut Document) {
    if let Ok(id) = doc.trailer.get(b"Encrypt").and_then(Object::as_reference) {
        doc.objects.remove(&id);
    }
    doc.trailer.remove(b"Encrypt");
    doc.encryption_state = None;
}

fn open(data: &[u8]) -> Result<Document, Error> {
    let doc = Document::load_mem(data).map_err(|e| keep_text::load_error(&e))?;
    if doc.is_encrypted() {
        return Err(Error::msg("PDF protetto da password"));
    }
    Ok(doc)
}

/// Numero di pagine del PDF in chiaro.
pub fn page_count(data: &[u8]) -> Result<usize, Error> {
    Ok(open(data)?.get_pages().len())
}

/// Applica il piano e restituisce il nuovo PDF.
pub fn apply(data: &[u8], plan: &Plan) -> Result<Vec<u8>, Error> {
    let mut doc = open(data)?;
    let pages: Vec<(u32, ObjectId)> = doc.get_pages().into_iter().collect();
    let total = pages.len();
    if total == 0 {
        return Err(Error::msg("il PDF non ha pagine"));
    }
    if plan.keep.len() != total || plan.rotation.len() != total {
        return Err(Error::msg("il documento è cambiato: riaprilo nel pannello"));
    }
    if !plan.keep.contains(&true) {
        return Err(Error::msg("nessuna pagina rimasta"));
    }

    // Prima le rotazioni: numeri, timbro e firma seguono la pagina diritta.
    for (i, (_, id)) in pages.iter().enumerate() {
        let delta = i64::from(plan.rotation[i] % 360);
        if delta != 0 {
            let current = inherited(&doc, *id, b"Rotate").and_then(|o| o.as_i64().ok()).unwrap_or(0);
            if let Ok(dict) = doc.get_dictionary_mut(*id) {
                dict.set("Rotate", (current + delta).rem_euclid(360));
            }
        }
    }

    let removed: Vec<u32> = pages.iter().zip(&plan.keep).filter(|(_, keep)| !**keep).map(|((n, _), _)| *n).collect();
    if !removed.is_empty() {
        // Segnalibri, moduli e collegamenti verso pagine tolte terrebbero il file pesante o rotto.
        if let Ok(catalog) = doc.catalog_mut() {
            for key in DOCUMENT_WIDE {
                catalog.remove(key);
            }
        }
        for ((_, id), keep) in pages.iter().zip(&plan.keep) {
            if *keep {
                drop_internal_links(&mut doc, *id);
            }
        }
        doc.delete_pages(&removed);
    }

    let kept: Vec<ObjectId> = doc.get_pages().values().copied().collect();
    add_overlays(&mut doc, &kept, plan)?;
    if !removed.is_empty() {
        doc.prune_objects();
    }
    doc.compress();

    match &plan.password {
        Some(password) => {
            encrypt(&mut doc, password)?;
            save_doc(&mut doc, true)
        }
        None => save_doc(&mut doc, false),
    }
}

/// Nuovo PDF con le sole pagine indicate (indici delle pagine originali), con le rotazioni del piano.
pub fn extract(data: &[u8], plan: &Plan, selected: &[usize]) -> Result<Vec<u8>, Error> {
    let mut only = plan.pages_only();
    for (i, keep) in only.keep.iter_mut().enumerate() {
        *keep = selected.contains(&i);
    }
    apply(data, &only)
}

/// Un JPG per pagina rimasta, con le rotazioni del piano.
pub fn to_jpegs(data: &[u8], plan: &Plan, progress: Progress) -> Result<Vec<Vec<u8>>, Error> {
    progress(0.05, "Preparazione pagine…");
    let pdf_bytes = apply(data, &plan.pages_only())?;
    let pdf = raster::open(pdf_bytes)?;
    let total = pdf.pages().len();
    let mut out = Vec::with_capacity(total);
    for (i, page) in pdf.pages().iter().enumerate() {
        progress(0.1 + 0.85 * i as f64 / total.max(1) as f64, &format!("Pagina {} di {total}…", i + 1));
        let image = raster::render_rgb(page, JPG_DPI);
        let jpeg = Raster::from_rgb(image).encode_jpeg(JPG_QUALITY).ok_or_else(|| Error::msg(format!("pagina {} non convertibile", i + 1)))?;
        out.push(jpeg);
    }
    Ok(out)
}

/// Riconosce un'immagine dal contenuto, senza decodificarla: serve alla firma.
pub fn is_image(data: &[u8]) -> bool {
    ImageReader::new(Cursor::new(data)).with_guessed_format().ok().and_then(|r| r.format()).is_some()
}

/// PDF su cui lavora il pannello: il PDF stesso, o l'immagine messa in una pagina.
pub fn prepare(file: &Loaded, progress: Progress) -> Result<Vec<u8>, Error> {
    match file.kind {
        Kind::Pdf => Ok(file.data.clone()),
        Kind::Image => imaging::compress_image(&file.data, 0, progress),
        Kind::Word | Kind::Excel => Err(Error::msg("Strumenti lavora su PDF e immagini")),
    }
}

/// Salva: tutte le modifiche in nome_modificato.pdf.
pub fn save_modified(dest: &Destination, data: &[u8], plan: &Plan, progress: Progress) -> Result<Outcome, Error> {
    guarded(|| {
        progress(0.2, "Applicazione delle modifiche…");
        let out = apply(data, plan)?;
        let output = save_as(dest, "_modificato", "pdf", &out, quiet(progress))?;
        let kept = plan.keep.iter().filter(|k| **k).count();
        let mut summary = pages_label(kept);
        if plan.password.is_some() {
            summary.push_str(" · protetto con password");
        }
        Ok(Outcome { output, is_folder: false, summary })
    })
}

/// Estrai: le pagine scelte in nome_estratto.pdf.
pub fn save_extract(dest: &Destination, data: &[u8], plan: &Plan, selected: &[usize], progress: Progress) -> Result<Outcome, Error> {
    guarded(|| {
        progress(0.2, "Estrazione delle pagine…");
        let out = extract(data, plan, selected)?;
        let output = save_as(dest, "_estratto", "pdf", &out, quiet(progress))?;
        Ok(Outcome { output, is_folder: false, summary: format!("{} estratte", pages_label(selected.len())) })
    })
}

/// Pagine in JPG: cartella nome_immagini con un JPG per pagina.
pub fn save_jpegs(dest: &Destination, name: &str, data: &[u8], plan: &Plan, progress: Progress) -> Result<Outcome, Error> {
    guarded(|| {
        let images = to_jpegs(data, plan, progress)?;
        progress(0.98, "Salvataggio…");
        let folder = save_files(dest, name, "_immagini", "jpg", &images).map_err(|e| Error::msg(format!("salvataggio non riuscito: {e}")))?;
        Ok(Outcome { output: folder, is_folder: true, summary: format!("{} in JPG a {JPG_DPI} DPI", pages_label(images.len())) })
    })
}

fn pages_label(count: usize) -> String {
    if count == 1 { "1 pagina".into() } else { format!("{count} pagine") }
}

/// Callbacks per save_as, che usa solo l'avanzamento.
fn quiet(progress: Progress<'_>) -> Callbacks<'_> {
    Callbacks { progress, confirm: &|_| true }
}

fn save_doc(doc: &mut Document, encrypted: bool) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    // Gli object stream non vanno d'accordo con la cifratura oggetto per oggetto: formato classico.
    let result = if encrypted { doc.save_to(&mut out) } else { doc.save_modern(&mut out) };
    result.map_err(|e| Error::msg(format!("PDF non scrivibile: {e}")))?;
    Ok(out)
}

/// AES-256 (PDF 2.0, revisione 6): stessa password per aprire e per modificare.
fn encrypt(doc: &mut Document, password: &str) -> Result<(), Error> {
    if password.is_empty() {
        return Err(Error::msg("password vuota"));
    }
    let mut key = [0u8; 32];
    getrandom::fill(&mut key).map_err(|_| Error::msg("generatore casuale di Windows non disponibile"))?;
    let filter: Arc<dyn CryptFilter> = Arc::new(Aes256CryptFilter);
    let version = EncryptionVersion::V5 {
        encrypt_metadata: true,
        crypt_filters: BTreeMap::from([(b"StdCF".to_vec(), filter)]),
        file_encryption_key: &key,
        stream_filter: b"StdCF".to_vec(),
        string_filter: b"StdCF".to_vec(),
        owner_password: password,
        user_password: password,
        permissions: Permissions::all(),
    };
    let state = EncryptionState::try_from(version).map_err(|e| Error::msg(format!("cifratura non riuscita: {e}")))?;
    doc.encrypt(&state).map_err(|e| Error::msg(format!("cifratura non riuscita: {e}")))
}

// ---------- livelli aggiunti: numeri, timbro, firma ----------

fn add_overlays(doc: &mut Document, pages: &[ObjectId], plan: &Plan) -> Result<(), Error> {
    let stamp = plan.stamp.as_deref().map(str::trim).filter(|t| !t.is_empty());
    if !plan.numbers && stamp.is_none() && plan.signature.is_none() {
        return Ok(());
    }
    let total = pages.len();
    let signature = match &plan.signature {
        Some(sig) => Some((signature_object(doc, &sig.image)?, sig, sig.page.unwrap_or(total - 1).min(total - 1))),
        None => None,
    };
    let regular = doc.add_object(base_font(b"Helvetica"));
    let bold = doc.add_object(base_font(b"Helvetica-Bold"));
    let state = doc.add_object(dictionary! { "Type" => "ExtGState", "ca" => STAMP_OPACITY, "CA" => STAMP_OPACITY });
    let open = doc.add_object(Stream::new(Dictionary::new(), b"q\n".to_vec()));
    let close = doc.add_object(Stream::new(Dictionary::new(), b"\nQ\n".to_vec()));

    for (index, &page) in pages.iter().enumerate() {
        let geometry = PageGeometry::read(doc, page).ok_or_else(|| Error::msg("pagina senza dimensioni"))?;
        let (width, height) = geometry.upright_size();
        let m = geometry.upright_matrix();
        let mut content = format!("q {} {} {} {} {} {} cm\n", m[0], m[1], m[2], m[3], fmt(m[4]), fmt(m[5]));
        let mut resources: Vec<(&[u8], &[u8], ObjectId)> = Vec::new();

        if let Some(((image, image_w, image_h), sig, on_page)) = &signature {
            if *on_page == index {
                let (w, h) = signature_size(*image_w, *image_h, sig.size, width, height);
                let (col, row) = (f64::from(sig.position % 3), f64::from((sig.position / 3).min(2)));
                let x = PAGE_MARGIN + col * (width - 2.0 * PAGE_MARGIN - w).max(0.0) / 2.0;
                let y = height - PAGE_MARGIN - h - row * (height - 2.0 * PAGE_MARGIN - h).max(0.0) / 2.0;
                let _ = writeln!(content, "q {} 0 0 {} {} {} cm /{} Do Q", fmt(w), fmt(h), fmt(x), fmt(y), name(SIGNATURE_IMAGE));
                resources.push((b"XObject", SIGNATURE_IMAGE, *image));
            }
        }
        if plan.numbers {
            let text = format!("Pagina {} di {total}", index + 1);
            let size = 10.0;
            let x = (width - text_width(&text, false) * size) / 2.0;
            let _ = writeln!(content, "BT 0.3 g /{} {size} Tf 1 0 0 1 {} {} Tm {} Tj ET", name(FONT_REGULAR), fmt(x), fmt(18.0), hex_text(&text));
            resources.push((b"Font", FONT_REGULAR, regular));
        }
        if let Some(text) = stamp {
            content.push_str(&stamp_content(text, width, height));
            resources.push((b"Font", FONT_BOLD, bold));
            resources.push((b"ExtGState", STATE_STAMP, state));
        }
        content.push_str("Q\n");

        let mut stream = Stream::new(Dictionary::new(), content.into_bytes());
        let _ = stream.compress();
        let layer = doc.add_object(stream);
        append_layer(doc, page, open, close, layer, &resources).map_err(Error::msg)?;
    }
    Ok(())
}

/// Testo centrato sulla diagonale dal basso a sinistra all'alto a destra.
fn stamp_content(text: &str, width: f64, height: f64) -> String {
    let units = text_width(text, true).max(0.1);
    let diagonal = (width * width + height * height).sqrt();
    let size = (diagonal * 0.7 / units).min(height * 0.18).min(200.0);
    let angle = height.atan2(width);
    let (sin, cos) = angle.sin_cos();
    // Inizio del testo: centro pagina meno meta' larghezza lungo la diagonale e meta' altezza delle maiuscole.
    let (half, cap) = (units * size / 2.0, 0.72 * size / 2.0);
    let x = width / 2.0 - half * cos + cap * sin;
    let y = height / 2.0 - half * sin - cap * cos;
    format!(
        "q /{} gs 0.8 0.1 0.1 rg BT /{} {} Tf {} {} {} {} {} {} Tm {} Tj ET Q\n",
        name(STATE_STAMP), name(FONT_BOLD), fmt(size), fmt4(cos), fmt4(sin), fmt4(-sin), fmt4(cos), fmt(x), fmt(y), hex_text(text)
    )
}

fn signature_size(image_w: u32, image_h: u32, size: u8, page_w: f64, page_h: f64) -> (f64, f64) {
    let ratio = f64::from(image_h) / f64::from(image_w.max(1));
    let mut w = SIGNATURE_WIDTHS[usize::from(size.min(2))].min(page_w * 0.6);
    let mut h = w * ratio;
    if h > page_h * 0.4 {
        h = page_h * 0.4;
        w = h / ratio;
    }
    (w, h)
}

/// Immagine della firma come XObject: JPEG per i colori, maschera di trasparenza se il file la ha.
fn signature_object(doc: &mut Document, data: &[u8]) -> Result<(ObjectId, u32, u32), Error> {
    let unreadable = || Error::msg("immagine della firma non leggibile");
    let image = ImageReader::new(Cursor::new(data)).with_guessed_format().map_err(|_| unreadable())?.decode().map_err(|_| unreadable())?;
    let image = if image.width().max(image.height()) > SIGNATURE_MAX_SIDE {
        let (w, h) = (image.width() as f64, image.height() as f64);
        let scale = f64::from(SIGNATURE_MAX_SIDE) / w.max(h);
        DynamicImage::from(imageops::resize(&image.to_rgba8(), (w * scale).round().max(1.0) as u32, (h * scale).round().max(1.0) as u32, FilterType::Triangle))
    } else {
        image
    };
    let (w, h) = (image.width(), image.height());
    let rgba = image.to_rgba8();
    let has_alpha = image.color().has_alpha() && rgba.pixels().any(|p| p[3] < 255);
    // Dove e' trasparente il colore non conta: bianco, cosi' il JPEG non sbava ai bordi.
    let rgb = image::RgbImage::from_fn(w, h, |x, y| {
        let p = rgba.get_pixel(x, y);
        let a = u16::from(p[3]);
        let mix = |c: u8| ((u16::from(c) * a + 255 * (255 - a)) / 255) as u8;
        image::Rgb([mix(p[0]), mix(p[1]), mix(p[2])])
    });
    let raster = Raster::from_rgb(rgb);
    let jpeg = raster.encode_jpeg(90).ok_or_else(unreadable)?;
    let color_space = if matches!(raster, Raster::Gray(_)) { "DeviceGray" } else { "DeviceRGB" };
    let mut dict = dictionary! {
        "Type" => "XObject",
        "Subtype" => "Image",
        "Width" => i64::from(w),
        "Height" => i64::from(h),
        "ColorSpace" => color_space,
        "BitsPerComponent" => 8,
        "Filter" => "DCTDecode",
    };
    if has_alpha {
        let alpha: Vec<u8> = rgba.pixels().map(|p| p[3]).collect();
        let mut mask = Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Image",
                "Width" => i64::from(w),
                "Height" => i64::from(h),
                "ColorSpace" => "DeviceGray",
                "BitsPerComponent" => 8,
            },
            alpha,
        );
        let _ = mask.compress();
        dict.set("SMask", doc.add_object(mask));
    }
    let mut stream = Stream::new(dict, jpeg);
    stream.allows_compression = false;
    Ok((doc.add_object(stream), w, h))
}

fn base_font(base: &[u8]) -> Dictionary {
    dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => Object::Name(base.to_vec()),
        "Encoding" => "WinAnsiEncoding",
    }
}

fn name(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).unwrap_or("X")
}

fn fmt(value: f64) -> String {
    format!("{value:.2}")
}

fn fmt4(value: f64) -> String {
    format!("{value:.4}")
}

/// Byte WinAnsi di un carattere; '?' se l'encoding non lo ha.
fn win_ansi(c: char) -> u8 {
    match c as u32 {
        0x20..=0x7E | 0xA0..=0xFF => c as u32 as u8,
        0x20AC => 0x80,
        0x2018 => 0x91,
        0x2019 => 0x92,
        0x201C => 0x93,
        0x201D => 0x94,
        0x2013 => 0x96,
        0x2014 => 0x97,
        _ => b'?',
    }
}

/// Stringa esadecimale WinAnsi: niente problemi con parentesi e barre nel testo.
fn hex_text(text: &str) -> String {
    let mut out = String::from("<");
    for c in text.chars() {
        let _ = write!(out, "{:02X}", win_ansi(c));
    }
    out.push('>');
    out
}

/// Larghezze AFM di Helvetica e Helvetica-Bold per i codici 32..126, in millesimi.
const HELVETICA: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556,
    1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556,
    333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];
const HELVETICA_BOLD: [u16; 95] = [
    278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 333, 333, 584, 584, 584, 611,
    975, 722, 722, 722, 722, 667, 611, 778, 722, 278, 556, 722, 611, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 333, 278, 333, 584, 556,
    333, 556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556, 278, 889, 611, 611, 611, 611, 389, 556, 333, 611, 556, 778, 556, 556, 500, 389, 280, 389, 584,
];

/// Larghezza del testo in unita' di corpo (da moltiplicare per la dimensione del font).
fn text_width(text: &str, bold: bool) -> f64 {
    let table = if bold { &HELVETICA_BOLD } else { &HELVETICA };
    let total: u32 = text
        .chars()
        .map(|c| match win_ansi(c) {
            code @ 0x20..=0x7E => u32::from(table[usize::from(code - 0x20)]),
            // Lettere accentate e simboli: larghezza media di una maiuscola.
            _ => 667,
        })
        .sum();
    f64::from(total) / 1000.0
}

//! Converte le pagine in JPEG quando non si riesce a ridurle altrimenti, con testo OCR.

use hayro::hayro_interpret::hayro_syntax::page::Page;
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::{LoadPdfError, Pdf};
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{render, PixmapSettings, RenderCache, RenderSettings};
use image::RgbImage;
use lopdf::Document;

use super::imaging::Raster;
use super::pdf_write::{write_jpeg_pdf, JpegPage, PAGE_OVERHEAD};
use super::text_layer::{add_layers, Word};
use super::{limit, limit_label, ocr, Error, Progress};

const DPIS: [u32; 8] = [200, 150, 120, 100, 85, 72, 60, 50];
const QUALITIES: [u8; 4] = [80, 60, 40, 30];
/// Risoluzione per l'OCR: piu' alta di quanto serva a vedere la pagina.
pub const OCR_DPI: u32 = 250;
/// Byte stimati per parola nello strato di testo compresso.
const BYTES_PER_WORD: usize = 12;

pub struct Rasterized {
    pub bytes: Vec<u8>,
    pub dpi: u32,
    pub has_text: bool,
}

impl Rasterized {
    pub fn note(&self) -> String {
        if self.has_text {
            format!("{} DPI, testo da OCR", self.dpi)
        } else {
            format!("{} DPI, testo non selezionabile", self.dpi)
        }
    }
}

pub fn open(data: Vec<u8>) -> Result<Pdf, Error> {
    Pdf::new(data).map_err(|e| match e {
        LoadPdfError::Decryption(_) => Error::msg("PDF protetto da password"),
        _ => Error::msg("PDF danneggiato o non leggibile"),
    })
}

/// Scende di DPI finche' il PDF sta nel limite. Con with_ocr il testo resta selezionabile.
pub fn rasterize(data: Vec<u8>, with_ocr: bool, progress: Progress) -> Result<Rasterized, Error> {
    progress(0.05, "Lettura PDF…");
    let pdf = open(data)?;
    let count = pdf.pages().len();
    if count == 0 {
        return Err(Error::msg("il PDF non ha pagine"));
    }

    let words = if with_ocr && ocr::available() { recognize_pages(&pdf, progress) } else { Vec::new() };
    let word_count: usize = words.iter().map(Vec::len).sum();

    let start = start_dpi(count);
    for (i, &dpi) in DPIS.iter().enumerate() {
        if dpi > start {
            continue;
        }
        let pct = 0.4 + 0.55 * i as f64 / DPIS.len() as f64;
        let on_page = |page: usize| progress(pct, &format!("Pagina {} di {count} a {dpi} DPI…", page + 1));
        let reserve = word_count * BYTES_PER_WORD;
        let Some(pages) = render_pages(&pdf, count, dpi, reserve, &on_page)? else { continue };
        let plain = write_jpeg_pdf(&pages).ok_or_else(|| Error::msg("JPEG non valido"))?;
        let bytes = if word_count > 0 { with_text_layer(&plain, words_view(&words))? } else { plain };
        if bytes.len() <= limit() {
            return Ok(Rasterized { bytes, dpi, has_text: word_count > 0 });
        }
    }
    Err(Error::msg(format!("troppe pagine ({count}): sotto {} non resterebbero leggibili", limit_label())))
}

fn words_view(words: &[Vec<Word>]) -> impl Iterator<Item = Vec<Word>> + '_ {
    words.iter().map(|page| {
        page.iter().map(|w| Word { text: w.text.clone(), x: w.x, y: w.y, w: w.w, h: w.h }).collect()
    })
}

/// Riconosce il testo di tutte le pagine. Una pagina che non riesce resta senza testo.
fn recognize_pages(pdf: &Pdf, progress: Progress) -> Vec<Vec<Word>> {
    let count = pdf.pages().len();
    pdf.pages()
        .iter()
        .enumerate()
        .map(|(i, page)| {
            let pct = 0.05 + 0.35 * i as f64 / count as f64;
            progress(pct, &format!("Riconoscimento testo: pagina {} di {count}…", i + 1));
            ocr::recognize(&render_rgb(page, OCR_DPI)).unwrap_or_default()
        })
        .collect()
}

/// Aggiunge lo strato di testo al PDF di soli JPEG, pagina per pagina.
pub fn with_text_layer(plain: &[u8], words: impl Iterator<Item = Vec<Word>>) -> Result<Vec<u8>, Error> {
    let fail = |_| Error::msg("scrittura del testo non riuscita");
    let mut doc = Document::load_mem(plain).map_err(|e| fail(e.to_string()))?;
    let pages: Vec<_> = doc.get_pages().values().copied().zip(words).collect();
    add_layers(&mut doc, &pages).map_err(fail)?;
    let mut out = Vec::new();
    doc.save_modern(&mut out).map_err(|e| fail(e.to_string()))?;
    Ok(out)
}

/// Pagina diritta su sfondo bianco.
pub fn render_rgb(page: &Page, dpi: u32) -> RgbImage {
    let scale = dpi as f32 / 72.0;
    let settings = PixmapSettings { x_scale: scale, y_scale: scale, bg_color: WHITE };
    let cache = RenderCache::new();
    let pixmap = render(page, &cache, &InterpreterSettings::default(), &RenderSettings::default(), &settings);
    let (width, height) = (pixmap.width() as u32, pixmap.height() as u32);
    let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
    for pixel in pixmap.data() {
        rgb.extend_from_slice(&[pixel.r, pixel.g, pixel.b]);
    }
    RgbImage::from_raw(width, height, rgb).unwrap_or_default()
}

/// Evita passate inutili sui documenti lunghi. 0,03 byte/pixel e' una stima
/// prudente per pagine A4 al livello di qualita' piu' basso.
fn start_dpi(pages: usize) -> u32 {
    let per_page = limit() as f64 / pages as f64;
    let dpi = (per_page / (0.03 * 8.27 * 11.69)).sqrt() as u32;
    dpi.clamp(DPIS[DPIS.len() - 1], DPIS[0])
}

/// Ad alti DPI meglio scendere di risoluzione che sporcare il testo.
fn min_quality(dpi: u32) -> u8 {
    match dpi {
        101.. => 60,
        72..=100 => 40,
        _ => 30,
    }
}

/// Tiene in memoria solo i JPEG, non i pixel. None se a questi DPI non ci si sta.
fn render_pages(pdf: &Pdf, count: usize, dpi: u32, reserve: usize, on_page: &dyn Fn(usize)) -> Result<Option<Vec<JpegPage>>, Error> {
    let budget = limit().saturating_sub(1024 + PAGE_OVERHEAD * count + reserve);
    let floor = min_quality(dpi);
    let mut encoded: Vec<[Vec<u8>; QUALITIES.len()]> = Vec::with_capacity(count);
    let mut sizes = Vec::with_capacity(count);
    let mut totals = [0usize; QUALITIES.len()];

    for (i, page) in pdf.pages().iter().enumerate() {
        on_page(i);
        let rgb = render_rgb(page, dpi);
        let (width, height) = rgb.dimensions();
        if width == 0 || height == 0 {
            return Err(Error::msg(format!("pagina {} non renderizzabile", i + 1)));
        }
        let image = Raster::Rgb(rgb);

        let mut per_quality: [Vec<u8>; QUALITIES.len()] = Default::default();
        let results: Vec<Option<Vec<u8>>> = std::thread::scope(|scope| {
            let handles: Vec<_> = QUALITIES
                .iter()
                .map(|&quality| {
                    let image = &image;
                    scope.spawn(move || image.encode_jpeg(quality))
                })
                .collect();
            handles.into_iter().map(|h| h.join().ok().flatten()).collect()
        });
        for (q, result) in results.into_iter().enumerate() {
            per_quality[q] = result.ok_or_else(|| Error::msg("codifica JPEG non riuscita"))?;
            totals[q] += per_quality[q].len();
        }
        encoded.push(per_quality);
        sizes.push((width as f64 * 72.0 / dpi as f64, height as f64 * 72.0 / dpi as f64));

        // Basta il livello piu' leggero per sapere che qui non ci si sta.
        if totals[QUALITIES.len() - 1] > budget {
            return Ok(None);
        }
    }

    for (q, &quality) in QUALITIES.iter().enumerate() {
        if quality >= floor && totals[q] <= budget {
            let pages = encoded
                .into_iter()
                .zip(sizes)
                .map(|(mut levels, (width_pt, height_pt))| JpegPage {
                    data: std::mem::take(&mut levels[q]),
                    width_pt,
                    height_pt,
                })
                .collect();
            return Ok(Some(pages));
        }
    }
    Ok(None)
}

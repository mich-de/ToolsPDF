use std::io::Cursor;

use image::codecs::jpeg::JpegEncoder;
use image::imageops::{self, FilterType};
use image::metadata::Orientation;
use image::{DynamicImage, ExtendedColorType, GrayImage, ImageDecoder, ImageFormat, ImageReader, RgbImage};

use super::pdf_write::{jpeg_info, write_jpeg_pdf, JpegPage};
use super::{limit, limit_label, Error, Progress};

/// Oltre questa soglia si rischia di esaurire la RAM su file enormi o malformati.
pub const MAX_PIXELS: u64 = 400_000_000;
/// Lato lungo della pagina: quello di un A4.
const PAGE_LONG_SIDE_PT: f64 = 842.0;
/// Lati lunghi da provare; 0 significa dimensione originale.
const SIDES: [u32; 10] = [0, 3508, 2480, 2000, 1654, 1240, 1000, 800, 640, 480];

const UNSUPPORTED: &str = "formato immagine non supportato: usa JPG, PNG, GIF, BMP, TIFF o WebP";
const DAMAGED: &str = "immagine danneggiata o non leggibile";

/// Immagine a un solo canale o RGB: i JPEG grigi pesano meno e le SMask lo richiedono.
pub enum Raster {
    Gray(GrayImage),
    Rgb(RgbImage),
}

impl Raster {
    /// Grigia se ogni pixel e' neutro: un JPEG a un canale pesa meno e appare identico.
    pub fn from_rgb(image: RgbImage) -> Raster {
        let neutral = image.pixels().all(|p| {
            let [r, g, b] = p.0;
            r.abs_diff(g) <= 6 && g.abs_diff(b) <= 6 && r.abs_diff(b) <= 6
        });
        if neutral { Raster::Gray(imageops::grayscale(&image)) } else { Raster::Rgb(image) }
    }

    pub fn dimensions(&self) -> (u32, u32) {
        match self {
            Raster::Gray(i) => i.dimensions(),
            Raster::Rgb(i) => i.dimensions(),
        }
    }

    /// Porta il lato lungo a side mantenendo le proporzioni.
    pub fn resized(&self, side: u32) -> Raster {
        let (w, h) = self.dimensions();
        let (nw, nh) = if h > w {
            (((w as u64 * side as u64 / h as u64) as u32).max(1), side)
        } else {
            (side, ((h as u64 * side as u64 / w as u64) as u32).max(1))
        };
        match self {
            Raster::Gray(i) => Raster::Gray(imageops::resize(i, nw, nh, FilterType::CatmullRom)),
            Raster::Rgb(i) => Raster::Rgb(imageops::resize(i, nw, nh, FilterType::CatmullRom)),
        }
    }

    pub fn encode_jpeg(&self, quality: u8) -> Option<Vec<u8>> {
        let mut out = Vec::new();
        let mut encoder = JpegEncoder::new_with_quality(&mut out, quality);
        let (w, h) = self.dimensions();
        let result = match self {
            Raster::Gray(i) => encoder.encode(i.as_raw(), w, h, ExtendedColorType::L8),
            Raster::Rgb(i) => encoder.encode(i.as_raw(), w, h, ExtendedColorType::Rgb8),
        };
        result.ok().map(|_| out)
    }
}

/// Immagine diritta e su bianco, come la vede l'utente.
pub fn decode_rgb(data: &[u8]) -> Result<RgbImage, Error> {
    let mut reader = ImageReader::new(Cursor::new(data)).with_guessed_format().map_err(|_| Error::msg(UNSUPPORTED))?;
    reader.no_limits();
    let mut decoder = reader.into_decoder().map_err(|_| Error::msg(UNSUPPORTED))?;
    let (width, height) = decoder.dimensions();
    if width as u64 * height as u64 > MAX_PIXELS {
        return Err(Error::msg("immagine troppo grande (oltre 400 megapixel)"));
    }
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut decoded = DynamicImage::from_decoder(decoder).map_err(|_| Error::msg(DAMAGED))?;
    decoded.apply_orientation(orientation);
    Ok(flatten(&decoded))
}

/// Crea un PDF di una pagina. reserve sono i byte da lasciare liberi, per esempio per il testo OCR.
pub fn compress_image(data: &[u8], reserve: usize, progress: Progress) -> Result<Vec<u8>, Error> {
    progress(0.05, "Lettura immagine…");
    let mut reader = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(|_| Error::msg(UNSUPPORTED))?;
    let format = reader.format().ok_or_else(|| Error::msg(UNSUPPORTED))?;
    reader.no_limits();
    let mut decoder = reader.into_decoder().map_err(|_| Error::msg(UNSUPPORTED))?;

    let (width, height) = decoder.dimensions();
    if width as u64 * height as u64 > MAX_PIXELS {
        return Err(Error::msg("immagine troppo grande (oltre 400 megapixel)"));
    }
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);

    // JPEG gia' piccolo e dritto: si incorpora tal quale, senza perdere qualita'.
    if format == ImageFormat::Jpeg
        && orientation == Orientation::NoTransforms
        && data.len() + 1024 + reserve <= limit()
        && jpeg_info(data).is_some_and(|i| matches!(i.components, 1 | 3))
    {
        let (w, h) = page_size(width, height);
        let page = JpegPage { data: data.to_vec(), width_pt: w, height_pt: h };
        return write_jpeg_pdf(&[page]).ok_or_else(|| Error::msg(DAMAGED));
    }

    let mut decoded = DynamicImage::from_decoder(decoder).map_err(|_| Error::msg(DAMAGED))?;
    decoded.apply_orientation(orientation);
    let source = Raster::Rgb(flatten(&decoded));
    let budget = limit().saturating_sub(1024 + reserve);

    for (i, &side) in SIDES.iter().enumerate() {
        progress(0.15 + 0.8 * i as f64 / SIDES.len() as f64, "Compressione immagine…");
        let (w, h) = source.dimensions();
        if side > 0 && w.max(h) <= side {
            continue;
        }
        let resized = (side > 0).then(|| source.resized(side));
        let current = resized.as_ref().unwrap_or(&source);
        let min_quality = if i == SIDES.len() - 1 { 25 } else { 60 };
        if let Some(encoded) = best_jpeg(current, min_quality, 90, budget) {
            let (cw, ch) = current.dimensions();
            let (pw, ph) = page_size(cw, ch);
            let page = JpegPage { data: encoded, width_pt: pw, height_pt: ph };
            return write_jpeg_pdf(&[page]).ok_or_else(|| Error::msg(DAMAGED));
        }
    }
    Err(Error::msg(format!("immagine non comprimibile sotto {}", limit_label())))
}

/// Cerca la qualita' piu' alta in [min_q, max_q] che sta nel budget.
fn best_jpeg(image: &Raster, min_q: u8, max_q: u8, budget: usize) -> Option<Vec<u8>> {
    let mut best = image.encode_jpeg(min_q).filter(|e| e.len() <= budget)?;
    let (mut lo, mut hi) = (min_q + 1, max_q);
    while lo <= hi {
        let q = (lo + hi) / 2;
        match image.encode_jpeg(q).filter(|e| e.len() <= budget) {
            Some(encoded) => {
                best = encoded;
                lo = q + 1;
            }
            None => hi = q - 1,
        }
    }
    Some(best)
}

fn page_size(w: u32, h: u32) -> (f64, f64) {
    if w >= h {
        (PAGE_LONG_SIDE_PT, PAGE_LONG_SIDE_PT * h as f64 / w as f64)
    } else {
        (PAGE_LONG_SIDE_PT * w as f64 / h as f64, PAGE_LONG_SIDE_PT)
    }
}

/// Appoggia l'immagine su bianco: il JPEG non ha trasparenza.
fn flatten(image: &DynamicImage) -> RgbImage {
    if !image.color().has_alpha() {
        return image.to_rgb8();
    }
    let rgba = image.to_rgba8();
    RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let p = rgba.get_pixel(x, y).0;
        let a = p[3] as u32;
        let over = |c: u8| ((c as u32 * a + 255 * (255 - a)) / 255) as u8;
        image::Rgb([over(p[0]), over(p[1]), over(p[2])])
    })
}

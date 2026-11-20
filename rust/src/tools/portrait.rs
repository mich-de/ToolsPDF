//! Foto tessera: sfondo staccato con U-2-Net (modello u2netp dentro l'exe, eseguito con rten),
//! bordi rifiniti con un filtro guidato, ritaglio al formato scelto con le guide ICAO.
//! Tutto in memoria: nessun file temporaneo, nessuna rete.

use std::sync::OnceLock;

use image::codecs::jpeg::{JpegEncoder, PixelDensity};
use image::imageops::{self, FilterType};
use image::{ExtendedColorType, Rgba, RgbaImage, RgbImage};
use rten::Model;
use rten_tensor::prelude::*;
use rten_tensor::NdTensor;

use super::{imaging, signature, Error};

pub use super::signature::Stroke;

static U2NETP: &[u8] = include_bytes!("../../models/u2netp.onnx");

/// Lato dell'ingresso del modello.
const NET_SIDE: usize = 320;
/// Foto piu' grandi vengono ridotte: bastano per 600 DPI e restano leggere.
const MAX_SIDE: u32 = 2400;
/// Risoluzione delle foto salvate.
pub const DPI: u32 = 600;
/// Qualita' JPEG della foto su sfondo bianco.
const JPEG_QUALITY: u8 = 95;

/// Formato di stampa: misure in mm e altezza della testa (dal mento alla sommita' dei capelli).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Format {
    pub name: &'static str,
    pub width_mm: f32,
    pub height_mm: f32,
    pub head_mm: (f32, f32),
}

/// Formati proposti nel menu; il primo e' il predefinito.
pub const FORMATS: &[Format] = &[
    Format { name: "35x45 mm UE / Italia (CIE, passaporto, patente)", width_mm: 35.0, height_mm: 45.0, head_mm: (32.0, 36.0) },
    // Gente di mare italiana: il MIT chiede foto in formato ICAO (35x45) per i titoli marittimi.
    Format { name: "35x45 mm Italia (libretto di navigazione, titoli marittimi)", width_mm: 35.0, height_mm: 45.0, head_mm: (32.0, 36.0) },
    // Autorita' Marittima di Panama: moduli dei titoli marittimi, 4x4 cm su fondo bianco.
    Format { name: "40x40 mm Panama AMP (libretto marittimo, certificati)", width_mm: 40.0, height_mm: 40.0, head_mm: (26.0, 30.0) },
    Format { name: "35x45 mm Panama (passaporto)", width_mm: 35.0, height_mm: 45.0, head_mm: (32.0, 36.0) },
    // Identity Malta: passaporto 40 mm di altezza per 30 di larghezza.
    Format { name: "30x40 mm Malta (passaporto)", width_mm: 30.0, height_mm: 40.0, head_mm: (28.0, 32.0) },
    Format { name: "35x45 mm Regno Unito", width_mm: 35.0, height_mm: 45.0, head_mm: (29.0, 34.0) },
    Format { name: "35x45 mm Australia", width_mm: 35.0, height_mm: 45.0, head_mm: (32.0, 36.0) },
    Format { name: "35x45 mm India", width_mm: 35.0, height_mm: 45.0, head_mm: (25.0, 35.0) },
    Format { name: "51x51 mm USA (2x2 pollici)", width_mm: 51.0, height_mm: 51.0, head_mm: (25.0, 35.0) },
    Format { name: "50x70 mm Canada", width_mm: 50.0, height_mm: 70.0, head_mm: (31.0, 36.0) },
    Format { name: "33x48 mm Cina", width_mm: 33.0, height_mm: 48.0, head_mm: (28.0, 33.0) },
    Format { name: "35x45 mm Giappone", width_mm: 35.0, height_mm: 45.0, head_mm: (32.0, 36.0) },
    Format { name: "40x50 mm", width_mm: 40.0, height_mm: 50.0, head_mm: (35.0, 40.0) },
    Format { name: "30x40 mm", width_mm: 30.0, height_mm: 40.0, head_mm: (28.0, 32.0) },
    Format { name: "40x40 mm", width_mm: 40.0, height_mm: 40.0, head_mm: (26.0, 30.0) },
    Format { name: "50x50 mm", width_mm: 50.0, height_mm: 50.0, head_mm: (30.0, 36.0) },
    Format { name: "35x35 mm", width_mm: 35.0, height_mm: 35.0, head_mm: (23.0, 27.0) },
    Format { name: "25x30 mm", width_mm: 25.0, height_mm: 30.0, head_mm: (21.0, 24.0) },
];

/// Testa piu' alta che larga: altezza (mento-sommita') su larghezza con i capelli.
const HEAD_RATIO: f32 = 1.3;
/// Quota dello spazio libero sopra la testa (il resto va sotto il mento).
const TOP_SHARE: f32 = 0.35;
/// Occhi a questa quota dell'altezza della testa, misurata dalla sommita'.
const EYES_SHARE: f32 = 0.45;

impl Format {
    fn head_avg(&self) -> f32 {
        (self.head_mm.0 + self.head_mm.1) / 2.0
    }

    fn top_mm(&self) -> f32 {
        (self.height_mm - self.head_avg()).max(0.0) * TOP_SHARE
    }

    /// Pixel della foto stampata a questa risoluzione.
    pub fn pixels(&self, dpi: u32) -> (u32, u32) {
        let px = |mm: f32| ((mm / 25.4 * dpi as f32).round() as u32).max(1);
        (px(self.width_mm), px(self.height_mm))
    }

    /// Guide da disegnare sul riquadro, in frazioni di larghezza e altezza della foto.
    pub fn guides(&self) -> Guides {
        let (w, h) = (self.width_mm, self.height_mm);
        let top = self.top_mm();
        let head = self.head_avg();
        Guides {
            crown: top / h,
            chin: (top + head) / h,
            eyes: (top + head * EYES_SHARE) / h,
            oval_w: head / HEAD_RATIO / w,
            head_min: self.head_mm.0 / h,
            head_max: self.head_mm.1 / h,
        }
    }
}

/// Guide ICAO in frazioni del riquadro: ovale del volto dalla sommita' al mento, linea degli occhi,
/// altezza della testa ammessa (minima e massima).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Guides {
    pub crown: f32,
    pub chin: f32,
    pub eyes: f32,
    pub oval_w: f32,
    pub head_min: f32,
    pub head_max: f32,
}

/// Sfondo della foto salvata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Background {
    White,
    Transparent,
}

/// Parte della foto che finisce nel formato: angolo in alto a sinistra e misure, in pixel della
/// foto. Puo' uscire dalla foto (li' c'e' lo sfondo).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crop {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Crop {
    /// Spostata di dx, dy pixel della foto.
    pub fn moved(&self, dx: f32, dy: f32) -> Crop {
        Crop { x: self.x + dx, y: self.y + dy, ..*self }
    }

    /// Nuova larghezza w tenendo fermo il punto alle frazioni (fx, fy) del riquadro.
    pub fn zoomed(&self, w: f32, fx: f32, fy: f32) -> Crop {
        let w = w.max(8.0);
        let h = w * self.h / self.w.max(1e-3);
        let (sx, sy) = (self.x + fx * self.w, self.y + fy * self.h);
        Crop { x: sx - fx * w, y: sy - fy * h, w, h }
    }
}

/// Foto con lo sfondo staccato: alfa = persona, colori premoltiplicati (niente aloni del vecchio
/// sfondo quando si riduce). levels[0] e' la foto intera, poi meta', un quarto... per l'anteprima.
pub struct Photo {
    source: RgbImage,
    /// Foto con la messa a fuoco attuale (la gomma non la ricalcola).
    sharp: RgbImage,
    /// Sagoma del modello, solo soggetto principale; alpha = questa piu' le passate di gomma.
    auto_alpha: Vec<f32>,
    alpha: Vec<f32>,
    /// Somiglianza di ogni pixel alla persona invece che allo sfondo (person_likeness).
    likeness: Vec<u8>,
    strokes: Vec<Stroke>,
    levels: Vec<RgbaImage>,
}

/// Messa a fuoco predefinita (0 = foto com'e', SHARPEN_MAX = massima).
pub const SHARPEN_DEFAULT: f32 = 0.8;
pub const SHARPEN_MAX: f32 = 2.0;

/// Alfa minima perche' un pixel conti come persona nei pezzi della sagoma: gli aloni deboli non
/// fanno da ponte tra la persona e un oggetto dietro.
const SUBJECT_LINK: f32 = 0.12;
/// Un pezzo staccato resta se e' grande almeno questa quota del pezzo piu' grande.
const SUBJECT_KEEP: f32 = 0.25;
/// Quota minima di un pezzo che deve stare dentro la passata della gomma magica.
const MAGIC_INSIDE: f32 = 0.8;

impl Photo {
    /// Stacca lo sfondo. Mezzo secondo circa: da fare una volta per foto, fuori dal thread UI.
    pub fn new(image: RgbImage, sharpen: f32) -> Result<Photo, Error> {
        let image = limit(image);
        let mut alpha = mask(&image).ok_or_else(|| Error::msg("impossibile staccare lo sfondo della foto"))?;
        keep_main_subject(&mut alpha, image.width() as usize, image.height() as usize);
        let likeness = person_likeness(&image, &alpha);
        drop_background_colors(&mut alpha, &likeness);
        Ok(Photo::with_alpha(image, alpha, likeness, sharpen))
    }

    fn with_alpha(source: RgbImage, alpha: Vec<f32>, likeness: Vec<u8>, sharpen: f32) -> Photo {
        let sharp = self::sharpen(&source, sharpen.clamp(0.0, SHARPEN_MAX));
        let mut photo = Photo { source, sharp, auto_alpha: alpha.clone(), alpha, likeness, strokes: Vec::new(), levels: Vec::new() };
        photo.rebuild_levels();
        photo
    }

    #[cfg(test)]
    fn from_parts(image: &RgbImage, alpha: &[f32]) -> Photo {
        Photo::with_alpha(image.clone(), alpha.to_vec(), person_likeness(image, alpha), 0.0)
    }

    /// Rifa la foto con un'altra messa a fuoco (0..=SHARPEN_MAX). Decimi di secondo: al rilascio
    /// dello slider, non a ogni movimento.
    pub fn set_sharpen(&mut self, amount: f32) {
        self.sharp = sharpen(&self.source, amount.clamp(0.0, SHARPEN_MAX));
        self.rebuild_levels();
    }

    /// Passata di gomma in pixel della foto. Magica: toglie solo i pezzi della sagoma che stanno
    /// quasi tutti dentro la passata (un oggetto dietro la spalla), la persona resta intera;
    /// normale: toglie tutto quello che tocca.
    pub fn add_stroke(&mut self, stroke: Stroke) {
        self.strokes.push(stroke);
        self.rebuild_alpha();
    }

    /// Toglie l'ultima passata. Vero se ce n'era una.
    pub fn undo_stroke(&mut self) -> bool {
        let undone = self.strokes.pop().is_some();
        if undone {
            self.rebuild_alpha();
        }
        undone
    }

    pub fn stroke_count(&self) -> usize {
        self.strokes.len()
    }

    fn rebuild_alpha(&mut self) {
        let (w, h) = (self.source.width() as usize, self.source.height() as usize);
        let mut alpha = self.auto_alpha.clone();
        for stroke in &self.strokes {
            erase(&mut alpha, w, h, stroke, &self.likeness);
        }
        self.alpha = alpha;
        self.rebuild_levels();
    }

    fn rebuild_levels(&mut self) {
        let mut levels = vec![premultiplied(&self.sharp, &self.alpha)];
        while levels.last().is_some_and(|l| l.width().min(l.height()) >= 64) {
            let half = halve(levels.last().unwrap());
            levels.push(half);
        }
        self.levels = levels;
    }

    pub fn dimensions(&self) -> (u32, u32) {
        self.levels[0].dimensions()
    }

    /// Vero se il ritaglio ha meno della meta' dei pixel della stampa: la foto uscira' sgranata.
    pub fn low_resolution(&self, crop: Crop, format: &Format) -> bool {
        crop.w < format.pixels(DPI).0 as f32 * 0.5
    }

    /// Inquadratura iniziale: testa alla misura e all'altezza del formato; senza persona, la foto
    /// intera centrata.
    pub fn fit(&self, format: &Format) -> Crop {
        let (w, h) = self.dimensions();
        match head(&self.levels[0]) {
            Some((cx, crown, head_px)) => {
                let k = head_px / format.head_avg();
                let (cw, ch) = (format.width_mm * k, format.height_mm * k);
                Crop { x: cx - cw / 2.0, y: crown - format.top_mm() * k, w: cw, h: ch }
            }
            None => cover(w, h, format),
        }
    }

    /// Foto finale out_w x out_h, ricampionata bene (Lanczos), su bianco o trasparente.
    pub fn render(&self, crop: Crop, out_w: u32, out_h: u32, background: Background) -> RgbaImage {
        let (out_w, out_h) = (out_w.max(1), out_h.max(1));
        // Il livello piu' piccolo ancora piu' grande dell'uscita: meno lavoro, stessa resa.
        let mut level = 0;
        while level + 1 < self.levels.len() && crop.w / (1u32 << (level + 1)) as f32 >= out_w as f32 {
            level += 1;
        }
        let f = (1u32 << level) as f32;
        let src = &self.levels[level];
        let (cx, cy) = ((crop.x / f).round() as i64, (crop.y / f).round() as i64);
        let (cw, ch) = (((crop.w / f).round() as u32).max(1), ((crop.h / f).round() as u32).max(1));
        let mut canvas = RgbaImage::new(cw, ch);
        imageops::replace(&mut canvas, src, -cx, -cy);
        let resized = imageops::resize(&canvas, out_w, out_h, FilterType::Lanczos3);
        finish(resized, background)
    }

    /// Anteprima BGRA dall'alto in basso box_w x box_h, veloce (bilineare sul livello giusto):
    /// sfondo bianco o scacchiera chiara (lato cell) per il trasparente.
    pub fn preview(&self, crop: Crop, box_w: u32, box_h: u32, background: Background, cell: u32) -> Vec<u8> {
        let (box_w, box_h, cell) = (box_w.max(1), box_h.max(1), cell.max(1));
        let step = crop.w / box_w as f32;
        let mut level = 0;
        while level + 1 < self.levels.len() && step / (1u32 << (level + 1)) as f32 >= 1.0 {
            level += 1;
        }
        let f = (1u32 << level) as f32;
        let src = &self.levels[level];
        let mut out = Vec::with_capacity((box_w * box_h * 4) as usize);
        for y in 0..box_h {
            let sy = (crop.y + (y as f32 + 0.5) * crop.h / box_h as f32) / f - 0.5;
            for x in 0..box_w {
                let sx = (crop.x + (x as f32 + 0.5) * step) / f - 0.5;
                let [r, g, b, a] = bilinear(src, sx, sy);
                let paper = match background {
                    Background::White => 255.0,
                    Background::Transparent if (x / cell + y / cell) % 2 == 0 => 255.0,
                    Background::Transparent => 226.0,
                };
                let under = paper * (1.0 - a / 255.0);
                let px = |c: f32| (c + under).round().clamp(0.0, 255.0) as u8;
                out.extend_from_slice(&[px(b), px(g), px(r), 255]);
            }
        }
        out
    }
}

/// PNG con trasparenza (sfondo trasparente) o JPEG (sfondo bianco), con la risoluzione scritta
/// nel file: Word e le stampanti la mettono alla misura giusta. Restituisce anche l'estensione.
pub fn encode(image: &RgbaImage, background: Background, dpi: u32) -> Result<(Vec<u8>, &'static str), Error> {
    match background {
        Background::Transparent => Ok((signature::to_png(image, dpi)?, "png")),
        Background::White => {
            let rgb: Vec<u8> = image.pixels().flat_map(|p| [p.0[0], p.0[1], p.0[2]]).collect();
            let mut out = Vec::new();
            let mut encoder = JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY);
            encoder.set_pixel_density(PixelDensity::dpi(dpi.min(u16::MAX as u32) as u16));
            encoder
                .encode(&rgb, image.width(), image.height(), ExtendedColorType::Rgb8)
                .map_err(|_| Error::msg("impossibile creare il JPEG della foto"))?;
            Ok((out, "jpg"))
        }
    }
}

/// Suffisso del file salvato: nome_fototessera.jpg / .png.
pub const SUFFIX: &str = "_fototessera";

/// Salva accanto all'originale (o nella cartella fissa) senza mai sovrascrivere.
pub fn save_beside(original: &std::path::Path, data: &[u8], ext: &str) -> Result<std::path::PathBuf, Error> {
    super::write_new(original, SUFFIX, ext, data).map_err(|e| Error::msg(&format!("impossibile scrivere la foto: {e}")))
}

/// Immagine da file (JPG, PNG, ...), raddrizzata secondo l'EXIF.
pub fn decode(data: &[u8]) -> Result<RgbImage, Error> {
    imaging::decode_rgb(data)
}

fn limit(image: RgbImage) -> RgbImage {
    let (w, h) = image.dimensions();
    let side = w.max(h);
    if side <= MAX_SIDE {
        return image;
    }
    let scale = |v: u32| ((v as u64 * MAX_SIDE as u64 / side as u64) as u32).max(1);
    imageops::resize(&image, scale(w), scale(h), FilterType::Triangle)
}

/// Tutta la foto (o quasi) nel formato, centrata.
fn cover(w: u32, h: u32, format: &Format) -> Crop {
    let (w, h) = (w as f32, h as f32);
    let ratio = format.height_mm / format.width_mm;
    let (cw, ch) = if h / w > ratio { (w, w * ratio) } else { (h / ratio, h) };
    Crop { x: (w - cw) / 2.0, y: (h - ch) / 2.0, w: cw, h: ch }
}

fn engine() -> Option<&'static Model> {
    static MODEL: OnceLock<Option<Model>> = OnceLock::new();
    MODEL.get_or_init(|| Model::load_static_slice(U2NETP).ok()).as_ref()
}

/// Alfa 0..1 per pixel: 1 = persona.
fn mask(image: &RgbImage) -> Option<Vec<f32>> {
    let model = engine()?;
    let small = imageops::resize(image, NET_SIDE as u32, NET_SIDE as u32, FilterType::Triangle);
    const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
    const STD: [f32; 3] = [0.229, 0.224, 0.225];
    let plane = NET_SIDE * NET_SIDE;
    let mut input = vec![0f32; 3 * plane];
    for (i, p) in small.pixels().enumerate() {
        for c in 0..3 {
            input[c * plane + i] = (p.0[c] as f32 / 255.0 - MEAN[c]) / STD[c];
        }
    }
    let tensor = NdTensor::from_data([1, 3, NET_SIDE, NET_SIDE], input);
    let output: NdTensor<f32, 4> = model.run_one(tensor.into(), None).ok()?.try_into().ok()?;
    let raw = output.to_vec();
    if raw.len() != plane {
        return None;
    }
    let (lo, hi) = raw.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
    // Su immagini uniformi senza soggetto u2netp produce valori quasi tutti nulli:
    // non stirare il rumore su [0, 255].
    let span = (hi - lo).max(0.5);
    let coarse = image::GrayImage::from_fn(NET_SIDE as u32, NET_SIDE as u32, |x, y| {
        let v = if hi < 0.1 { 0.0 } else { ((raw[y as usize * NET_SIDE + x as usize] - lo) / span).clamp(0.0, 1.0) };
        image::Luma([(v * 255.0).round() as u8])
    });
    let (w, h) = image.dimensions();
    let up = imageops::resize(&coarse, w, h, FilterType::Triangle);
    let p: Vec<f32> = up.as_raw().iter().map(|&v| v as f32 / 255.0).collect();
    Some(refine(image, &p))
}

/// Filtro guidato (He et al.) con la luminanza come guida: il bordo della maschera, grossolano
/// a 320 px, si appoggia ai contorni veri (capelli, spalle). Poi pulizia dei quasi 0 e quasi 1.
fn refine(image: &RgbImage, p: &[f32]) -> Vec<f32> {
    let (w, h) = (image.width() as usize, image.height() as usize);
    let r = (w.max(h) / 150).clamp(2, 16);
    const EPS: f32 = 1e-3;
    let guide: Vec<f32> = image.pixels().map(|q| (q.0[0] as f32 * 0.299 + q.0[1] as f32 * 0.587 + q.0[2] as f32 * 0.114) / 255.0).collect();
    let product = |a: &[f32], b: &[f32]| -> Vec<f32> { a.iter().zip(b).map(|(x, y)| x * y).collect() };
    let mean_i = box_mean(&guide, w, h, r);
    let mean_p = box_mean(p, w, h, r);
    let corr_ii = box_mean(&product(&guide, &guide), w, h, r);
    let corr_ip = box_mean(&product(&guide, p), w, h, r);
    let mut a = vec![0f32; w * h];
    let mut b = vec![0f32; w * h];
    for i in 0..w * h {
        let var = corr_ii[i] - mean_i[i] * mean_i[i];
        let cov = corr_ip[i] - mean_i[i] * mean_p[i];
        a[i] = cov / (var + EPS);
        b[i] = mean_p[i] - a[i] * mean_i[i];
    }
    let mean_a = box_mean(&a, w, h, r);
    let mean_b = box_mean(&b, w, h, r);
    (0..w * h)
        .map(|i| {
            let q = mean_a[i] * guide[i] + mean_b[i];
            ((q - 0.03) / 0.94).clamp(0.0, 1.0)
        })
        .collect()
}

/// Media sul quadrato (2r+1)^2 con immagine integrale; ai bordi conta solo la parte dentro.
fn box_mean(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let stride = w + 1;
    let mut sum = vec![0f64; stride * (h + 1)];
    for y in 0..h {
        let mut row = 0f64;
        for x in 0..w {
            row += src[y * w + x] as f64;
            sum[(y + 1) * stride + x + 1] = sum[y * stride + x + 1] + row;
        }
    }
    let mut out = vec![0f32; w * h];
    for y in 0..h {
        let (y0, y1) = (y.saturating_sub(r), (y + r + 1).min(h));
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(r), (x + r + 1).min(w));
            let total = sum[y1 * stride + x1] - sum[y0 * stride + x1] - sum[y1 * stride + x0] + sum[y0 * stride + x0];
            out[y * w + x] = (total / ((y1 - y0) * (x1 - x0)) as f64) as f32;
        }
    }
    out
}

/// Alfa 0..1 in byte, per cercare i pezzi della sagoma con signature::blobs.
fn alpha_bytes(alpha: &[f32]) -> Vec<u8> {
    alpha.iter().map(|a| (a * 255.0).round().clamp(0.0, 255.0) as u8).collect()
}

fn link_byte() -> u8 {
    (SUBJECT_LINK * 255.0).round() as u8
}

/// Tiene solo la persona: u2netp cerca oggetti in evidenza, non ritratti, e prende per soggetto
/// anche un oggetto contrastato dietro (un sigillo alla parete). Dei pezzi staccati della sagoma
/// restano il piu' grande e quelli grandi almeno SUBJECT_KEEP di lui; degli altri si toglie anche
/// l'alone debole intorno.
fn keep_main_subject(alpha: &mut [f32], w: usize, h: usize) {
    let pieces = signature::blobs(&alpha_bytes(alpha), w, h, link_byte());
    let Some(largest) = pieces.iter().map(|p| p.pixels.len()).max() else { return };
    let mut kept = vec![0f32; alpha.len()];
    let mut dropped = false;
    for piece in &pieces {
        let keep = piece.pixels.len() as f32 >= largest as f32 * SUBJECT_KEEP;
        dropped |= !keep;
        for &i in &piece.pixels {
            if keep {
                kept[i] = 1.0;
            } else {
                alpha[i] = 0.0;
            }
        }
    }
    if !dropped {
        return;
    }
    let near = box_mean(&kept, w, h, (w.max(h) / 400).max(3));
    for (a, n) in alpha.iter_mut().zip(near) {
        if *a < SUBJECT_LINK && n <= 0.0 {
            *a = 0.0;
        }
    }
}

/// Sotto questa distanza di colore (0..255 per canale) da entrambi i campioni piu' vicini, persona
/// e sfondo si somigliano: il colore non decide e si lascia la sagoma.
const UNMIX_CONTRAST: f32 = 24.0;
/// Somiglianza allo sfondo sotto cui la gomma magica toglie il pixel.
const MAGIC_BACKGROUND: u8 = 140;
/// Campioni per lato della finestra in cui cercare i colori vicini.
const UNMIX_SAMPLES: usize = 12;

/// Per ogni pixel di bordo, quanto il suo colore somiglia alla persona (255) piu' che allo sfondo
/// (0): distanza dal colore piu' simile tra i campioni di sfondo sicuro vicini e tra quelli di
/// persona "profonda" (lontana dallo sfondo, quindi senza aloni). Il campione piu' simile, non la
/// media: uno sfondo sfumato (piu' chiaro accanto alla persona, come il bagliore intorno alle
/// orecchie) resta sfondo. 255 dove non si puo' dire.
fn person_likeness(image: &RgbImage, alpha: &[f32]) -> Vec<u8> {
    let (w, h) = (image.width() as usize, image.height() as usize);
    let r = (w.max(h) / 25).max(8);
    let back: Vec<f32> = alpha.iter().map(|&a| if a < 0.05 { 1.0 } else { 0.0 }).collect();
    let near_back = box_mean(&back, w, h, r);
    let shell = box_mean(&back, w, h, r / 2);
    let deep: Vec<bool> = alpha.iter().zip(&shell).map(|(&a, &s)| a > 0.95 && s <= 0.0).collect();
    let step = (2 * r / UNMIX_SAMPLES).max(1) as i64;
    let p = image.as_raw();
    let dist = |i: usize, j: usize| -> f32 { (0..3).map(|c| (p[i * 3 + c] as f32 - p[j * 3 + c] as f32).powi(2)).sum::<f32>().sqrt() };
    (0..w * h)
        .map(|i| {
            if alpha[i] < 0.02 || near_back[i] <= 0.0 {
                return 255;
            }
            let (x, y) = ((i % w) as i64, (i / w) as i64);
            let (mut to_back, mut to_front) = (f32::MAX, f32::MAX);
            let mut dy = -(r as i64);
            while dy <= r as i64 {
                let mut dx = -(r as i64);
                while dx <= r as i64 {
                    let (sx, sy) = (x + dx, y + dy);
                    if sx >= 0 && sy >= 0 && (sx as usize) < w && (sy as usize) < h {
                        let j = sy as usize * w + sx as usize;
                        if back[j] > 0.0 {
                            to_back = to_back.min(dist(i, j));
                        } else if deep[j] {
                            to_front = to_front.min(dist(i, j));
                        }
                    }
                    dx += step;
                }
                dy += step;
            }
            if to_back == f32::MAX || to_front == f32::MAX || (to_back < UNMIX_CONTRAST && to_front < UNMIX_CONTRAST) {
                return 255;
            }
            (to_back / (to_back + to_front) * 255.0).round() as u8
        })
        .collect()
}

/// Toglie dalla sagoma i pixel col colore dello sfondo (aloni azzurri intorno a capelli e
/// orecchie quando la maschera del modello e' larga): l'alfa non supera la somiglianza alla
/// persona, con una rampa che lascia piena la persona vera.
fn drop_background_colors(alpha: &mut [f32], likeness: &[u8]) {
    for (a, &l) in alpha.iter_mut().zip(likeness) {
        let t = ((l as f32 / 255.0 - 0.3) / 0.45).clamp(0.0, 1.0);
        *a = a.min(t);
    }
}

/// Una passata di gomma sulla sagoma (vedi Photo::add_stroke). La magica toglie i pezzi staccati
/// quasi tutti dentro la passata e, dentro la passata, i pixel col colore dello sfondo.
fn erase(alpha: &mut [f32], w: usize, h: usize, stroke: &Stroke, likeness: &[u8]) {
    let mut brushed = vec![false; alpha.len()];
    signature::brush(&mut brushed, w, h, stroke);
    if stroke.magic {
        for piece in signature::blobs(&alpha_bytes(alpha), w, h, link_byte()) {
            let inside = piece.pixels.iter().filter(|&&i| brushed[i]).count();
            if inside > 0 && inside as f32 >= piece.pixels.len() as f32 * MAGIC_INSIDE {
                for &i in &piece.pixels {
                    alpha[i] = 0.0;
                }
            }
        }
    }
    for ((a, &b), &l) in alpha.iter_mut().zip(&brushed).zip(likeness) {
        if b && (!stroke.magic || *a < SUBJECT_LINK || l < MAGIC_BACKGROUND) {
            *a = 0.0;
        }
    }
}

/// Colori premoltiplicati per l'alfa, togliendo dai bordi semitrasparenti il colore del vecchio
/// sfondo (stimato come media dei pixel di sfondo vicini): niente alone colorato sui capelli.
fn premultiplied(image: &RgbImage, alpha: &[f32]) -> RgbaImage {
    let (w, h) = (image.width() as usize, image.height() as usize);
    let r = (w.max(h) / 50).clamp(4, 48);
    let back: Vec<f32> = alpha.iter().map(|a| 1.0 - a).collect();
    let weight = box_mean(&back, w, h, r);
    let channels: Vec<Vec<f32>> = (0..3)
        .map(|c| {
            let weighted: Vec<f32> = image.pixels().zip(&back).map(|(p, b)| p.0[c] as f32 * b).collect();
            box_mean(&weighted, w, h, r)
        })
        .collect();
    RgbaImage::from_fn(w as u32, h as u32, |x, y| {
        let i = y as usize * w + x as usize;
        let a = alpha[i];
        let p = image.get_pixel(x, y).0;
        let mut out = [0u8; 4];
        for c in 0..3 {
            let color = if a > 0.02 && a < 0.98 && weight[i] > 1e-3 {
                // I = a F + (1 - a) B: il colore della persona F, gia' moltiplicato per a.
                let background = channels[c][i] / weight[i];
                (p[c] as f32 - (1.0 - a) * background).clamp(0.0, 255.0 * a)
            } else {
                p[c] as f32 * a
            };
            out[c] = color.round() as u8;
        }
        out[3] = (a * 255.0).round() as u8;
        Rgba(out)
    })
}

/// Messa a fuoco (maschera di contrasto) sulla sola luminanza: niente aloni colorati. Le
/// differenze minime (rumore, blocchi JPEG) non vengono rinforzate. Il raggio cresce con la foto.
fn sharpen(image: &RgbImage, amount: f32) -> RgbImage {
    if amount <= 0.0 {
        return image.clone();
    }
    const NOISE: f32 = 2.0;
    let (w, h) = (image.width() as usize, image.height() as usize);
    let r = (w.max(h) / 1200).max(1);
    let luma: Vec<f32> = image.pixels().map(|p| p.0[0] as f32 * 0.299 + p.0[1] as f32 * 0.587 + p.0[2] as f32 * 0.114).collect();
    // Due medie di fila: quasi una gaussiana.
    let blur = box_mean(&box_mean(&luma, w, h, r), w, h, r);
    let mut out = image.clone();
    for (i, p) in out.pixels_mut().enumerate() {
        let d = luma[i] - blur[i];
        let d = d.signum() * (d.abs() - NOISE).max(0.0) * amount;
        p.0 = p.0.map(|c| (c as f32 + d).round().clamp(0.0, 255.0) as u8);
    }
    out
}

/// Meta' per lato, media di 2x2 (i colori premoltiplicati si possono mediare cosi').
fn halve(src: &RgbaImage) -> RgbaImage {
    let (w, h) = ((src.width() / 2).max(1), (src.height() / 2).max(1));
    RgbaImage::from_fn(w, h, |x, y| {
        let mut sum = [0u32; 4];
        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let p = src.get_pixel((2 * x + dx).min(src.width() - 1), (2 * y + dy).min(src.height() - 1)).0;
            for c in 0..4 {
                sum[c] += p[c] as u32;
            }
        }
        Rgba(sum.map(|s| ((s + 2) / 4) as u8))
    })
}

/// Pixel premoltiplicato nel punto (x, y), trasparente fuori dalla foto.
fn bilinear(src: &RgbaImage, x: f32, y: f32) -> [f32; 4] {
    let (w, h) = (src.width() as i64, src.height() as i64);
    let (x0, y0) = (x.floor() as i64, y.floor() as i64);
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let mut out = [0f32; 4];
    for (dx, dy, k) in [(0, 0, (1.0 - fx) * (1.0 - fy)), (1, 0, fx * (1.0 - fy)), (0, 1, (1.0 - fx) * fy), (1, 1, fx * fy)] {
        let (px, py) = (x0 + dx, y0 + dy);
        if px < 0 || py < 0 || px >= w || py >= h {
            continue;
        }
        let p = src.get_pixel(px as u32, py as u32).0;
        for c in 0..4 {
            out[c] += p[c] as f32 * k;
        }
    }
    out
}

/// Da premoltiplicato a finale: su bianco (opaco) o trasparente (colori normali).
fn finish(mut image: RgbaImage, background: Background) -> RgbaImage {
    for p in image.pixels_mut() {
        let [r, g, b, a] = p.0;
        p.0 = match background {
            Background::White => {
                let over = |c: u8| (c as u32 + 255 - a as u32).min(255) as u8;
                [over(r), over(g), over(b), 255]
            }
            Background::Transparent if a == 0 => [0, 0, 0, 0],
            Background::Transparent => {
                let un = |c: u8| ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
                [un(r), un(g), un(b), a]
            }
        };
    }
    image
}

/// Testa nella maschera: centro orizzontale, riga della sommita' e altezza stimata (mento-sommita').
/// La larghezza massima della parte alta e' la testa; dove la sagoma si allarga di colpo
/// iniziano le spalle.
fn head(image: &RgbaImage) -> Option<(f32, f32, f32)> {
    let (w, h) = image.dimensions();
    let min_w = (w / 100).max(2);
    let span = |y: u32| -> Option<(u32, u32)> {
        let row = (0..w).filter(|&x| image.get_pixel(x, y).0[3] >= 128);
        let (mut lo, mut hi) = (u32::MAX, 0);
        for x in row {
            lo = lo.min(x);
            hi = hi.max(x);
        }
        (lo <= hi && hi - lo + 1 >= min_w).then_some((lo, hi))
    };
    let crown = (0..h).find(|&y| span(y).is_some())?;
    let (mut widest, mut cx) = (0u32, 0f32);
    let mut shoulders = None;
    for y in crown..h {
        let Some((lo, hi)) = span(y) else { continue };
        let width = hi - lo + 1;
        let dy = (y - crown) as f32;
        if widest > 0 && dy > 0.8 * widest as f32 && width as f32 > 1.4 * widest as f32 {
            shoulders = Some(y);
            break;
        }
        if (widest == 0 || dy < 0.75 * widest as f32) && width > widest {
            widest = width;
            cx = (lo + hi) as f32 / 2.0;
        }
    }
    if widest < min_w * 2 {
        return None;
    }
    let mut height = widest as f32 * HEAD_RATIO;
    if let Some(s) = shoulders {
        height = height.min((s - crown) as f32);
    }
    Some((cx, crown as f32, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sagoma: testa ellittica 100x130 con la sommita' a y=65, collo, spalle larghe.
    fn silhouette() -> (RgbImage, Vec<f32>) {
        let (w, h) = (200u32, 320u32);
        let mut alpha = vec![0f32; (w * h) as usize];
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = ((x as f32 - 100.0) / 50.0, (y as f32 - 130.0) / 65.0);
                let in_head = fx * fx + fy * fy <= 1.0;
                let in_neck = (80..120).contains(&x) && (180..230).contains(&y);
                let in_body = (10..190).contains(&x) && y >= 225;
                if in_head || in_neck || in_body {
                    alpha[(y * w + x) as usize] = 1.0;
                }
            }
        }
        let image = RgbImage::from_fn(w, h, |x, y| if alpha[(y * w + x) as usize] > 0.5 { image::Rgb([180, 120, 90]) } else { image::Rgb([40, 90, 200]) });
        (image, alpha)
    }

    /// Sigillo staccato dietro la spalla destra della sagoma: disco pieno con un alone debole.
    fn add_seal(alpha: &mut [f32]) {
        for y in 0..320usize {
            for x in 0..200usize {
                let d = (x as f32 - 175.0).hypot(y as f32 - 40.0);
                if d <= 15.0 {
                    alpha[y * 200 + x] = 1.0;
                } else if d <= 18.0 {
                    alpha[y * 200 + x] = 0.05;
                }
            }
        }
    }

    #[test]
    fn a_light_glow_of_the_background_inside_a_loose_mask_goes_away() {
        // Persona 40..80 x 30..120; sfondo blu che schiarisce accanto a lei (bagliore azzurro);
        // la maschera prende anche i 4 px di bagliore piu' vicini.
        let (w, h) = (120u32, 120u32);
        let distance = |x: u32, y: u32| {
            let dx = if x < 40 { 40 - x } else { x.saturating_sub(79) };
            let dy = 30u32.saturating_sub(y);
            dx.max(dy)
        };
        let image = RgbImage::from_fn(w, h, |x, y| match distance(x, y) {
            0 => image::Rgb([150, 90, 70]),
            1..=10 => image::Rgb([190, 225, 250]),
            _ => image::Rgb([40, 110, 210]),
        });
        let mut alpha: Vec<f32> = (0..w * h).map(|i| if distance(i % w, i / w) <= 4 { 1.0 } else { 0.0 }).collect();
        let likeness = person_likeness(&image, &alpha);
        drop_background_colors(&mut alpha, &likeness);
        assert!(alpha[(60 * w + 37) as usize] < 0.05, "bagliore tolto");
        assert!(alpha[(60 * w + 42) as usize] > 0.99 && alpha[(100 * w + 60) as usize] > 0.99, "persona intera");
    }

    #[test]
    fn a_detached_object_behind_the_person_is_dropped_with_its_halo() {
        let (_, person) = silhouette();
        let mut alpha = person.clone();
        add_seal(&mut alpha);
        keep_main_subject(&mut alpha, 200, 320);
        assert!(alpha == person);
    }

    #[test]
    fn magic_eraser_takes_whole_pieces_and_never_cuts_the_person_the_plain_one_does() {
        let (image, person) = silhouette();
        let mut alpha = person.clone();
        add_seal(&mut alpha);
        let mut photo = Photo::from_parts(&image, &alpha);
        // Dal sigillo fin dentro la testa: il sigillo sparisce, la persona resta intera.
        photo.add_stroke(Stroke { points: vec![(175.0, 20.0), (140.0, 90.0)], radius: 30.0, magic: true });
        assert!(photo.alpha == person);
        assert_eq!(photo.stroke_count(), 1);
        photo.add_stroke(Stroke { points: vec![(100.0, 130.0)], radius: 10.0, magic: false });
        assert_eq!(photo.alpha[130 * 200 + 100], 0.0);
        assert!(photo.undo_stroke());
        assert!(photo.alpha == person);
        assert!(photo.undo_stroke());
        assert!(photo.alpha == alpha);
        assert!(!photo.undo_stroke());
    }

    #[test]
    fn formats_have_sane_guides_and_pixels() {
        for f in FORMATS {
            let g = f.guides();
            assert!(g.crown > 0.0 && g.crown < g.eyes && g.eyes < g.chin && g.chin < 1.0, "{}", f.name);
            assert!(g.head_min <= g.head_max && g.oval_w < 1.0, "{}", f.name);
        }
        assert_eq!(FORMATS[0].pixels(DPI), (827, 1063));
    }

    #[test]
    fn head_is_found_in_the_silhouette_and_fit_puts_it_on_the_guides() {
        let (image, alpha) = silhouette();
        let photo = Photo::from_parts(&image, &alpha);
        let (cx, crown, height) = head(&photo.levels[0]).unwrap();
        assert!((cx - 100.0).abs() <= 2.0, "centro {cx}");
        assert!((crown - 65.0).abs() <= 2.0, "sommita' {crown}");
        assert!((height - 130.0).abs() <= 8.0, "altezza {height}");

        let format = &FORMATS[0];
        let crop = photo.fit(format);
        let g = format.guides();
        assert!(((crop.x + crop.w / 2.0) - 100.0).abs() <= 2.0);
        assert!(((65.0 - crop.y) / crop.h - g.crown).abs() < 0.02);
        assert!((crop.h / crop.w - 45.0 / 35.0).abs() < 1e-3);
    }

    #[test]
    fn render_white_is_opaque_and_transparent_keeps_the_background_out() {
        let (image, alpha) = silhouette();
        let photo = Photo::from_parts(&image, &alpha);
        let crop = Crop { x: -20.0, y: 0.0, w: 240.0, h: 320.0 };
        let white = photo.render(crop, 120, 160, Background::White);
        assert_eq!(white.dimensions(), (120, 160));
        assert!(white.pixels().all(|p| p.0[3] == 255));
        assert_eq!(white.get_pixel(2, 2).0, [255, 255, 255, 255], "fuori dalla foto: bianco");
        assert!(white.get_pixel(60, 70).0[2] < 140, "volto conservato");

        let clear = photo.render(crop, 120, 160, Background::Transparent);
        assert_eq!(clear.get_pixel(12, 20).0[3], 0, "vecchio sfondo tolto");
        let face = clear.get_pixel(60, 70).0;
        assert!(face[3] > 250 && (face[0] as i32 - 180).abs() < 6, "volto {face:?}");
    }

    #[test]
    fn edge_colors_lose_the_old_background() {
        // Bordo semitrasparente: 50% persona arancione, 50% sfondo blu.
        let (w, h) = (64u32, 64u32);
        let mut alpha = vec![0f32; (w * h) as usize];
        let image = RgbImage::from_fn(w, h, |x, y| {
            let a = if x < 30 { 1.0 } else if x == 30 { 0.5 } else { 0.0 };
            alpha[(y * w + x) as usize] = a;
            let mix = |f: f32, b: f32| (a * f + (1.0 - a) * b) as u8;
            image::Rgb([mix(220.0, 30.0), mix(140.0, 60.0), mix(60.0, 220.0)])
        });
        let photo = Photo::from_parts(&image, &alpha);
        let p = finish(photo.levels[0].clone(), Background::Transparent).get_pixel(30, 32).0;
        assert!((p[0] as i32 - 220).abs() < 12 && (p[2] as i32 - 60).abs() < 12, "colore del bordo {p:?}");
    }

    #[test]
    fn sharpen_raises_edge_contrast_and_leaves_flat_areas_alone() {
        // Bordo sfumato tra grigio scuro e chiaro.
        let image = RgbImage::from_fn(40, 10, |x, _| {
            let v = (60 + (x.saturating_sub(16)).min(8) * 15) as u8;
            image::Rgb([v, v, v])
        });
        let sharp = sharpen(&image, 1.5);
        assert_eq!(sharp.get_pixel(3, 5), image.get_pixel(3, 5), "zona piatta invariata");
        let contrast = |img: &RgbImage| img.get_pixel(24, 5).0[0] as i32 - img.get_pixel(16, 5).0[0] as i32;
        assert!(contrast(&sharp) > contrast(&image), "bordo piu' netto");
        assert_eq!(sharpen(&image, 0.0), image);
    }

    #[test]
    fn crop_zoom_keeps_the_point_under_the_mouse() {
        let crop = Crop { x: 10.0, y: 20.0, w: 100.0, h: 128.0 };
        let z = crop.zoomed(50.0, 0.25, 0.5);
        assert!((z.x + 0.25 * z.w - (crop.x + 0.25 * crop.w)).abs() < 1e-3);
        assert!((z.y + 0.5 * z.h - (crop.y + 0.5 * crop.h)).abs() < 1e-3);
        assert!((z.h / z.w - 1.28).abs() < 1e-4);
    }

    #[test]
    fn encode_writes_dpi_and_the_right_format() {
        let image = RgbaImage::from_pixel(35, 45, Rgba([200, 150, 120, 255]));
        let (png, ext) = encode(&image, Background::Transparent, 600).unwrap();
        assert_eq!((ext, signature::png_dpi(&png)), ("png", Some(600)));
        let (jpg, ext) = encode(&image, Background::White, 600).unwrap();
        assert_eq!(ext, "jpg");
        assert_eq!(&jpg[..2], &[0xFF, 0xD8]);
        // JFIF: unita' 1 (pollici), densita' 600 x 600.
        assert_eq!(&jpg[6..11], b"JFIF\0");
        assert_eq!(&jpg[13..18], &[1, 0x02, 0x58, 0x02, 0x58]);
    }

    /// Prova a mano su una foto vera: PORTRAIT_IN=foto.jpg PORTRAIT_OUT=cartella
    /// cargo test real_photo -- --ignored. Scrive bianco, trasparente e maschera.
    #[test]
    #[ignore]
    fn real_photo() {
        let input = std::env::var("PORTRAIT_IN").expect("PORTRAIT_IN");
        let out = std::path::PathBuf::from(std::env::var("PORTRAIT_OUT").expect("PORTRAIT_OUT"));
        let image = decode(&std::fs::read(&input).unwrap()).unwrap();
        let start = std::time::Instant::now();
        let photo = Photo::new(image, SHARPEN_DEFAULT).unwrap();
        eprintln!("maschera in {:?}, foto {:?}", start.elapsed(), photo.dimensions());
        let format = &FORMATS[0];
        let crop = photo.fit(format);
        eprintln!("ritaglio {crop:?}, sgranata {}", photo.low_resolution(crop, format));
        let (w, h) = format.pixels(DPI);
        for (background, name) in [(Background::White, "bianco"), (Background::Transparent, "trasparente")] {
            let image = photo.render(crop, w, h, background);
            let (data, ext) = encode(&image, background, DPI).unwrap();
            std::fs::write(out.join(format!("{name}.{ext}")), data).unwrap();
        }
        let alpha = image::GrayImage::from_fn(photo.levels[0].width(), photo.levels[0].height(), |x, y| image::Luma([photo.levels[0].get_pixel(x, y).0[3]]));
        alpha.save(out.join("maschera.png")).unwrap();
    }

    #[test]
    fn model_runs_and_separates_a_figure_from_a_flat_background() {
        let (image, _) = silhouette();
        let alpha = mask(&image).expect("modello u2netp");
        assert_eq!(alpha.len(), (image.width() * image.height()) as usize);
    }
}

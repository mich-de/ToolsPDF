//! Firme: da una foto o da uno screenshot a una firma nera su sfondo trasparente.
//! Il colore e' sempre nero (0,0,0); l'alfa e' l'intensita' dell'inchiostro, cosi' i bordi
//! restano morbidi. La pulizia lavora in memoria; le firme salvate stanno in Firme\ accanto
//! all'exe, altrimenti in %APPDATA%\ToolsPDF\Firme. Mai nelle cartelle temporanee.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use image::codecs::png::PngEncoder;
use image::imageops::{self, FilterType};
use image::{ExtendedColorType, GrayImage, ImageEncoder, Luma, Rgba, RgbaImage, RgbImage};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;

use super::{imaging, location, write_new, Error};
use crate::settings;

/// Escursione dello slider "piu' chiara / piu' scura".
pub const DARKNESS_RANGE: i32 = 50;
/// Risoluzione di una firma presa dallo schermo: un pixel = 1/96 di pollice.
pub const BASE_DPI: u32 = 96;
/// Lato massimo della firma migliorata per la stampa.
const PRINT_SIDE: u32 = 2400;
/// Lunghezza massima della nota nel nome del file.
pub const NOTE_MAX: usize = 60;

/// Scelte dell'anteprima.
#[derive(Clone, Copy, PartialEq, Default)]
pub struct Options {
    pub darkness: i32,
    /// Toglie righe dritte, linee a puntini e polvere del modulo (il testo stampato no).
    pub remove_print: bool,
    /// Piu' pixel e bordi lisci: vedi for_print.
    pub for_print: bool,
    /// Tratto nero pieno, niente grigi: vedi solid_ink.
    pub solid: bool,
}

/// Passata di gomma: punti e raggio in pixel della sorgente.
/// magic: toglie solo i pezzi d'inchiostro che stanno quasi tutti dentro la passata, interi;
/// i tratti che escono dalla passata (la firma) restano intatti.
#[derive(Clone, PartialEq, Debug)]
pub struct Stroke {
    pub points: Vec<(f32, f32)>,
    pub radius: f32,
    pub magic: bool,
}

/// Quota minima di un pezzo che deve stare dentro la passata della gomma magica.
const MAGIC_INSIDE: f64 = 0.8;
/// Alfa minima per unire i pixel in pezzi nella gomma magica: gli aloni chiari non fanno da ponte.
const MAGIC_LINK: u8 = 64;

/// Firma pronta: immagine, DPI da scrivere nel PNG e posizione nella sorgente
/// (pixel della firma = origin + pixel / scale), per l'inquadratura della gomma.
#[derive(Clone)]
pub struct Prepared {
    pub image: RgbaImage,
    pub dpi: u32,
    pub origin: (u32, u32),
    pub scale: u32,
}

impl Prepared {
    /// Riquadro della firma nella sorgente: x, y, larghezza, altezza.
    pub fn source_rect(&self) -> (f32, f32, f32, f32) {
        let k = self.scale.max(1) as f32;
        (self.origin.0 as f32, self.origin.1 as f32, self.image.width() as f32 / k, self.image.height() as f32 / k)
    }

    /// Alfa nel punto della sorgente (0 fuori dalla firma), media sul quadrato di lato step.
    fn alpha_at(&self, sx: f32, sy: f32, step: f32) -> u32 {
        let k = self.scale.max(1) as f32;
        let (w, h) = self.image.dimensions();
        let (px, py) = ((sx - self.origin.0 as f32) * k, (sy - self.origin.1 as f32) * k);
        let span = step * k;
        let n = (span.ceil() as u32).clamp(1, 4);
        let mut total = 0u32;
        for j in 0..n {
            for i in 0..n {
                let (x, y) = (px + span * (i as f32 + 0.5) / n as f32, py + span * (j as f32 + 0.5) / n as f32);
                if x >= 0.0 && y >= 0.0 && (x as u32) < w && (y as u32) < h {
                    total += self.image.get_pixel(x as u32, y as u32).0[3] as u32;
                }
            }
        }
        total / (n * n)
    }
}

/// Inquadratura dell'anteprima ingrandita: punto della sorgente nell'angolo in alto a sinistra
/// e pixel di sorgente per pixel di schermo.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct View {
    pub x: f32,
    pub y: f32,
    pub k: f32,
}

impl View {
    /// Tutta la firma nel riquadro, centrata, con un po' di margine.
    pub fn fit(prepared: &Prepared, box_w: u32, box_h: u32) -> View {
        let (x, y, w, h) = prepared.source_rect();
        let (box_w, box_h) = (box_w.max(1) as f32, box_h.max(1) as f32);
        let pad = (box_w.min(box_h) / 12.0).max(2.0);
        let k = (w / (box_w - 2.0 * pad).max(1.0)).max(h / (box_h - 2.0 * pad).max(1.0)).max(0.01);
        View { x: x + w / 2.0 - box_w * k / 2.0, y: y + h / 2.0 - box_h * k / 2.0, k }
    }

    /// Punto della sorgente sotto il pixel (bx, by) del riquadro.
    pub fn to_source(&self, bx: f32, by: f32) -> (f32, f32) {
        (self.x + bx * self.k, self.y + by * self.k)
    }

    /// Nuova scala tenendo fermo il punto sotto (bx, by).
    pub fn zoom_at(&self, k: f32, bx: f32, by: f32) -> View {
        let (sx, sy) = self.to_source(bx, by);
        View { x: sx - bx * k, y: sy - by * k, k }
    }
}

/// Anteprima BGRA come preview, ma con l'inquadratura indicata (zoom della gomma).
pub fn preview_view(prepared: &Prepared, view: View, box_w: u32, box_h: u32, cell: u32) -> Vec<u8> {
    let (box_w, box_h, cell) = (box_w.max(1), box_h.max(1), cell.max(1));
    let mut out = Vec::with_capacity((box_w * box_h * 4) as usize);
    for y in 0..box_h {
        for x in 0..box_w {
            let paper: u32 = if (x / cell + y / cell) % 2 == 0 { 255 } else { 226 };
            let (sx, sy) = view.to_source(x as f32, y as f32);
            let a = prepared.alpha_at(sx, sy, view.k);
            let v = (paper * (255 - a) / 255) as u8;
            out.extend_from_slice(&[v, v, v, 255]);
        }
    }
    out
}

/// Scritte stampate del modulo (trovate con l'OCR) come passate di gomma magica: i tratti della
/// firma che le attraversano restano interi. Lento (mezzo secondo): da fare una volta per immagine.
pub fn printed_strokes(image: &RgbImage) -> Vec<Stroke> {
    super::form_text::printed_boxes(image)
        .into_iter()
        .map(|[x0, y0, x1, y1]| {
            let (cy, h) = ((y0 + y1) / 2.0, (y1 - y0).max(1.0));
            Stroke { points: vec![(x0, cy), (x1, cy)], radius: h * 0.65, magic: true }
        })
        .collect()
}

/// Pulizia completa secondo le scelte, con le passate di gomma.
pub fn prepare(image: &RgbImage, options: Options, strokes: &[Stroke]) -> Result<Prepared, Error> {
    let (cleaned, origin) = clean_erased(image, options.darkness, options.remove_print, strokes)?;
    let (image, dpi, scale) = if options.for_print {
        let (image, dpi) = for_print(&cleaned, options.solid);
        let scale = image.width() / cleaned.width().max(1);
        (image, dpi, scale.max(1))
    } else if options.solid {
        let image = RgbaImage::from_fn(cleaned.width(), cleaned.height(), |x, y| Rgba([0, 0, 0, solid_ink(cleaned.get_pixel(x, y).0[3])]));
        (image, BASE_DPI, 1)
    } else {
        (cleaned, BASE_DPI, 1)
    };
    Ok(Prepared { image, dpi, origin, scale })
}

/// Toglie l'inchiostro sotto le passate di gomma: dischi lungo ogni tratto. La gomma magica
/// toglie solo i pezzi interi che stanno quasi tutti dentro la passata.
fn apply_strokes(alpha: &mut [u8], w: usize, h: usize, strokes: &[Stroke]) {
    for stroke in strokes {
        let mut brushed = vec![false; alpha.len()];
        brush(&mut brushed, w, h, stroke);
        if stroke.magic {
            magic_erase(alpha, w, h, &brushed);
        } else {
            for (a, &b) in alpha.iter_mut().zip(&brushed) {
                if b {
                    *a = 0;
                }
            }
        }
    }
}

/// Gomma magica: via i pezzi con almeno MAGIC_INSIDE dei pixel dentro la passata, piu' il loro
/// alone chiaro (2 px) dentro la passata. Dai pezzi che escono (lettere attraversate dalla firma)
/// toglie solo quello che non e' prolungamento dei tratti che entrano: vedi split_mixed.
fn magic_erase(alpha: &mut [u8], w: usize, h: usize, brushed: &[bool]) {
    let mut erased = vec![false; alpha.len()];
    for blob in blobs(alpha, w, h, MAGIC_LINK) {
        let inside = blob.pixels.iter().filter(|&&i| brushed[i]).count();
        if inside == 0 {
            continue;
        }
        let take = if inside as f64 >= blob.pixels.len() as f64 * MAGIC_INSIDE { blob.pixels.clone() } else { split_mixed(&blob, w, brushed) };
        for i in take {
            alpha[i] = 0;
            erased[i] = true;
        }
    }
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if alpha[i] == 0 || alpha[i] >= MAGIC_LINK || !brushed[i] {
                continue;
            }
            let near = (y.saturating_sub(2)..(y + 3).min(h)).any(|ny| (x.saturating_sub(2)..(x + 3).min(w)).any(|nx| erased[ny * w + nx]));
            if near {
                alpha[i] = 0;
            }
        }
    }
}

/// Pezzo misto: quota minima dell'inchiostro dentro la passata che non e' prolungamento dei
/// tratti entranti, perche' si tolga qualcosa (sotto: la passata attraversa solo la firma).
const MAGIC_RESIDUE: f64 = 0.3;
/// Lunghezza massima del prolungamento protetto di un tratto dentro la passata, in pixel.
const MAGIC_TUBE: f32 = 40.0;
/// Passi fuori dalla passata per stimare la direzione di un tratto entrante; un tratto piu'
/// corto fuori dalla passata non e' firma ma coda di una lettera.
const MAGIC_REACH: u32 = 8;

/// Pezzo che esce dalla passata: ogni tratto che entra nella passata viene prolungato nella sua
/// direzione (un tubo largo quanto il tratto, finche' c'e' inchiostro) e protetto; il resto
/// dell'inchiostro dentro la passata (lettere stampate) viene restituito da togliere, ma solo
/// se e' almeno MAGIC_RESIDUE. Pixel globali.
fn split_mixed(blob: &Blob, w: usize, brushed: &[bool]) -> Vec<usize> {
    let (bx, by, bw, bh) = (blob.x0, blob.y0, blob.width(), blob.height());
    let mut ink = vec![false; bw * bh];
    let mut inside = vec![false; bw * bh];
    for &i in &blob.pixels {
        let l = (i / w - by) * bw + (i % w - bx);
        ink[l] = true;
        inside[l] = brushed[i];
    }
    let at = |x: i64, y: i64| -> Option<usize> { (x >= 0 && y >= 0 && (x as usize) < bw && (y as usize) < bh).then(|| y as usize * bw + x as usize) };
    let around = |l: usize| {
        let (x, y) = ((l % bw) as i64, (l / bw) as i64);
        (-1i64..=1).flat_map(move |dy| (-1i64..=1).map(move |dx| (x + dx, y + dy))).filter(move |&(nx, ny)| (nx, ny) != (x, y))
    };
    let outside = |n: usize| ink[n] && !inside[n];
    // Entrate: pixel dentro la passata accanto a inchiostro fuori.
    let entry: Vec<bool> = (0..bw * bh).map(|l| inside[l] && around(l).any(|(nx, ny)| at(nx, ny).is_some_and(outside))).collect();

    let mut protect = vec![false; bw * bh];
    let mut seen = vec![false; bw * bh];
    for start in 0..bw * bh {
        if !entry[start] || seen[start] {
            continue;
        }
        // Sezione del tratto: entrate collegate tra loro.
        let mut group = Vec::new();
        let mut stack = vec![start];
        seen[start] = true;
        while let Some(l) = stack.pop() {
            group.push(l);
            for (nx, ny) in around(l) {
                if let Some(n) = at(nx, ny) {
                    if entry[n] && !seen[n] {
                        seen[n] = true;
                        stack.push(n);
                    }
                }
            }
        }
        // Inchiostro fuori dalla passata vicino all'entrata (fino a MAGIC_REACH passi): da dove arriva il tratto.
        let mut depth = vec![u32::MAX; bw * bh];
        let mut queue = std::collections::VecDeque::new();
        for &l in &group {
            for (nx, ny) in around(l) {
                if let Some(n) = at(nx, ny).filter(|&n| outside(n) && depth[n] == u32::MAX) {
                    depth[n] = 1;
                    queue.push_back(n);
                }
            }
        }
        let mut samples = Vec::new();
        while let Some(l) = queue.pop_front() {
            samples.push(l);
            if depth[l] >= MAGIC_REACH {
                continue;
            }
            for (nx, ny) in around(l) {
                if let Some(n) = at(nx, ny).filter(|&n| outside(n) && depth[n] == u32::MAX) {
                    depth[n] = depth[l] + 1;
                    queue.push_back(n);
                }
            }
        }
        // Solo tratti lunghi fuori dalla passata: le code delle lettere che escono di poco no.
        if samples.len() < 3 || samples.iter().all(|&l| depth[l] < MAGIC_REACH) {
            continue;
        }
        let mean = |list: &[usize]| {
            let n = list.len() as f32;
            (list.iter().map(|&l| (l % bw) as f32).sum::<f32>() / n, list.iter().map(|&l| (l / bw) as f32).sum::<f32>() / n)
        };
        let ((cx, cy), (sx, sy)) = (mean(&group), mean(&samples));
        let (dx, dy) = (cx - sx, cy - sy);
        let len = dx.hypot(dy);
        if len < 0.5 {
            continue;
        }
        let (dx, dy) = (dx / len, dy / len);
        let half = (group.len() as f32 / 2.0).clamp(1.0, 3.0) + 0.5;
        let reach = half.ceil() as i64;
        let mut misses = 0;
        let mut t = 0.0f32;
        while t <= MAGIC_TUBE {
            let (px, py) = ((cx + dx * t).round() as i64, (cy + dy * t).round() as i64);
            if at(px, py).is_some_and(|c| ink[c]) {
                misses = 0;
            } else {
                misses += 1;
                if misses > 2 {
                    break;
                }
            }
            for oy in -reach..=reach {
                for ox in -reach..=reach {
                    if (ox * ox + oy * oy) as f32 <= half * half {
                        if let Some(n) = at(px + ox, py + oy).filter(|&n| inside[n]) {
                            protect[n] = true;
                        }
                    }
                }
            }
            t += 0.5;
        }
    }
    let in_count = inside.iter().filter(|&&b| b).count();
    let residue: Vec<usize> = (0..bw * bh).filter(|&l| inside[l] && !protect[l]).collect();
    if (residue.len() as f64) < in_count as f64 * MAGIC_RESIDUE {
        return Vec::new();
    }
    residue.into_iter().map(|l| (by + l / bw) * w + bx + l % bw).collect()
}

/// Segna i pixel coperti dalla passata: dischi ravvicinati lungo il tratto.
fn brush(brushed: &mut [bool], w: usize, h: usize, stroke: &Stroke) {
    let r = stroke.radius.max(0.5);
    let mut stamp = |cx: f32, cy: f32| {
        let (x0, x1) = (((cx - r).floor().max(0.0)) as usize, ((cx + r).ceil().max(0.0) as usize).min(w.saturating_sub(1)));
        let (y0, y1) = (((cy - r).floor().max(0.0)) as usize, ((cy + r).ceil().max(0.0) as usize).min(h.saturating_sub(1)));
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (dx, dy) = (x as f32 - cx, y as f32 - cy);
                if dx * dx + dy * dy <= r * r && y < h && x < w {
                    brushed[y * w + x] = true;
                }
            }
        }
    };
    let mut last: Option<(f32, f32)> = None;
    for &(x, y) in &stroke.points {
        if let Some((lx, ly)) = last {
            let steps = (((x - lx).hypot(y - ly) / (r / 2.0)).ceil() as usize).max(1);
            for k in 1..=steps {
                let t = k as f32 / steps as f32;
                stamp(lx + (x - lx) * t, ly + (y - ly) * t);
            }
        } else {
            stamp(x, y);
        }
        last = Some((x, y));
    }
}

const NO_SIGNATURE: &str = "nessuna firma trovata: seleziona una firma scura su fondo chiaro";
const DARK_BACKGROUND: &str = "sfondo troppo scuro: serve una firma scura su fondo chiaro";
/// Differenza minima di luminosita' tra carta e inchiostro.
const MIN_CONTRAST: i32 = 48;
/// Sotto questa luminosita' la "carta" non e' carta.
const MIN_PAPER: i32 = 110;
/// Oltre questa quota di pixel scuri non e' una firma ma una foto o un fondo scuro.
const MAX_INK_SHARE: f64 = 0.6;

/// Immagine da file (PNG, JPG, ...), raddrizzata e appoggiata su bianco.
pub fn decode(data: &[u8]) -> Result<RgbImage, Error> {
    imaging::decode_rgb(data)
}

/// Da una DIB 32 bit BGRA dall'alto in basso (copia dello schermo) a RGB.
pub fn from_bgra(width: u32, height: u32, bgra: &[u8]) -> Option<RgbImage> {
    let needed = width as usize * height as usize * 4;
    if width == 0 || height == 0 || bgra.len() < needed {
        return None;
    }
    let rgb = bgra[..needed].chunks_exact(4).flat_map(|p| [p[2], p[1], p[0]]).collect();
    RgbImage::from_raw(width, height, rgb)
}

/// Pulisce la firma. darkness va da -DARKNESS_RANGE (piu' chiara) a +DARKNESS_RANGE (piu' scura).
/// Con remove_print toglie anche righe dritte, linee a puntini e polvere del modulo.
#[cfg(test)]
pub fn clean(image: &RgbImage, darkness: i32, remove_print: bool) -> Result<RgbaImage, Error> {
    clean_erased(image, darkness, remove_print, &[]).map(|(image, _)| image)
}

/// Come clean, con le passate di gomma; restituisce anche l'angolo del ritaglio nella sorgente.
fn clean_erased(image: &RgbImage, darkness: i32, remove_print: bool, strokes: &[Stroke]) -> Result<(RgbaImage, (u32, u32)), Error> {
    let (w, h) = image.dimensions();
    if w == 0 || h == 0 {
        return Err(Error::msg(NO_SIGNATURE));
    }
    let luma: Vec<u8> = image.pixels().map(|p| luminance(p.0)).collect();
    let mut histogram = [0u32; 256];
    for &l in &luma {
        histogram[l as usize] += 1;
    }
    let total = luma.len() as f64;
    let paper = percentile(&histogram, 0.90) as i32;
    let ink = percentile(&histogram, 0.005) as i32;
    if paper < MIN_PAPER {
        return Err(Error::msg(DARK_BACKGROUND));
    }
    if paper - ink < MIN_CONTRAST {
        return Err(Error::msg(NO_SIGNATURE));
    }
    let threshold = otsu(&histogram, paper as usize).clamp(ink + 1, paper - 1);

    // Rampa dell'alfa: pieno sotto lo, nullo sopra hi. Lo slider sposta entrambi.
    let shift = darkness.clamp(-DARKNESS_RANGE, DARKNESS_RANGE) * (paper - ink) / (4 * DARKNESS_RANGE);
    let hi = (threshold + (paper - threshold) / 2 + shift).clamp(1, 255);
    let lo = (threshold - (threshold - ink) / 2 + shift).clamp(0, hi - 1);
    let mut alpha: Vec<u8> = luma
        .iter()
        .map(|&l| {
            let l = l as i32;
            if l <= lo {
                255
            } else if l >= hi {
                0
            } else {
                ((hi - l) * 255 / (hi - lo)) as u8
            }
        })
        .collect();

    let dark = alpha.iter().filter(|&&a| a >= 128).count() as f64;
    if dark / total > MAX_INK_SHARE {
        return Err(Error::msg(DARK_BACKGROUND));
    }
    apply_strokes(&mut alpha, w as usize, h as usize, strokes);
    if remove_print {
        remove_lines(&mut alpha, w as usize, h as usize);
        remove_dotted_lines(&mut alpha, w as usize, h as usize);
        remove_noise(&mut alpha, w as usize, h as usize);
    }
    remove_specks(&mut alpha, w as usize, h as usize);

    let Some((x0, y0, x1, y1)) = ink_box(&alpha, w as usize, h as usize) else {
        return Err(Error::msg(NO_SIGNATURE));
    };
    let margin = (((x1 - x0).max(y1 - y0) as f64 * 0.04) as usize).max(4);
    let (cx0, cy0) = (x0.saturating_sub(margin), y0.saturating_sub(margin));
    let (cw, ch) = (x1 - x0 + 1 + 2 * margin, y1 - y0 + 1 + 2 * margin);
    // Il margine puo' uscire a destra e in basso (trasparente); a sinistra e in alto no.
    let cropped = RgbaImage::from_fn(cw as u32, ch as u32, |x, y| {
        let (sx, sy) = (cx0 + x as usize, cy0 + y as usize);
        let a = if sx < w as usize && sy < h as usize { alpha[sy * w as usize + sx] } else { 0 };
        Rgba([0, 0, 0, a])
    });
    Ok((cropped, (cx0 as u32, cy0 as u32)))
}

/// PNG con trasparenza e risoluzione (chunk pHYs): Word lo inserisce alla misura giusta.
pub fn to_png(image: &RgbaImage, dpi: u32) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(image.as_raw(), image.width(), image.height(), ExtendedColorType::Rgba8)
        .map_err(|_| Error::msg("impossibile creare il PNG della firma"))?;
    Ok(with_dpi(out, dpi))
}

/// Pixel per metro, come li vogliono PNG e DIB.
fn per_meter(dpi: u32) -> u32 {
    (dpi.max(1) as f64 / 0.0254).round() as u32
}

/// Aggiunge il chunk pHYs subito dopo IHDR (firma PNG 8 byte + IHDR 25 byte).
fn with_dpi(png: Vec<u8>, dpi: u32) -> Vec<u8> {
    const IHDR_END: usize = 8 + 25;
    if png.len() < IHDR_END || &png[12..16] != b"IHDR" {
        return png;
    }
    let ppm = per_meter(dpi).to_be_bytes();
    let mut chunk = Vec::with_capacity(13);
    chunk.extend_from_slice(b"pHYs");
    chunk.extend_from_slice(&ppm);
    chunk.extend_from_slice(&ppm);
    chunk.push(1); // unita': metro
    let mut crc = flate2::Crc::new();
    crc.update(&chunk);
    let mut out = Vec::with_capacity(png.len() + 21);
    out.extend_from_slice(&png[..IHDR_END]);
    out.extend_from_slice(&9u32.to_be_bytes());
    out.extend_from_slice(&chunk);
    out.extend_from_slice(&crc.sum().to_be_bytes());
    out.extend_from_slice(&png[IHDR_END..]);
    out
}

/// Risoluzione scritta nel PNG, se c'e'.
pub fn png_dpi(png: &[u8]) -> Option<u32> {
    let mut at = 8;
    while at + 12 <= png.len() {
        let len = u32::from_be_bytes(png[at..at + 4].try_into().ok()?) as usize;
        let kind = &png[at + 4..at + 8];
        if kind == b"pHYs" && len == 9 && at + 17 <= png.len() {
            let data = &png[at + 8..at + 17];
            if data[8] != 1 {
                return None;
            }
            let ppm = u32::from_be_bytes(data[0..4].try_into().ok()?);
            return Some((ppm as f64 * 0.0254).round() as u32);
        }
        if kind == b"IDAT" {
            return None;
        }
        at += 12 + len;
    }
    None
}

/// Firma migliore per la stampa:
/// - 2 o 3 volte i pixel, con DPI alzati dello stesso fattore: stessa misura sulla pagina, piu' nitida;
/// - tratti troppo sottili (penna fine, scansione chiara) ispessiti di un pixel;
/// - bordi lisciati; con solid anche tratto nero pieno: niente grigi che la stampante retina a puntini.
pub fn for_print(image: &RgbaImage, solid: bool) -> (RgbaImage, u32) {
    let (w, h) = image.dimensions();
    let side = w.max(h).max(1);
    let factor = if side * 3 <= PRINT_SIDE { 3 } else if side * 2 <= PRINT_SIDE { 2 } else { 1 };
    let mut alpha = GrayImage::from_fn(w, h, |x, y| Luma([image.get_pixel(x, y).0[3]]));
    if thin_strokes(&alpha) {
        alpha = thicken(&alpha);
    }
    let big = imageops::resize(&alpha, w * factor, h * factor, FilterType::CatmullRom);
    let smooth = imageops::blur(&big, 0.35 * factor as f32);
    let out = RgbaImage::from_fn(w * factor, h * factor, |x, y| {
        let a = smooth.get_pixel(x, y).0[0];
        Rgba([0, 0, 0, if solid { solid_ink(a) } else { a }])
    });
    (out, BASE_DPI * factor)
}

/// Curva morbida: sotto 50 trasparente, da 130 pieno, in mezzo il bordo liscio.
fn solid_ink(a: u8) -> u8 {
    let t = ((a as f32 - 50.0) / 80.0).clamp(0.0, 1.0);
    (t * t * (3.0 - 2.0 * t) * 255.0).round() as u8
}

/// Tratti di un pixel: quasi nessun pixel pieno (meno del 10%) ha pieni anche i quattro vicini.
fn thin_strokes(alpha: &GrayImage) -> bool {
    let (w, h) = alpha.dimensions();
    let solid = |x: u32, y: u32| alpha.get_pixel(x, y).0[0] >= SOLID;
    let (mut total, mut inner) = (0usize, 0usize);
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            if solid(x, y) {
                total += 1;
                if solid(x - 1, y) && solid(x + 1, y) && solid(x, y - 1) && solid(x, y + 1) {
                    inner += 1;
                }
            }
        }
    }
    total > 0 && inner * 10 < total
}

/// Ispessisce di un pixel: massimo tra il pixel e i quattro vicini.
fn thicken(alpha: &GrayImage) -> GrayImage {
    let (w, h) = alpha.dimensions();
    GrayImage::from_fn(w, h, |x, y| {
        let mut a = alpha.get_pixel(x, y).0[0];
        for (dx, dy) in [(-1i64, 0i64), (1, 0), (0, -1), (0, 1)] {
            let (nx, ny) = (x as i64 + dx, y as i64 + dy);
            if nx >= 0 && ny >= 0 && nx < w as i64 && ny < h as i64 {
                a = a.max(alpha.get_pixel(nx as u32, ny as u32).0[0]);
            }
        }
        Luma([a])
    })
}

/// Luminanza Rec. 601 in interi.
/// Lato massimo dell'immagine da pulire: una foto da 12 megapixel renderebbe lento lo slider
/// e il PNG enorme, senza firme piu' nitide sulla pagina.
const MAX_SIDE: u32 = 1600;

/// Riduce le immagini troppo grandi prima della pulizia.
pub fn limit(image: RgbImage) -> RgbImage {
    let (w, h) = image.dimensions();
    let side = w.max(h);
    if side <= MAX_SIDE {
        return image;
    }
    let (nw, nh) = ((w as u64 * MAX_SIDE as u64 / side as u64).max(1) as u32, (h as u64 * MAX_SIDE as u64 / side as u64).max(1) as u32);
    image::imageops::resize(&image, nw, nh, image::imageops::FilterType::Triangle)
}

/// Firma salvata, con la sua trasparenza.
pub fn load(path: &Path) -> Result<RgbaImage, Error> {
    let data = fs::read(path).map_err(|e| Error::msg(format!("firma non leggibile: {e}")))?;
    image::load_from_memory_with_format(&data, image::ImageFormat::Png)
        .map(|decoded| decoded.to_rgba8())
        .map_err(|_| Error::msg("firma non leggibile: PNG danneggiato"))
}

/// Dove preview mette una firma iw x ih in un riquadro box_w x box_h: angolo e misura.
pub fn preview_fit(iw: u32, ih: u32, box_w: u32, box_h: u32) -> (u32, u32, u32, u32) {
    let (box_w, box_h) = (box_w.max(1), box_h.max(1));
    let pad = (box_w.min(box_h) / 12).max(2);
    let (room_w, room_h) = (box_w.saturating_sub(2 * pad).max(1), box_h.saturating_sub(2 * pad).max(1));
    let ratio = (room_w as f64 / iw.max(1) as f64).min(room_h as f64 / ih.max(1) as f64);
    let (w, h) = (((iw as f64 * ratio) as u32).clamp(1, room_w), ((ih as f64 * ratio) as u32).clamp(1, room_h));
    ((box_w - w) / 2, (box_h - h) / 2, w, h)
}

/// Anteprima BGRA dall'alto in basso di box_w x box_h pixel: scacchiera chiara (mostra la
/// trasparenza anche nel tema scuro) con la firma ridotta e centrata, senza deformarla.
pub fn preview(image: &RgbaImage, box_w: u32, box_h: u32, cell: u32) -> Vec<u8> {
    let (box_w, box_h, cell) = (box_w.max(1), box_h.max(1), cell.max(1));
    let (x0, y0, w, h) = preview_fit(image.width(), image.height(), box_w, box_h);
    let small = image::imageops::resize(image, w, h, image::imageops::FilterType::Triangle);
    let mut out = Vec::with_capacity((box_w * box_h * 4) as usize);
    for y in 0..box_h {
        for x in 0..box_w {
            let paper: u32 = if (x / cell + y / cell) % 2 == 0 { 255 } else { 226 };
            let inside = x >= x0 && x < x0 + w && y >= y0 && y < y0 + h;
            let a = if inside { small.get_pixel(x - x0, y - y0).0[3] as u32 } else { 0 };
            // L'inchiostro e' nero: resta solo la carta attenuata dall'alfa.
            let v = (paper * (255 - a) / 255) as u8;
            out.extend_from_slice(&[v, v, v, 255]);
        }
    }
    out
}

fn luminance([r, g, b]: [u8; 3]) -> u8 {
    ((r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000) as u8
}

/// Primo livello sotto il quale cade la quota q dei pixel.
fn percentile(histogram: &[u32; 256], q: f64) -> u8 {
    let total: u64 = histogram.iter().map(|&c| c as u64).sum();
    let target = (total as f64 * q).ceil().max(1.0) as u64;
    let mut seen = 0u64;
    for (level, &count) in histogram.iter().enumerate() {
        seen += count as u64;
        if seen >= target {
            return level as u8;
        }
    }
    255
}

/// Soglia di Otsu sui livelli 0..=upto: separa inchiostro e carta.
fn otsu(histogram: &[u32; 256], upto: usize) -> i32 {
    let levels = &histogram[..=upto.min(255)];
    let total: f64 = levels.iter().map(|&c| c as f64).sum();
    let sum: f64 = levels.iter().enumerate().map(|(i, &c)| i as f64 * c as f64).sum();
    let (mut weight_b, mut sum_b, mut best) = (0.0, 0.0, -1.0);
    // In parita' (vuoto tra inchiostro e carta) si prende il centro del vuoto.
    let (mut first, mut last) = (upto as i32 / 2, upto as i32 / 2);
    for (t, &count) in levels.iter().enumerate() {
        weight_b += count as f64;
        if weight_b == 0.0 {
            continue;
        }
        let weight_f = total - weight_b;
        if weight_f == 0.0 {
            break;
        }
        sum_b += t as f64 * count as f64;
        let (mean_b, mean_f) = (sum_b / weight_b, (sum - sum_b) / weight_f);
        let between = weight_b * weight_f * (mean_b - mean_f).powi(2);
        if between > best * (1.0 + 1e-9) {
            best = between;
            (first, last) = (t as i32, t as i32);
        } else if between >= best * (1.0 - 1e-9) {
            last = t as i32;
        }
    }
    (first + last) / 2
}

/// Gruppo di pixel 8-connessi.
struct Blob {
    pixels: Vec<usize>,
    /// Somma delle alfa in 0..1.
    weight: f64,
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

impl Blob {
    fn height(&self) -> usize {
        self.y1 - self.y0 + 1
    }
    fn width(&self) -> usize {
        self.x1 - self.x0 + 1
    }
    fn center_y(&self) -> f64 {
        (self.y0 + self.y1) as f64 / 2.0
    }
}

/// Distanza approssimata (chamfer 3-4, in pixel) di ogni pixel dal piu' vicino pixel vero.
fn distance_map(ink: &[bool], w: usize, h: usize) -> Vec<f64> {
    const FAR: u32 = u32::MAX / 2;
    let mut d: Vec<u32> = ink.iter().map(|&on| if on { 0 } else { FAR }).collect();
    let relax = |d: &mut Vec<u32>, x: usize, y: usize, nx: i64, ny: i64, cost: u32| {
        if nx >= 0 && ny >= 0 && (nx as usize) < w && (ny as usize) < h {
            let candidate = d[ny as usize * w + nx as usize] + cost;
            if candidate < d[y * w + x] {
                d[y * w + x] = candidate;
            }
        }
    };
    for y in 0..h {
        for x in 0..w {
            let (xi, yi) = (x as i64, y as i64);
            for (dx, dy, cost) in [(-1, 0, 3), (-1, -1, 4), (0, -1, 3), (1, -1, 4)] {
                relax(&mut d, x, y, xi + dx, yi + dy, cost);
            }
        }
    }
    for y in (0..h).rev() {
        for x in (0..w).rev() {
            let (xi, yi) = (x as i64, y as i64);
            for (dx, dy, cost) in [(1, 0, 3), (1, 1, 4), (0, 1, 3), (-1, 1, 4)] {
                relax(&mut d, x, y, xi + dx, yi + dy, cost);
            }
        }
    }
    d.into_iter().map(|v| v as f64 / 3.0).collect()
}

/// Gruppi 8-connessi dei pixel con alfa almeno min.
fn blobs(alpha: &[u8], w: usize, h: usize, min: u8) -> Vec<Blob> {
    let min = min.max(1);
    let mut seen = vec![false; alpha.len()];
    let mut stack = Vec::new();
    let mut out = Vec::new();
    for start in 0..alpha.len() {
        if alpha[start] < min || seen[start] {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let (sx, sy) = (start % w, start / w);
        let mut blob = Blob { pixels: Vec::new(), weight: 0.0, x0: sx, y0: sy, x1: sx, y1: sy };
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            blob.pixels.push(i);
            blob.weight += alpha[i] as f64 / 255.0;
            (blob.x0, blob.y0, blob.x1, blob.y1) = (blob.x0.min(x), blob.y0.min(y), blob.x1.max(x), blob.y1.max(y));
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                        continue;
                    }
                    let n = ny as usize * w + nx as usize;
                    if alpha[n] >= min && !seen[n] {
                        seen[n] = true;
                        stack.push(n);
                    }
                }
            }
        }
        out.push(blob);
    }
    out
}

fn erase(alpha: &mut [u8], blob: &Blob) {
    for &i in &blob.pixels {
        alpha[i] = 0;
    }
}

/// Toglie le macchioline: gruppi di inchiostro con poco peso rispetto al totale.
fn remove_specks(alpha: &mut [u8], w: usize, h: usize) {
    let total: f64 = alpha.iter().map(|&a| a as f64 / 255.0).sum();
    let min_weight = (total / 400.0).max(2.0);
    for blob in blobs(alpha, w, h, 1) {
        if blob.weight < min_weight {
            erase(alpha, &blob);
        }
    }
}

/// Inchiostro pieno: sotto questa alfa il pixel e' solo bordo sfumato.
const SOLID: u8 = 128;

/// Toglie le righe stampate del modulo: tratti dritti lunghi almeno il 30% del lato.
/// Dove la firma attraversa la riga (inchiostro da entrambi i lati) il pixel resta.
fn remove_lines(alpha: &mut [u8], w: usize, h: usize) {
    let solid: Vec<bool> = alpha.iter().map(|&a| a >= SOLID).collect();
    // Orizzontali: corse lunghe lungo una riga; si tolgono colonna per colonna.
    let mut horizontal = vec![false; alpha.len()];
    mark_runs(&solid, &mut horizontal, h, w, (w * 3 / 10).max(40), |line, k| line * w + k);
    clear_runs(alpha, &solid, &horizontal, w, h, |line, k| k * w + line);
    // Verticali: corse lunghe lungo una colonna; si tolgono riga per riga.
    let mut vertical = vec![false; alpha.len()];
    mark_runs(&solid, &mut vertical, w, h, (h * 3 / 10).max(40), |line, k| k * w + line);
    clear_runs(alpha, &solid, &vertical, h, w, |line, k| line * w + k);
}

/// Segna le corse di inchiostro lunghe almeno min_len lungo ogni linea (riga o colonna).
fn mark_runs(solid: &[bool], marked: &mut [bool], lines: usize, len: usize, min_len: usize, at: impl Fn(usize, usize) -> usize) {
    for line in 0..lines {
        let mut k = 0;
        while k < len {
            if !solid[at(line, k)] {
                k += 1;
                continue;
            }
            let start = k;
            while k < len && solid[at(line, k)] {
                k += 1;
            }
            if k - start >= min_len {
                for j in start..k {
                    marked[at(line, j)] = true;
                }
            }
        }
    }
}

/// Toglie i pixel segnati, a tratti perpendicolari alla riga, piu' il bordo sfumato accanto.
/// Il tratto resta se la firma continua da entrambi i lati: e' un incrocio.
fn clear_runs(alpha: &mut [u8], solid: &[bool], marked: &[bool], lines: usize, len: usize, at: impl Fn(usize, usize) -> usize) {
    for line in 0..lines {
        let mut k = 0;
        while k < len {
            if !marked[at(line, k)] {
                k += 1;
                continue;
            }
            let start = k;
            while k < len && marked[at(line, k)] {
                k += 1;
            }
            // Inchiostro non di riga appena fuori dal tratto, anche in diagonale (+-2 linee).
            let ink = |j: usize| {
                (line.saturating_sub(2)..=(line + 2).min(lines - 1)).any(|l| solid[at(l, j)] && !marked[at(l, j)])
            };
            if start > 0 && ink(start - 1) && k < len && ink(k) {
                continue;
            }
            for j in start..k {
                alpha[at(line, j)] = 0;
            }
            for j in [start.checked_sub(1), (k < len).then_some(k)].into_iter().flatten() {
                if alpha[at(line, j)] < SOLID {
                    alpha[at(line, j)] = 0;
                }
            }
        }
    }
}

/// Toglie le linee a puntini o a trattini: almeno 5 pezzi alti al massimo 4 px, sulla stessa
/// riga, distesi su almeno un quarto della larghezza. Il testo stampato NON si tocca: le sue
/// lettere somigliano troppo a quelle del corsivo.
fn remove_dotted_lines(alpha: &mut [u8], w: usize, h: usize) {
    let mut dots: Vec<Blob> = blobs(alpha, w, h, SOLID).into_iter().filter(|b| b.height() <= 4 && b.width() <= 12).collect();
    dots.sort_by(|a, b| a.center_y().total_cmp(&b.center_y()));
    let mut row: Vec<&Blob> = Vec::new();
    let flush = |row: &mut Vec<&Blob>, alpha: &mut [u8]| {
        let left = row.iter().map(|b| b.x0).min().unwrap_or(0);
        let right = row.iter().map(|b| b.x1).max().unwrap_or(0);
        if row.len() >= 5 && (right - left) as f64 >= 0.25 * w as f64 {
            for blob in row.iter() {
                erase(alpha, blob);
            }
        }
        row.clear();
    };
    for dot in &dots {
        if row.last().is_some_and(|last| (dot.center_y() - last.center_y()).abs() > 2.0) {
            flush(&mut row, alpha);
        }
        row.push(dot);
    }
    flush(&mut row, alpha);
}

/// Toglie i puntini sparsi (polvere, rumore dello scanner): pezzi piccoli lontani dal tratto.
/// I puntini delle i e gli accenti stanno vicino al tratto e restano.
fn remove_noise(alpha: &mut [u8], w: usize, h: usize) {
    let found = blobs(alpha, w, h, 1);
    let largest = found.iter().map(|b| b.weight).fold(0.0, f64::max);
    let total: f64 = found.iter().map(|b| b.weight).sum();
    let limit = (largest * 0.08).max(total / 50.0);
    let big: Vec<&Blob> = found.iter().filter(|b| b.weight >= limit).collect();
    let reach = big.iter().map(|b| b.height()).max().unwrap_or(0) as f64 * 0.15 + 2.0;
    let mut ink = vec![false; alpha.len()];
    for blob in &big {
        for &i in &blob.pixels {
            ink[i] = true;
        }
    }
    let distance = distance_map(&ink, w, h);
    for blob in &found {
        if blob.weight < limit && blob.pixels.iter().all(|&i| distance[i] > reach) {
            erase(alpha, blob);
        }
    }
}

/// Riquadro (x0, y0, x1, y1 inclusi) dei pixel con inchiostro.
fn ink_box(alpha: &[u8], w: usize, h: usize) -> Option<(usize, usize, usize, usize)> {
    let mut found: Option<(usize, usize, usize, usize)> = None;
    for y in 0..h {
        for x in 0..w {
            if alpha[y * w + x] == 0 {
                continue;
            }
            found = Some(match found {
                None => (x, y, x, y),
                Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
            });
        }
    }
    found
}

// ---- Archivio delle firme ----

const FOLDER: &str = "Firme";

/// Cartelle possibili, in ordine: accanto all'exe (se non e' temporanea), poi in AppData.
fn candidate_folders() -> Vec<PathBuf> {
    let beside = settings::portable_dir().filter(|dir| !location::is_temporary(dir));
    beside.into_iter().chain(settings::appdata_dir()).map(|dir| dir.join(FOLDER)).collect()
}

/// Cartella delle firme gia' esistente, se c'e'.
pub fn existing_folder() -> Option<PathBuf> {
    candidate_folders().into_iter().find(|dir| dir.is_dir())
}

/// Firme salvate, dalla piu' recente.
pub fn list() -> Vec<PathBuf> {
    existing_folder().map(|dir| list_in(&dir)).unwrap_or_default()
}

fn list_in(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<_> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && path.extension().is_some_and(|e| e.eq_ignore_ascii_case("png")))
        .map(|path| (fs::metadata(&path).and_then(|m| m.modified()).unwrap_or(UNIX_EPOCH), path))
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    files.into_iter().map(|(_, path)| path).collect()
}

/// Nota ripulita per il nome del file: niente caratteri vietati, spazi compatti, al massimo NOTE_MAX.
pub fn clean_note(note: &str) -> String {
    let kept: String = note.chars().map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { ' ' } else { c }).collect();
    let words = kept.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = words.chars().take(NOTE_MAX).collect();
    cut.trim_end_matches(['.', ' ']).to_string()
}

/// Nome senza estensione e senza " (2)" aggiunto per non sovrascrivere.
fn base_name(path: &Path) -> String {
    let stem = path.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    match stem.rsplit_once(" (") {
        Some((head, tail)) if tail.ends_with(')') && tail[..tail.len() - 1].chars().all(|c| c.is_ascii_digit()) => head.to_string(),
        _ => stem,
    }
}

/// Nota scritta al salvataggio: firma_AAAAMMGG_HHMMSS - nota.png.
pub fn note_of(path: &Path) -> Option<String> {
    base_name(path).split_once(" - ").map(|(_, note)| note.to_string()).filter(|n| !n.is_empty())
}

/// Nome leggibile: "Mario Rossi · 11/11/2026 10:15", oppure "Firma del 11/11/2026 10:15".
pub fn label(path: &Path) -> String {
    let base = base_name(path);
    let head = base.split_once(" - ").map_or(base.as_str(), |(head, _)| head);
    let digits: Vec<char> = head.chars().filter(char::is_ascii_digit).collect();
    let date = (head.starts_with("firma_") && digits.len() >= 12).then(|| {
        let d = |a: usize, b: usize| digits[a..b].iter().collect::<String>();
        format!("{}/{}/{} {}:{}", d(6, 8), d(4, 6), d(0, 4), d(8, 10), d(10, 12))
    });
    match (note_of(path), date) {
        (Some(note), Some(date)) => format!("{note} · {date}"),
        (Some(note), None) => note,
        (None, Some(date)) => format!("Firma del {date}"),
        (None, None) => base,
    }
}

/// Salva il PNG come firma_AAAAMMGG_HHMMSS[ - nota].png, senza sovrascrivere nulla.
pub fn save(png: &[u8], note: &str) -> Result<PathBuf, Error> {
    let note = clean_note(note);
    let mut name = format!("firma_{}", timestamp());
    if !note.is_empty() {
        name.push_str(" - ");
        name.push_str(&note);
    }
    let folders = existing_folder().into_iter().chain(candidate_folders());
    for dir in folders {
        if fs::create_dir_all(&dir).is_ok() {
            if let Ok(path) = save_in(&dir, &name, png) {
                return Ok(path);
            }
        }
    }
    Err(Error::msg("impossibile salvare la firma: cartella Firme non scrivibile"))
}

fn save_in(dir: &Path, name: &str, png: &[u8]) -> std::io::Result<PathBuf> {
    // L'estensione esplicita protegge le note con un punto ("Dott. Rossi").
    write_new(&dir.join(format!("{name}.png")), "", "png", png)
}

/// Elimina una firma, solo se sta nella cartella delle firme.
pub fn delete(path: &Path) -> Result<(), Error> {
    let inside = path.parent().is_some_and(|parent| candidate_folders().iter().any(|dir| dir == parent));
    if !inside {
        return Err(Error::msg("il file non sta nella cartella delle firme"));
    }
    fs::remove_file(path).map_err(|e| Error::msg(format!("impossibile eliminare la firma: {e}")))
}

/// Data e ora locali: AAAAMMGG_HHMMSS.
fn timestamp() -> String {
    let mut t = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    format!("{:04}{:02}{:02}_{:02}{:02}{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

// ---- Formati per gli appunti ----


/// CF_DIBV5: BITMAPV5HEADER e pixel BGRA dal basso in alto, con alfa.
/// Il colore e' nero, quindi alfa premoltiplicato o diretto danno lo stesso risultato.
pub fn dib_v5(image: &RgbaImage, dpi: u32) -> Vec<u8> {
    let (w, h) = image.dimensions();
    let mut out = Vec::with_capacity(124 + (w * h * 4) as usize);
    let put = |out: &mut Vec<u8>, bytes: &[u8]| out.extend_from_slice(bytes);
    put(&mut out, &124u32.to_le_bytes());
    put(&mut out, &(w as i32).to_le_bytes());
    put(&mut out, &(h as i32).to_le_bytes());
    put(&mut out, &1u16.to_le_bytes());
    put(&mut out, &32u16.to_le_bytes());
    put(&mut out, &3u32.to_le_bytes()); // BI_BITFIELDS
    put(&mut out, &(w * h * 4).to_le_bytes());
    put(&mut out, &(per_meter(dpi) as i32).to_le_bytes());
    put(&mut out, &(per_meter(dpi) as i32).to_le_bytes());
    put(&mut out, &[0; 8]); // colori usati e importanti
    for mask in [0x00FF_0000u32, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000] {
        put(&mut out, &mask.to_le_bytes());
    }
    put(&mut out, &0x7352_4742u32.to_le_bytes()); // LCS_sRGB
    put(&mut out, &[0; 36 + 12]); // estremi e gamma, ignorati con sRGB
    put(&mut out, &4u32.to_le_bytes()); // LCS_GM_IMAGES
    put(&mut out, &[0; 12]); // profilo e riservato
    for y in (0..h).rev() {
        for x in 0..w {
            let a = image.get_pixel(x, y).0[3];
            out.extend_from_slice(&[0, 0, 0, a]);
        }
    }
    out
}

/// CF_DIB per le app che ignorano la trasparenza: 24 bit, firma appoggiata su bianco.
/// Senza questo Windows ricaverebbe il DIB dal V5 e la firma diventerebbe un rettangolo nero.
pub fn dib_on_white(image: &RgbaImage, dpi: u32) -> Vec<u8> {
    let (w, h) = image.dimensions();
    let stride = (w as usize * 3).div_ceil(4) * 4;
    let mut out = Vec::with_capacity(40 + stride * h as usize);
    let put = |out: &mut Vec<u8>, bytes: &[u8]| out.extend_from_slice(bytes);
    put(&mut out, &40u32.to_le_bytes());
    put(&mut out, &(w as i32).to_le_bytes());
    put(&mut out, &(h as i32).to_le_bytes());
    put(&mut out, &1u16.to_le_bytes());
    put(&mut out, &24u16.to_le_bytes());
    put(&mut out, &0u32.to_le_bytes()); // BI_RGB
    put(&mut out, &((stride * h as usize) as u32).to_le_bytes());
    put(&mut out, &(per_meter(dpi) as i32).to_le_bytes());
    put(&mut out, &(per_meter(dpi) as i32).to_le_bytes());
    put(&mut out, &[0; 8]);
    for y in (0..h).rev() {
        let start = out.len();
        for x in 0..w {
            let v = 255 - image.get_pixel(x, y).0[3];
            out.extend_from_slice(&[v, v, v]);
        }
        out.resize(start + stride, 0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Carta un po' grigia e rumorosa, tratto blu spesso 3 px con bordo sfumato, un puntino isolato.
    fn scanned_signature() -> RgbImage {
        let mut state = 0x2545_F491u32;
        RgbImage::from_fn(200, 80, |x, y| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let noise = (state % 9) as u8;
            let distance = (y as i32 - 40 - ((x as f64 / 12.0).sin() * 15.0) as i32).abs();
            let on_line = (30..170).contains(&x);
            let speck = x == 5 && y == 5;
            if on_line && distance <= 1 {
                image::Rgb([20, 30, 140])
            } else if on_line && distance == 2 {
                image::Rgb([130, 135, 185])
            } else if speck {
                image::Rgb([90, 90, 90])
            } else {
                image::Rgb([228 + noise, 226 + noise, 220 + noise])
            }
        })
    }

    #[test]
    fn signature_is_black_on_transparent_and_cropped() {
        let out = clean(&scanned_signature(), 0, false).unwrap();
        assert!(out.pixels().all(|p| p.0[..3] == [0, 0, 0]));
        // Carta trasparente: gli angoli del ritaglio sono vuoti.
        assert_eq!(out.get_pixel(0, 0).0[3], 0);
        // Tratto pieno: almeno un pixel opaco.
        assert!(out.pixels().any(|p| p.0[3] == 255));
        // Ritagliata al tratto (140 px) piu' il margine, il puntino in (5,5) e' sparito.
        assert!(out.width() < 170 && out.width() >= 140, "larghezza {}", out.width());
        assert!(out.height() < 60, "altezza {}", out.height());
    }

    #[test]
    fn signature_slider_changes_ink() {
        let image = scanned_signature();
        let ink = |d| clean(&image, d, false).unwrap().pixels().map(|p| p.0[3] as u64).sum::<u64>();
        assert!(ink(DARKNESS_RANGE) > ink(0));
        assert!(ink(0) > ink(-DARKNESS_RANGE));
    }

    #[test]
    fn signature_rejects_blank_and_dark_images() {
        let blank = RgbImage::from_pixel(50, 50, image::Rgb([240, 240, 240]));
        assert!(clean(&blank, 0, false).is_err());
        let dark = RgbImage::from_pixel(50, 50, image::Rgb([20, 20, 20]));
        assert!(clean(&dark, 0, false).is_err());
    }

    #[test]
    fn signature_png_round_trip_keeps_alpha() {
        let out = clean(&scanned_signature(), 0, false).unwrap();
        let png = to_png(&out, BASE_DPI).unwrap();
        let back = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(back.dimensions(), out.dimensions());
        assert_eq!(back.as_raw(), out.as_raw());
    }

    #[test]
    fn signature_from_bgra_swaps_channels() {
        let rgb = from_bgra(1, 1, &[1, 2, 3, 255]).unwrap();
        assert_eq!(rgb.get_pixel(0, 0).0, [3, 2, 1]);
        assert!(from_bgra(2, 2, &[0; 4]).is_none());
    }

    #[test]
    fn signature_clipboard_dibs_have_right_layout() {
        let mut image = RgbaImage::new(3, 2);
        image.put_pixel(0, 0, Rgba([0, 0, 0, 200]));
        let v5 = dib_v5(&image, BASE_DPI);
        assert_eq!(v5.len(), 124 + 3 * 2 * 4);
        assert_eq!(&v5[..4], &124u32.to_le_bytes());
        // Dal basso in alto: il pixel (0,0) e' nella seconda riga dei dati.
        assert_eq!(v5[124 + 3 * 4 + 3], 200);
        let white = dib_on_white(&image, BASE_DPI);
        // Righe da 9 byte portate a 12.
        assert_eq!(white.len(), 40 + 12 * 2);
        assert_eq!(white[40 + 12], 55);
        assert_eq!(white[40], 255);
    }

    #[test]
    fn signature_store_lists_newest_first_without_overwriting() {
        let dir = std::env::temp_dir().join(format!("toolspdf-firme-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let first = save_in(&dir, "firma_20261111_100000", b"a").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let second = save_in(&dir, "firma_20261111_100000", b"b").unwrap();
        assert_ne!(first, second);
        fs::write(dir.join("nota.txt"), b"x").unwrap();
        assert_eq!(list_in(&dir), vec![second, first]);
        fs::remove_dir_all(&dir).unwrap();
    }

    /// La firma di prova su un modulo: riga orizzontale che la attraversa, riga verticale a
    /// sinistra, linea a trattini sotto e polvere in alto a destra.
    fn signature_on_form() -> RgbImage {
        let small = scanned_signature();
        let (w, h) = small.dimensions();
        let mut image = RgbImage::from_pixel(w, h + 30, image::Rgb([230, 228, 222]));
        imageops::replace(&mut image, &small, 0, 0);
        let ink = image::Rgb([25, 25, 25]);
        let mut fill = |x0: u32, y0: u32, x1: u32, y1: u32| {
            for y in y0..y1 {
                for x in x0..x1 {
                    image.put_pixel(x, y, ink);
                }
            }
        };
        fill(10, 46, 190, 48);
        fill(3, 2, 4, 105);
        for dash in 0..22 {
            let x0 = 10 + dash * 8;
            fill(x0, 103, x0 + 4, 105);
        }
        for (x, y) in [(185, 3), (178, 10), (192, 16)] {
            fill(x, y, x + 3, y + 3);
        }
        image
    }

    #[test]
    fn printed_lines_dots_and_dust_go_away_signature_stays() {
        let plain = clean(&scanned_signature(), 0, false).unwrap();
        let kept = clean(&signature_on_form(), 0, false).unwrap();
        let cleaned = clean(&signature_on_form(), 0, true).unwrap();
        // Senza spunta resta tutto il modulo; con la spunta il ritaglio torna quello della firma.
        assert!(kept.height() > plain.height() + 30);
        assert!(cleaned.height().abs_diff(plain.height()) <= 4, "{} vs {}", cleaned.height(), plain.height());
        assert!(cleaned.width().abs_diff(plain.width()) <= 4, "{} vs {}", cleaned.width(), plain.width());
        let ink = |img: &RgbaImage| img.pixels().map(|p| p.0[3] as u64).sum::<u64>();
        let ratio = ink(&cleaned) as f64 / ink(&plain) as f64;
        // La soglia automatica cambia un poco con il nero del modulo: bordi sfumati piu sottili.
        assert!((0.75..1.15).contains(&ratio), "inchiostro {ratio}");
        // Una firma senza modulo non perde nulla con la spunta.
        let same = clean(&scanned_signature(), 0, true).unwrap();
        assert_eq!(ink(&same), ink(&plain));
    }

    #[test]
    fn print_version_is_sharper_solid_and_keeps_its_size() {
        // Tratto sottile di 1 px in diagonale, grigio come una penna fine.
        let thin = RgbaImage::from_fn(100, 40, |x, y| Rgba([0, 0, 0, if x / 3 == y { 150 } else { 0 }]));
        let (out, dpi) = for_print(&thin, true);
        assert_eq!(out.dimensions(), (300, 120));
        assert_eq!(dpi, 3 * BASE_DPI);
        assert!(out.pixels().all(|p| p.0[..3] == [0, 0, 0]));
        assert!(out.pixels().filter(|p| p.0[3] == 255).count() > 300);
        let png = to_png(&out, dpi).unwrap();
        assert_eq!(png_dpi(&png), Some(dpi));
        assert_eq!(image::load_from_memory(&png).unwrap().to_rgba8(), out);
        // Le firme grandi non crescono oltre PRINT_SIDE.
        let (big, big_dpi) = for_print(&RgbaImage::new(2000, 500), true);
        assert_eq!((big.width(), big_dpi), (2000, BASE_DPI));
    }

    #[test]
    fn eraser_removes_printed_text_and_maps_back_to_the_source() {
        let form = signature_on_form();
        let options = Options { darkness: 0, remove_print: false, for_print: true, solid: true };
        let before = prepare(&form, options, &[]).unwrap();
        // Una passata sopra la linea a trattini in basso: il ritaglio torna alla firma.
        let stroke = Stroke { points: vec![(0.0, 104.0), (200.0, 104.0)], radius: 4.0, magic: false };
        let after = prepare(&form, options, &[stroke]).unwrap();
        assert!(after.image.height() < before.image.height());
        assert_eq!(after.scale, 3);
        // Il riquadro nella sorgente parte dall'angolo del ritaglio e misura la firma / scala.
        let (sx, sy, sw, _) = after.source_rect();
        assert_eq!((sx, sy), (after.origin.0 as f32, after.origin.1 as f32));
        assert_eq!(sw, (after.image.width() / 3) as f32);
        // Tutto cancellato: nessuna firma.
        let all = Stroke { points: vec![(0.0, 0.0), (200.0, 0.0), (200.0, 110.0), (0.0, 110.0), (0.0, 0.0), (200.0, 110.0), (0.0, 110.0), (200.0, 0.0)], radius: 200.0, magic: false };
        assert!(prepare(&form, options, &[all]).is_err());
    }

    #[test]
    fn magic_eraser_takes_whole_printed_pieces_and_never_cuts_the_signature() {
        let form = signature_on_form();
        let options = Options { darkness: 0, remove_print: false, for_print: false, solid: false };
        let plain = prepare(&scanned_signature(), options, &[]).unwrap().image;
        let ink = |img: &RgbaImage| img.pixels().map(|p| p.0[3] as u64).sum::<u64>();
        // Passata sopra la riga orizzontale che attraversa la firma (y 46-47): la riga e' unita
        // alla firma; la gomma magica toglie la riga ma tiene gli incroci, la normale taglia la firma.
        let across = |magic| Stroke { points: vec![(10.0, 47.0), (190.0, 47.0)], radius: 3.0, magic };
        let base = prepare(&form, options, &[]).unwrap().image;
        let magic = prepare(&form, options, &[across(true)]).unwrap().image;
        let normal = prepare(&form, options, &[across(false)]).unwrap().image;
        assert!(ink(&magic) < ink(&base));
        assert!(ink(&normal) < ink(&magic));
        // Passata sulla polvere in alto a destra: pezzi interi dentro la passata, spariscono.
        let dust = Stroke { points: vec![(176.0, 4.0), (196.0, 18.0)], radius: 8.0, magic: true };
        let cleaned = prepare(&form, options, &[dust]).unwrap();
        let (sx, sy, _, _) = cleaned.source_rect();
        let before = prepare(&form, options, &[]).unwrap();
        assert!(ink(&cleaned.image) < ink(&before.image));
        assert!(sx >= 0.0 && sy >= 0.0);
        assert!(ink(&plain) > 0);
    }

    /// Scritta stampata (blocco pieno) attraversata da un tratto verticale della firma che sale da sotto.
    fn stroke_through_print(with_print: bool) -> RgbImage {
        RgbImage::from_fn(120, 70, |x, y| {
            let print = with_print && (20..80).contains(&x) && (5..17).contains(&y);
            let stroke = (49..52).contains(&x) && (8..60).contains(&y);
            let curl = (y as i32 - 60).abs() <= 1 && (20..52).contains(&x);
            if print || stroke || curl {
                image::Rgb([20, 20, 20])
            } else {
                image::Rgb([235, 235, 235])
            }
        })
    }

    #[test]
    fn magic_eraser_takes_print_crossed_by_the_signature_but_keeps_the_stroke() {
        let options = Options { darkness: 0, remove_print: false, for_print: false, solid: false };
        let pass = || Stroke { points: vec![(5.0, 11.0), (115.0, 11.0)], radius: 8.0, magic: true };
        let out = prepare(&stroke_through_print(true), options, &[pass()]).unwrap();
        let a = |x: f32, y: f32| {
            let (ox, oy) = (x - out.origin.0 as f32, y - out.origin.1 as f32);
            if ox < 0.0 || oy < 0.0 || ox as u32 >= out.image.width() || oy as u32 >= out.image.height() { 0 } else { out.image.get_pixel(ox as u32, oy as u32).0[3] }
        };
        // La scritta sparisce, il tratto resta intero anche dentro la scritta e sotto.
        assert_eq!(a(30.0, 10.0), 0);
        assert_eq!(a(70.0, 14.0), 0);
        assert_eq!(a(50.0, 12.0), 255);
        assert_eq!(a(50.0, 30.0), 255);
        assert_eq!(a(30.0, 60.0), 255);
        // Senza scritta la stessa passata attraversa solo la firma: non toglie nulla.
        let plain = stroke_through_print(false);
        let ink = |img: &RgbaImage| img.pixels().map(|p| p.0[3] as u64).sum::<u64>();
        let before = prepare(&plain, options, &[]).unwrap().image;
        let after = prepare(&plain, options, &[pass()]).unwrap().image;
        assert_eq!(ink(&after), ink(&before));
    }

    #[test]
    fn zoomed_view_keeps_the_point_under_the_mouse() {
        let options = Options { darkness: 0, remove_print: false, for_print: true, solid: true };
        let prepared = prepare(&scanned_signature(), options, &[]).unwrap();
        let fit = View::fit(&prepared, 300, 150);
        let (x, y, w, _) = prepared.source_rect();
        let (left, _) = fit.to_source(0.0, 75.0);
        assert!(left <= x && left + 300.0 * fit.k >= x + w);
        let zoomed = fit.zoom_at(fit.k / 3.0, 100.0, 50.0);
        let (a, b) = (fit.to_source(100.0, 50.0), zoomed.to_source(100.0, 50.0));
        assert!((a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3);
        let bgra = preview_view(&prepared, zoomed, 300, 150, 6);
        assert_eq!(bgra.len(), 300 * 150 * 4);
        assert!(bgra.chunks_exact(4).any(|p| p[0] < 60), "inchiostro visibile nello zoom");
        let _ = y;
    }

    #[test]
    fn solid_option_makes_every_stroke_fully_black() {
        let plain = Options { darkness: 0, remove_print: false, for_print: false, solid: false };
        let soft = prepare(&scanned_signature(), plain, &[]).unwrap().image;
        let solid = prepare(&scanned_signature(), Options { solid: true, ..plain }, &[]).unwrap().image;
        let grays = |img: &RgbaImage| img.pixels().filter(|p| (60..200).contains(&p.0[3])).count();
        assert!(grays(&solid) < grays(&soft));
        assert!(solid.pixels().filter(|p| p.0[3] == 255).count() >= soft.pixels().filter(|p| p.0[3] == 255).count());
        assert_eq!(solid.dimensions(), soft.dimensions());
    }

    #[test]
    fn note_goes_in_the_file_name_and_back() {
        assert_eq!(clean_note("  Dott. Rossi / ufficio:  paghe.  "), "Dott. Rossi ufficio paghe");
        assert_eq!(clean_note("x".repeat(80).as_str()).len(), NOTE_MAX);
        let path = Path::new(r"C:\Firme\firma_20261111_101500 - Mario Rossi (2).png");
        assert_eq!(note_of(path).as_deref(), Some("Mario Rossi"));
        assert_eq!(label(path), "Mario Rossi · 11/11/2026 10:15");
        assert_eq!(label(Path::new("firma_20261111_101500.png")), "Firma del 11/11/2026 10:15");
        assert_eq!(note_of(Path::new("firma_20261111_101500.png")), None);

        let dir = std::env::temp_dir().join(format!("toolspdf-nota-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let saved = save_in(&dir, "firma_20261111_101500 - Dott. Rossi", b"x").unwrap();
        assert_eq!(saved.file_name().unwrap(), "firma_20261111_101500 - Dott. Rossi.png");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn preview_centers_ink_on_checker_and_limit_shrinks_photos() {
        // Firma 10x2 tutta nera in un riquadro 40x20: centro scuro, angoli a scacchi.
        let ink = RgbaImage::from_pixel(10, 2, Rgba([0, 0, 0, 255]));
        let out = preview(&ink, 40, 20, 4);
        assert_eq!(out.len(), 40 * 20 * 4);
        let at = |x: usize, y: usize| out[(y * 40 + x) * 4];
        assert_eq!(at(20, 10), 0);
        assert_eq!(at(0, 0), 255);
        assert_eq!(at(4, 0), 226);
        assert!(out.chunks_exact(4).all(|p| p[3] == 255 && p[0] == p[1] && p[1] == p[2]));

        let photo = RgbImage::new(4000, 1000);
        assert_eq!(limit(photo).dimensions(), (MAX_SIDE, 400));
        assert_eq!(limit(RgbImage::new(300, 100)).dimensions(), (300, 100));
    }
}

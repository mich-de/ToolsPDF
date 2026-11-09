//! Miniature delle pagine per il pannello Unisci.

use image::imageops::{self, FilterType};
use image::RgbImage;

use super::{imaging, raster, Error, Kind, Loaded};

/// Miniatura pronta per GDI: BGRA, righe dall'alto.
pub struct Thumb {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

pub struct Preview {
    /// Pagine del documento; 0 se non si sa senza convertirlo (Word, Excel).
    pub pages: usize,
    /// Al massimo max_pages miniature, nell'ordine delle pagine.
    pub thumbs: Vec<Thumb>,
    /// Descrizione breve: "3 pagine", "immagine", "Word".
    pub label: String,
}

/// Prepara le miniature larghe circa width pixel. Word ed Excel non si convertono qui:
/// da Outlook non si sa ancora dove appoggiare il file per Office.
pub fn preview(file: &Loaded, max_pages: usize, width: u32) -> Result<Preview, Error> {
    match file.kind {
        Kind::Pdf => {
            let pdf = raster::open(file.data.clone())?;
            let pages = pdf.pages().len();
            let thumbs = pdf
                .pages()
                .iter()
                .take(max_pages)
                .map(|page| {
                    let (page_width, _) = page.render_dimensions();
                    let dpi = (width as f32 * 72.0 / page_width.max(1.0)).round().clamp(4.0, 300.0) as u32;
                    to_thumb(&raster::render_rgb(page, dpi))
                })
                .collect();
            let label = if pages == 1 { "1 pagina".to_string() } else { format!("{pages} pagine") };
            Ok(Preview { pages, thumbs, label })
        }
        Kind::Image => {
            let image = imaging::decode_rgb(&file.data)?;
            let height = ((image.height() as u64 * width as u64) / image.width().max(1) as u64).max(1) as u32;
            let small = imageops::resize(&image, width, height, FilterType::Triangle);
            Ok(Preview { pages: 1, thumbs: vec![to_thumb(&small)], label: "immagine".into() })
        }
        Kind::Word => Ok(Preview { pages: 0, thumbs: Vec::new(), label: "Word".into() }),
        Kind::Excel => Ok(Preview { pages: 0, thumbs: Vec::new(), label: "Excel".into() }),
    }
}

fn to_thumb(image: &RgbImage) -> Thumb {
    let mut bgra = Vec::with_capacity(image.width() as usize * image.height() as usize * 4);
    for pixel in image.pixels() {
        bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
    }
    Thumb { width: image.width(), height: image.height(), bgra }
}

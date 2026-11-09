//! Zona Word per scansioni e immagini: un docx con ogni pagina come immagine e sopra il
//! testo OCR trasparente. L'aspetto resta identico all'originale; il testo si cerca, si
//! seleziona e si copia. Word non serve: il file si scrive qui.

use std::fmt::Write as _;
use std::io::Write as _;

use flate2::write::DeflateEncoder;
use flate2::{Compression, Crc};
use image::codecs::png::{CompressionType, FilterType as PngFilter, PngEncoder};
use image::{ExtendedColorType, ImageEncoder, RgbImage};

use super::imaging::{self, Raster};
use super::raster::{self, render_rgb, OCR_DPI};
use super::text_layer::Word;
use super::{ocr, Error, Progress};

const JPEG_QUALITY: u8 = 80;
/// Risoluzione dell'immagine di pagina: a 200 DPI una scansione resta indistinguibile a schermo e in stampa.
const IMAGE_DPI: f64 = 200.0;
const EMU_PER_PT: f64 = 12700.0;
const TWIPS_PER_PT: f64 = 20.0;
/// Lati di un A4 in punti: un'immagine diventa una pagina di questa misura.
const A4_LONG: f64 = 842.0;
const A4_SHORT: f64 = 595.0;
/// Word non accetta pagine oltre 22 pollici.
const MAX_SIDE_PT: f64 = 1584.0;

struct DocPage {
    picture: Vec<u8>,
    /// "png" per le pagine in bianco e nero, "jpeg" per le altre.
    extension: &'static str,
    width_pt: f64,
    height_pt: f64,
    words: Vec<Word>,
}

/// Esito: il docx, quante pagine e quante parole riconosciute (None se manca l'OCR).
pub struct Built {
    pub bytes: Vec<u8>,
    pub pages: usize,
    pub words: Option<usize>,
}

pub fn from_pdf(data: &[u8], progress: Progress) -> Result<Built, Error> {
    progress(0.03, "Lettura PDF…");
    let pdf = raster::open(data.to_vec())?;
    let total = pdf.pages().len();
    if total == 0 {
        return Err(Error::msg("il PDF non ha pagine"));
    }
    let with_ocr = ocr::available();
    let mut pages = Vec::with_capacity(total);
    for (i, page) in pdf.pages().iter().enumerate() {
        progress(0.05 + 0.85 * i as f64 / total as f64, &format!("Pagina {} di {total}…", i + 1));
        // L'OCR legge meglio ad alta risoluzione; l'immagine si disegna direttamente a
        // IMAGE_DPI, senza ricampionare: le scansioni in bianco e nero restano nitide.
        let words = if with_ocr { recognize(&render_rgb(page, OCR_DPI))? } else { Vec::new() };
        let image = render_rgb(page, IMAGE_DPI as u32);
        let scale = 72.0 / IMAGE_DPI;
        let size = (f64::from(image.width()) * scale, f64::from(image.height()) * scale);
        pages.push(page_from(Raster::from_rgb(image), size, words)?);
    }
    finish(pages, with_ocr, progress)
}

pub fn from_image(data: &[u8], progress: Progress) -> Result<Built, Error> {
    progress(0.1, "Lettura immagine…");
    let image = imaging::decode_rgb(data)?;
    let with_ocr = ocr::available();
    let words = if with_ocr {
        progress(0.3, "Riconoscimento testo…");
        recognize(&image)?
    } else {
        Vec::new()
    };
    let size = a4_fit(image.width(), image.height());
    let wanted = (size.0.max(size.1) / 72.0 * IMAGE_DPI).round() as u32;
    let mut raster = Raster::from_rgb(image);
    if wanted >= 1 && wanted < raster.dimensions().0.max(raster.dimensions().1) {
        raster = raster.resized(wanted);
    }
    let page = page_from(raster, size, words)?;
    finish(vec![page], with_ocr, progress)
}

fn recognize(image: &RgbImage) -> Result<Vec<Word>, Error> {
    ocr::recognize(image).map_err(|e| Error::msg(format!("OCR non riuscito: {e}")))
}

fn finish(pages: Vec<DocPage>, with_ocr: bool, progress: Progress) -> Result<Built, Error> {
    progress(0.92, "Scrittura del documento Word…");
    let words = with_ocr.then(|| pages.iter().map(|p| p.words.len()).sum());
    Ok(Built { bytes: build(&pages)?, pages: pages.len(), words })
}

fn page_from(raster: Raster, (width_pt, height_pt): (f64, f64), words: Vec<Word>) -> Result<DocPage, Error> {
    let (picture, extension) = encode_page(&raster)?;
    let shrink = (MAX_SIDE_PT / width_pt.max(height_pt)).min(1.0);
    Ok(DocPage { picture, extension, width_pt: width_pt * shrink, height_pt: height_pt * shrink, words })
}

/// Pagina in bianco e nero: PNG senza perdite, pixel per pixel come l'originale e leggero.
/// Pagine con colori o sfumature: JPEG.
pub(super) fn encode_page(raster: &Raster) -> Result<(Vec<u8>, &'static str), Error> {
    if let Raster::Gray(gray) = raster {
        let pixels = gray.as_raw();
        let midtones = pixels.iter().filter(|&&v| (48..208).contains(&v)).count();
        if midtones * 100 < pixels.len() {
            let mut png = Vec::new();
            let bilevel: Vec<u8> = pixels.iter().map(|&v| if v < 128 { 0 } else { 255 }).collect();
            PngEncoder::new_with_quality(&mut png, CompressionType::Best, PngFilter::Adaptive)
                .write_image(&bilevel, gray.width(), gray.height(), ExtendedColorType::L8)
                .map_err(|_| Error::msg("immagine della pagina non scrivibile"))?;
            return Ok((png, "png"));
        }
    }
    let jpeg = raster.encode_jpeg(JPEG_QUALITY).ok_or_else(|| Error::msg("immagine della pagina non scrivibile"))?;
    Ok((jpeg, "jpeg"))
}

/// Un'immagine riempie una pagina A4 nel suo verso, con le sue proporzioni.
fn a4_fit(width: u32, height: u32) -> (f64, f64) {
    let (w, h) = (f64::from(width.max(1)), f64::from(height.max(1)));
    let (max_w, max_h) = if w > h { (A4_LONG, A4_SHORT) } else { (A4_SHORT, A4_LONG) };
    let scale = (max_w / w).min(max_h / h);
    (w * scale, h * scale)
}

/// Riga di testo in punti dalla pagina in alto a sinistra.
#[derive(Debug)]
struct Line {
    text: String,
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

/// L'OCR restituisce le parole in ordine di lettura: si riuniscono quelle sulla stessa riga,
/// cosi' il testo si seleziona e si copia a righe intere.
fn lines(page: &DocPage) -> Vec<Line> {
    let mut out: Vec<Line> = Vec::new();
    for word in &page.words {
        let x0 = f64::from(word.x) * page.width_pt;
        let y0 = f64::from(word.y) * page.height_pt;
        let x1 = x0 + f64::from(word.w) * page.width_pt;
        let y1 = y0 + f64::from(word.h) * page.height_pt;
        if let Some(line) = out.last_mut() {
            let height = (line.y1 - line.y0).max(y1 - y0);
            let same_row = ((y0 + y1) / 2.0 - (line.y0 + line.y1) / 2.0).abs() < height * 0.5;
            let after = x0 >= line.x1 - height * 0.5;
            let near = x0 - line.x1 < height * 4.0;
            if same_row && after && near {
                line.text.push(' ');
                line.text.push_str(&word.text);
                line.x1 = line.x1.max(x1);
                line.y0 = line.y0.min(y0);
                line.y1 = line.y1.max(y1);
                continue;
            }
        }
        out.push(Line { text: word.text.clone(), x0, y0, x1, y1 });
    }
    out
}

fn twips(pt: f64) -> i64 {
    (pt * TWIPS_PER_PT).round() as i64
}

fn emu(pt: f64) -> i64 {
    (pt * EMU_PER_PT).round() as i64
}

/// Testo sicuro in XML: caratteri speciali sostituiti, caratteri di controllo tolti.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c if (c as u32) < 0x20 => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// Riga di testo trasparente in una cornice a posizione fissa sulla pagina. "through" evita
/// che le cornici spostino il resto della pagina; fitText la allarga quanto la riga originale.
fn line_xml(out: &mut String, line: &Line) {
    let height = (line.y1 - line.y0).max(1.0);
    let width = (line.x1 - line.x0).max(1.0);
    let size = ((height * 2.0).round() as i64).clamp(2, 400);
    let _ = write!(
        out,
        concat!(
            r#"<w:p><w:pPr><w:framePr w:w="{w}" w:h="{h}" w:hRule="exact" w:wrap="through" w:hAnchor="page" w:vAnchor="page" w:x="{x}" w:y="{y}"/>"#,
            r#"<w:spacing w:before="0" w:after="0" w:line="{h}" w:lineRule="exact"/></w:pPr>"#,
            r#"<w:r><w:rPr><w:rFonts w:ascii="Arial" w:hAnsi="Arial" w:cs="Arial"/><w:sz w:val="{s}"/><w:szCs w:val="{s}"/><w:fitText w:val="{w}"/>"#,
            r#"<w14:textFill><w14:noFill/></w14:textFill></w:rPr><w:t xml:space="preserve">{t}</w:t></w:r></w:p>"#,
        ),
        w = twips(width),
        h = twips(height),
        x = twips(line.x0),
        y = twips(line.y0),
        s = size,
        t = escape(&line.text),
    );
}

fn section_xml(page: &DocPage) -> String {
    let orient = if page.width_pt > page.height_pt { r#" w:orient="landscape""# } else { "" };
    format!(
        r#"<w:sectPr><w:pgSz w:w="{}" w:h="{}"{orient}/><w:pgMar w:top="0" w:right="0" w:bottom="0" w:left="0" w:header="0" w:footer="0" w:gutter="0"/></w:sectPr>"#,
        twips(page.width_pt),
        twips(page.height_pt),
    )
}

/// Immagine della pagina dietro al testo, ancorata all'angolo della pagina.
fn image_xml(out: &mut String, page: &DocPage, number: usize, section: Option<&str>) {
    let (cx, cy) = (emu(page.width_pt), emu(page.height_pt));
    let _ = write!(
        out,
        concat!(
            r#"<w:p><w:pPr><w:spacing w:before="0" w:after="0" w:line="20" w:lineRule="exact"/>{sect}</w:pPr><w:r><w:drawing>"#,
            r#"<wp:anchor distT="0" distB="0" distL="0" distR="0" simplePos="0" relativeHeight="0" behindDoc="1" locked="1" layoutInCell="1" allowOverlap="1">"#,
            r#"<wp:simplePos x="0" y="0"/><wp:positionH relativeFrom="page"><wp:posOffset>0</wp:posOffset></wp:positionH>"#,
            r#"<wp:positionV relativeFrom="page"><wp:posOffset>0</wp:posOffset></wp:positionV><wp:extent cx="{cx}" cy="{cy}"/>"#,
            r#"<wp:effectExtent l="0" t="0" r="0" b="0"/><wp:wrapNone/><wp:docPr id="{n}" name="Pagina {n}"/>"#,
            r#"<wp:cNvGraphicFramePr><a:graphicFrameLocks noChangeAspect="1"/></wp:cNvGraphicFramePr>"#,
            r#"<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic>"#,
            r#"<pic:nvPicPr><pic:cNvPr id="{n}" name="pagina{n}.{ext}"/><pic:cNvPicPr/></pic:nvPicPr>"#,
            r#"<pic:blipFill><a:blip r:embed="rIdImg{n}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>"#,
            r#"<pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr>"#,
            r#"</pic:pic></a:graphicData></a:graphic></wp:anchor></w:drawing></w:r></w:p>"#,
        ),
        sect = section.unwrap_or(""),
        cx = cx,
        cy = cy,
        n = number,
        ext = page.extension,
    );
}

fn document_xml(pages: &[DocPage]) -> String {
    let mut body = String::new();
    for (i, page) in pages.iter().enumerate() {
        for line in lines(page) {
            line_xml(&mut body, &line);
        }
        // Ogni pagina e' una sezione con le sue misure; l'ultima chiude il corpo del documento.
        let section = (i + 1 < pages.len()).then(|| section_xml(page));
        image_xml(&mut body, page, i + 1, section.as_deref());
    }
    let last = pages.last().map(section_xml).unwrap_or_default();
    format!(
        concat!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
            r#"<w:document xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" "#,
            r#"xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" "#,
            r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" "#,
            r#"xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml" "#,
            r#"xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" "#,
            r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" "#,
            r#"xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture" mc:Ignorable="w14">"#,
            "<w:body>{}{}</w:body></w:document>",
        ),
        body, last
    )
}

const CONTENT_TYPES: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
    r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">"#,
    r#"<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>"#,
    r#"<Default Extension="xml" ContentType="application/xml"/>"#,
    r#"<Default Extension="jpeg" ContentType="image/jpeg"/>"#,
    r#"<Default Extension="png" ContentType="image/png"/>"#,
    r#"<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>"#,
    "</Types>",
);

const PACKAGE_RELS: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
    r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    r#"<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>"#,
    "</Relationships>",
);

fn document_rels(pages: &[DocPage]) -> String {
    let mut out = String::from(concat!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    ));
    for (i, page) in pages.iter().enumerate() {
        let _ = write!(
            out,
            r#"<Relationship Id="rIdImg{n}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/pagina{n}.{ext}"/>"#,
            n = i + 1,
            ext = page.extension,
        );
    }
    out.push_str("</Relationships>");
    out
}

fn build(pages: &[DocPage]) -> Result<Vec<u8>, Error> {
    let mut zip = Zip::default();
    zip.add("[Content_Types].xml", CONTENT_TYPES.as_bytes(), true)?;
    zip.add("_rels/.rels", PACKAGE_RELS.as_bytes(), true)?;
    zip.add("word/document.xml", document_xml(pages).as_bytes(), true)?;
    zip.add("word/_rels/document.xml.rels", document_rels(pages).as_bytes(), true)?;
    for (i, page) in pages.iter().enumerate() {
        // JPEG e PNG sono gia' compressi: si archiviano cosi' come sono.
        zip.add(&format!("word/media/pagina{}.{}", i + 1, page.extension), &page.picture, false)?;
    }
    zip.finish()
}

/// Archivio zip minimo, quello che serve a un docx: niente zip64, date fisse.
#[derive(Default)]
struct Zip {
    out: Vec<u8>,
    central: Vec<u8>,
    count: u16,
}

/// 1 gennaio 1980, la data piu' vecchia ammessa dal formato.
const DOS_DATE: u16 = (1 << 5) | 1;

impl Zip {
    fn add(&mut self, name: &str, data: &[u8], deflate: bool) -> Result<(), Error> {
        let too_big = || Error::msg("documento Word troppo grande (oltre 4 GB)");
        let mut crc = Crc::new();
        crc.update(data);
        let stored = if deflate {
            let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(data).and_then(|()| encoder.finish()).map_err(|e| Error::msg(format!("compressione non riuscita: {e}")))?
        } else {
            data.to_vec()
        };
        let offset = u32::try_from(self.out.len()).map_err(|_| too_big())?;
        let packed = u32::try_from(stored.len()).map_err(|_| too_big())?;
        let plain = u32::try_from(data.len()).map_err(|_| too_big())?;
        let method: u16 = if deflate { 8 } else { 0 };
        let name_len = name.len() as u16;
        self.count = self.count.checked_add(1).ok_or_else(too_big)?;

        let mut local = Vec::with_capacity(30 + name.len());
        local.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        for v in [20u16, 0, method, 0, DOS_DATE] {
            local.extend_from_slice(&v.to_le_bytes());
        }
        for v in [crc.sum(), packed, plain] {
            local.extend_from_slice(&v.to_le_bytes());
        }
        local.extend_from_slice(&name_len.to_le_bytes());
        local.extend_from_slice(&0u16.to_le_bytes());
        local.extend_from_slice(name.as_bytes());
        self.out.extend_from_slice(&local);
        self.out.extend_from_slice(&stored);

        self.central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        for v in [20u16, 20, 0, method, 0, DOS_DATE] {
            self.central.extend_from_slice(&v.to_le_bytes());
        }
        for v in [crc.sum(), packed, plain] {
            self.central.extend_from_slice(&v.to_le_bytes());
        }
        for v in [name_len, 0, 0, 0, 0] {
            self.central.extend_from_slice(&v.to_le_bytes());
        }
        self.central.extend_from_slice(&0u32.to_le_bytes());
        self.central.extend_from_slice(&offset.to_le_bytes());
        self.central.extend_from_slice(name.as_bytes());
        Ok(())
    }

    fn finish(mut self) -> Result<Vec<u8>, Error> {
        let too_big = || Error::msg("documento Word troppo grande (oltre 4 GB)");
        let start = u32::try_from(self.out.len()).map_err(|_| too_big())?;
        let size = u32::try_from(self.central.len()).map_err(|_| too_big())?;
        self.out.extend_from_slice(&self.central);
        self.out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        for v in [0u16, 0, self.count, self.count] {
            self.out.extend_from_slice(&v.to_le_bytes());
        }
        self.out.extend_from_slice(&size.to_le_bytes());
        self.out.extend_from_slice(&start.to_le_bytes());
        self.out.extend_from_slice(&0u16.to_le_bytes());
        Ok(self.out)
    }
}

/// Legge un file da un docx scritto qui: serve ai test.
#[cfg(test)]
pub fn read_entry(zip: &[u8], wanted: &str) -> Option<Vec<u8>> {
    use std::io::Read as _;
    let mut at = 0;
    while zip.get(at..at + 4)? == 0x0403_4b50u32.to_le_bytes() {
        let u16_at = |o: usize| u16::from_le_bytes([zip[at + o], zip[at + o + 1]]) as usize;
        let u32_at = |o: usize| u32::from_le_bytes(zip[at + o..at + o + 4].try_into().unwrap()) as usize;
        let (method, packed, name_len, extra) = (u16_at(8), u32_at(18), u16_at(26), u16_at(28));
        let name = std::str::from_utf8(&zip[at + 30..at + 30 + name_len]).ok()?;
        let data = &zip[at + 30 + name_len + extra..at + 30 + name_len + extra + packed];
        if name == wanted {
            if method == 0 {
                return Some(data.to_vec());
            }
            let mut plain = Vec::new();
            flate2::read::DeflateDecoder::new(data).read_to_end(&mut plain).ok()?;
            return Some(plain);
        }
        at += 30 + name_len + extra + packed;
    }
    None
}

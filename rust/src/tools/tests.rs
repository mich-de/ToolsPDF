use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use hayro::hayro_syntax::Pdf;
use image::codecs::jpeg::JpegEncoder;
use image::{ExtendedColorType, ImageEncoder};
use lopdf::{Document, Object};

use super::*;

static NEXT_DIR: AtomicU32 = AtomicU32::new(0);

fn temp_dir() -> PathBuf {
    let n = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("onembpdf-{}-{n}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn noise(len: usize) -> Vec<u8> {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

fn noisy_png(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(&noise((w * h * 3) as usize), w, h, ExtendedColorType::Rgb8)
        .unwrap();
    out
}

fn noisy_jpeg(w: u32, h: u32, quality: u8) -> Vec<u8> {
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, quality)
        .encode(&noise((w * h * 3) as usize), w, h, ExtendedColorType::Rgb8)
        .unwrap();
    out
}

/// PDF di prova: testo reale, un'immagine gia' codificata, contenuto incomprimibile opzionale.
fn build_pdf(text: &str, image_dict: &str, image: &[u8], filler: usize) -> Vec<u8> {
    let mut b: Vec<u8> = b"%PDF-1.5\n".to_vec();
    let mut offsets = Vec::new();
    let mut add = |b: &mut Vec<u8>, body: &[u8]| {
        offsets.push(b.len());
        write!(b, "{} 0 obj\n", offsets.len()).unwrap();
        b.extend_from_slice(body);
        b.extend_from_slice(b"\nendobj\n");
    };
    add(&mut b, b"<</Type/Catalog/Pages 2 0 R>>");
    add(&mut b, b"<</Type/Pages/Kids[3 0 R]/Count 1>>");
    add(&mut b, b"<</Type/Page/Parent 2 0 R/MediaBox[0 0 595 842]/Resources<</Font<</F 5 0 R>>/XObject<</I 6 0 R>>>>/Contents 4 0 R>>");
    let mut content = format!("BT /F 18 Tf 50 780 Td ({text}) Tj ET q 400 0 0 500 90 100 cm /I Do Q");
    for chunk in noise(filler * 8).chunks(8) {
        let hex: String = chunk.iter().map(|c| format!("{c:02x}")).collect();
        content.push_str(&format!(" BT /F 1 Tf 1 1 Td <{hex}> Tj ET"));
    }
    add(&mut b, format!("<</Length {}>>\nstream\n{content}\nendstream", content.len()).as_bytes());
    add(&mut b, b"<</Type/Font/Subtype/Type1/BaseFont/Helvetica>>");
    let mut obj = format!("<</Type/XObject/Subtype/Image{image_dict}/Length {}>>\nstream\n", image.len()).into_bytes();
    obj.extend_from_slice(image);
    obj.extend_from_slice(b"\nendstream");
    add(&mut b, &obj);
    let xref = b.len();
    write!(b, "xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).unwrap();
    for o in &offsets {
        write!(b, "{o:010} 00000 n \n").unwrap();
    }
    write!(b, "trailer\n<</Size {}/Root 1 0 R>>\nstartxref\n{xref}\n%%EOF\n", offsets.len() + 1).unwrap();
    b
}

fn dct_dict(w: u32, h: u32) -> String {
    format!("/Width {w}/Height {h}/ColorSpace/DeviceRGB/BitsPerComponent 8/Filter/DCTDecode")
}

fn write_temp(name: &str, data: &[u8]) -> PathBuf {
    let path = temp_dir().join(name);
    fs::write(&path, data).unwrap();
    path
}

fn no_progress(_: f64, _: &str) {}

fn never_asked(_: u64) -> bool {
    panic!("non doveva chiedere conferma")
}

fn callbacks<'a>(confirm: Confirm<'a>) -> Callbacks<'a> {
    Callbacks { progress: &no_progress, confirm }
}

/// Senza l'OCR delle altre zone: i test non dipendono dall'impostazione globale.
fn run_on(op: Operation, input: &Path, confirm: Confirm) -> Result<Outcome, Error> {
    run_ocr(op, input, confirm, false)
}

fn run_ocr(op: Operation, input: &Path, confirm: Confirm, with_ocr: bool) -> Result<Outcome, Error> {
    let file = load(Source::from_path(input.to_path_buf()))?;
    run_with(op, &file, &Destination::NextTo(input.to_path_buf()), callbacks(confirm), with_ocr)
}

fn process(input: &Path, confirm: Confirm) -> Result<Outcome, Error> {
    run_on(Operation::Compress, input, confirm)
}

fn checked_output(outcome: &Outcome, input: &Path) -> Vec<u8> {
    assert_eq!(outcome.output.parent(), input.parent());
    let out = fs::read(&outcome.output).unwrap();
    assert!(out.len() <= DEFAULT_LIMIT, "output troppo grande: {}", out.len());
    out
}

fn contains_text(pdf: &[u8], text: &str) -> bool {
    let doc = Document::load_mem(pdf).unwrap();
    doc.objects.values().any(|object| match object {
        Object::Stream(s) => {
            let plain = s.decompressed_content().unwrap_or_else(|_| s.content.clone());
            plain.windows(text.len()).any(|w| w == text.as_bytes())
        }
        _ => false,
    })
}

#[test]
fn large_png_becomes_small_pdf() {
    let input = write_temp("foto.png", &noisy_png(3000, 2000));
    let outcome = process(&input, &never_asked).unwrap();
    assert!(outcome.output.to_string_lossy().ends_with("foto_1MB.pdf"));
    let out = checked_output(&outcome, &input);
    assert_eq!(Pdf::new(out).unwrap().pages().len(), 1);
}

#[test]
fn existing_output_is_not_overwritten() {
    let input = write_temp("a.png", &noisy_png(50, 50));
    let first = process(&input, &never_asked).unwrap();
    let second = process(&input, &never_asked).unwrap();
    assert_ne!(first.output, second.output);
    assert!(second.output.to_string_lossy().ends_with("a_1MB (2).pdf"));
}

#[test]
fn pdf_with_heavy_jpeg_keeps_text_without_asking() {
    let src = build_pdf("TestoSelezionabile", &dct_dict(2600, 2000), &noisy_jpeg(2600, 2000, 90), 0);
    let input = write_temp("doc.pdf", &src);
    let outcome = process(&input, &never_asked).unwrap();
    let out = checked_output(&outcome, &input);
    assert!(contains_text(&out, "TestoSelezionabile"));
    assert_eq!(Pdf::new(out).unwrap().pages().len(), 1);
}

#[test]
fn pdf_with_raw_image_keeps_text_without_asking() {
    let (w, h) = (2000u32, 1500u32);
    let dict = format!("/Width {w}/Height {h}/ColorSpace/DeviceRGB/BitsPerComponent 8");
    let src = build_pdf("TestoGrezzo", &dict, &noise((w * h * 3) as usize), 0);
    let input = write_temp("raw.pdf", &src);
    let outcome = process(&input, &never_asked).unwrap();
    let out = checked_output(&outcome, &input);
    assert!(contains_text(&out, "TestoGrezzo"));
}

#[test]
fn uncompressed_content_is_deflated_without_asking() {
    let src = build_pdf("SoloTesto", &dct_dict(40, 40), &noisy_jpeg(40, 40, 75), 80000);
    let input = write_temp("testo.pdf", &src);
    let outcome = process(&input, &never_asked).unwrap();
    let out = checked_output(&outcome, &input);
    assert!(contains_text(&out, "SoloTesto"));
}

fn heavy_content_pdf() -> Vec<u8> {
    build_pdf("ContenutoPesante", &dct_dict(40, 40), &noisy_jpeg(40, 40, 75), 200000)
}

#[test]
fn heavy_content_asks_and_respects_no() {
    let input = write_temp("pesante.pdf", &heavy_content_pdf());
    let asked = std::cell::Cell::new(false);
    let answer = |kept: u64| {
        asked.set(true);
        assert!(kept as usize > DEFAULT_LIMIT, "kept={kept}");
        false
    };
    assert!(matches!(process(&input, &answer), Err(Error::Cancelled)));
    assert!(asked.get());
    assert!(!input.with_file_name("pesante_1MB.pdf").exists());
}

#[test]
fn heavy_content_rasterizes_on_yes() {
    let input = write_temp("pesante.pdf", &heavy_content_pdf());
    let outcome = process(&input, &|_| true).unwrap();
    let out = checked_output(&outcome, &input);
    assert!(outcome.summary.contains("DPI"), "riepilogo: {}", outcome.summary);
    assert_eq!(Pdf::new(out).unwrap().pages().len(), 1);
}

#[test]
fn unsupported_file_is_reported() {
    let input = write_temp("x.txt", b"ciao");
    match process(&input, &never_asked) {
        Err(Error::Message(m)) => assert!(m.contains("formato non supportato"), "{m}"),
        _ => panic!("doveva fallire"),
    }
}

/// Scansione di prova: una pagina con testo reale convertita in sola immagine.
fn scanned_pdf(text: &str) -> Vec<u8> {
    let native = build_pdf(text, &dct_dict(40, 40), &noisy_jpeg(40, 40, 75), 0);
    let scan = raster::rasterize(native, false, &no_progress).unwrap();
    assert!(!scan.has_text);
    scan.bytes
}

fn join_files(paths: &[PathBuf]) -> Outcome {
    join_ocr(paths, false)
}

fn join_ocr(paths: &[PathBuf], with_ocr: bool) -> Outcome {
    let files: Vec<Loaded> = paths.iter().map(|p| load(Source::from_path(p.clone())).unwrap()).collect();
    join_with(&files, &Destination::NextTo(paths[0].clone()), callbacks(&never_asked), with_ocr).unwrap()
}

/// Strato di testo invisibile scritto da text_layer.
fn has_ocr_layer(pdf: &[u8]) -> bool {
    contains_text(pdf, "3 Tr")
}

fn docx_part(path: &Path, part: &str) -> String {
    let bytes = fs::read(path).unwrap();
    String::from_utf8(docx::read_entry(&bytes, part).unwrap_or_else(|| panic!("manca {part}"))).unwrap()
}

#[test]
fn word_zone_turns_a_scan_into_identical_pages_with_text() {
    let dir = temp_dir();
    let mut paths = Vec::new();
    for (i, text) in ["Contratto di locazione firmato", "Allegato con le planimetrie"].iter().enumerate() {
        let path = dir.join(format!("w{i}.pdf"));
        fs::write(&path, scanned_pdf(text)).unwrap();
        paths.push(path);
    }
    let scan = join_files(&paths).output;
    let outcome = run_on(Operation::Word, &scan, &never_asked).unwrap();
    assert!(outcome.output.to_string_lossy().ends_with(".docx"), "{}", outcome.output.display());
    assert!(outcome.summary.contains("2 pagine identiche"), "riepilogo: {}", outcome.summary);
    assert!(outcome.summary.contains("parole riconosciute"), "riepilogo: {}", outcome.summary);
    let document = docx_part(&outcome.output, "word/document.xml");
    assert_eq!(document.matches("<w:sectPr>").count(), 2, "una sezione per pagina");
    assert_eq!(document.matches("behindDoc=\"1\"").count(), 2, "un'immagine per pagina");
    assert!(document.contains("Contratto") && document.contains("planimetrie"), "testo OCR mancante");
    assert!(document.contains("<w14:noFill/>"), "il testo deve essere trasparente");
    let rels = docx_part(&outcome.output, "word/_rels/document.xml.rels");
    let bytes = fs::read(&outcome.output).unwrap();
    for n in 1..=2 {
        let jpeg = docx::read_entry(&bytes, &format!("word/media/pagina{n}.jpeg"));
        let png = docx::read_entry(&bytes, &format!("word/media/pagina{n}.png"));
        match (jpeg, png) {
            (Some(j), None) => assert!(j.starts_with(&[0xFF, 0xD8]) && rels.contains(&format!("pagina{n}.jpeg"))),
            (None, Some(p)) => assert!(p.starts_with(b"\x89PNG") && rels.contains(&format!("pagina{n}.png"))),
            other => panic!("immagine della pagina {n}: {:?}", other.0.is_some()),
        }
    }
}

#[test]
fn black_and_white_pages_become_lossless_png() {
    let mut page = image::GrayImage::from_pixel(200, 280, image::Luma([255]));
    for x in 20..180 {
        page.put_pixel(x, 40, image::Luma([0]));
    }
    let (bytes, extension) = docx::encode_page(&imaging::Raster::Gray(page.clone())).unwrap();
    assert_eq!(extension, "png");
    assert_eq!(image::load_from_memory(&bytes).unwrap().to_luma8(), page, "il PNG deve essere identico");

    let photo = image::RgbImage::from_fn(200, 280, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 90]));
    assert_eq!(docx::encode_page(&imaging::Raster::Rgb(photo)).unwrap().1, "jpeg");
}

#[test]
fn word_zone_fits_images_to_an_a4_page() {
    let input = write_temp("foto.png", &noisy_png(1500, 1000));
    let outcome = run_on(Operation::Word, &input, &never_asked).unwrap();
    assert!(outcome.summary.contains("1 pagina identica"), "riepilogo: {}", outcome.summary);
    let document = docx_part(&outcome.output, "word/document.xml");
    assert!(document.contains(r#"<w:pgSz w:w="16840" w:h="11227" w:orient="landscape"/>"#), "pagina orizzontale A4");
}

#[test]
fn word_zone_refuses_office_files() {
    let input = write_temp("lettera.docx", b"PK finto");
    assert!(matches!(run_on(Operation::Word, &input, &never_asked), Err(Error::Message(m)) if m.contains("già un documento Word")));
}

/// Converte con Word vero i PDF e le immagini di TOOLSPDF_WORD; i docx restano accanto.
#[test]
#[ignore]
fn real_word() {
    let Some(dir) = std::env::var_os("TOOLSPDF_WORD") else { return };
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let Ok(file) = load(Source::from_path(path.clone())) else { continue };
        if file.kind == Kind::Word || file.kind == Kind::Excel {
            continue;
        }
        let started = std::time::Instant::now();
        match run(Operation::Word, &file, &Destination::NextTo(path.clone()), callbacks(&|_| true)) {
            Ok(outcome) => println!("ok   {} {:.1}s: {} -> {}", file.name, started.elapsed().as_secs_f32(), outcome.summary, outcome.output.display()),
            Err(e) => println!("ERR  {}: {e:?}", file.name),
        }
    }
}

#[test]
fn compress_adds_ocr_to_scans_when_enabled() {
    let input = write_temp("scan.pdf", &scanned_pdf("Fattura numero mille da comprimere"));
    let outcome = run_ocr(Operation::Compress, &input, &never_asked, true).unwrap();
    assert!(outcome.summary.contains("parole riconosciute"), "riepilogo: {}", outcome.summary);
    assert!(has_ocr_layer(&checked_output(&outcome, &input)));

    let plain = run_ocr(Operation::Compress, &input, &never_asked, false).unwrap();
    assert!(!has_ocr_layer(&checked_output(&plain, &input)));
}

#[test]
fn split_adds_ocr_to_every_scanned_page_when_enabled() {
    let dir = temp_dir();
    let mut paths = Vec::new();
    for (i, text) in ["Prima pagina scansionata", "Seconda pagina scansionata"].iter().enumerate() {
        let path = dir.join(format!("s{i}.pdf"));
        fs::write(&path, scanned_pdf(text)).unwrap();
        paths.push(path);
    }
    let joined = join_files(&paths).output;
    let outcome = run_ocr(Operation::Split, &joined, &never_asked, true).unwrap();
    assert!(outcome.summary.contains("parole riconosciute"), "riepilogo: {}", outcome.summary);
    let pages: Vec<PathBuf> = fs::read_dir(&outcome.output).unwrap().map(|e| e.unwrap().path()).collect();
    assert_eq!(pages.len(), 2);
    for page in pages {
        let page = fs::read(page).unwrap();
        assert!(page.len() <= DEFAULT_LIMIT);
        assert!(has_ocr_layer(&page));
    }
}

#[test]
fn join_adds_ocr_and_keeps_native_text_when_enabled() {
    let dir = temp_dir();
    let a = dir.join("nativo.pdf");
    let b = dir.join("scan.pdf");
    fs::write(&a, build_pdf("TestoVero", &dct_dict(40, 40), &noisy_jpeg(40, 40, 75), 0)).unwrap();
    fs::write(&b, scanned_pdf("Allegato scansionato da unire")).unwrap();
    let outcome = join_ocr(&[a.clone(), b], true);
    assert!(outcome.summary.contains("parole riconosciute"), "riepilogo: {}", outcome.summary);
    let out = checked_output(&outcome, &a);
    assert_eq!(Pdf::new(out.clone()).unwrap().pages().len(), 2);
    assert!(contains_text(&out, "TestoVero"));
    assert!(has_ocr_layer(&out));
}

#[test]
fn join_keeps_every_page_in_order_and_stays_under_limit() {
    let dir = temp_dir();
    let a = dir.join("a.pdf");
    let b = dir.join("b.pdf");
    fs::write(&a, build_pdf("PrimoFile", &dct_dict(40, 40), &noisy_jpeg(40, 40, 75), 0)).unwrap();
    fs::write(&b, build_pdf("SecondoFile", &dct_dict(40, 40), &noisy_jpeg(40, 40, 75), 0)).unwrap();
    let outcome = join_files(&[a.clone(), b]);
    assert!(outcome.output.to_string_lossy().ends_with("a_unito_1MB.pdf"));
    let out = checked_output(&outcome, &a);
    assert_eq!(Pdf::new(out.clone()).unwrap().pages().len(), 2);
    let doc = Document::load_mem(&out).unwrap();
    let pages: Vec<_> = doc.get_pages().values().copied().collect();
    let first = doc.get_page_content(pages[0]);
    let second = doc.get_page_content(pages[1]);
    assert!(String::from_utf8_lossy(&first).contains("PrimoFile"));
    assert!(String::from_utf8_lossy(&second).contains("SecondoFile"));
}

#[test]
fn join_accepts_images_and_compresses_heavy_files() {
    let dir = temp_dir();
    let a = dir.join("foto.png");
    let b = dir.join("doc.pdf");
    fs::write(&a, noisy_png(1500, 1000)).unwrap();
    fs::write(&b, build_pdf("ConFoto", &dct_dict(2600, 2000), &noisy_jpeg(2600, 2000, 90), 0)).unwrap();
    let outcome = join_files(&[a.clone(), b]);
    let out = checked_output(&outcome, &a);
    assert_eq!(Pdf::new(out).unwrap().pages().len(), 2);
}

fn three_page_pdf(dir: &Path) -> PathBuf {
    let mut paths = Vec::new();
    for (i, text) in ["PaginaUno", "PaginaDue", "PaginaTre"].iter().enumerate() {
        let path = dir.join(format!("p{i}.pdf"));
        fs::write(&path, build_pdf(text, &dct_dict(40, 40), &noisy_jpeg(40, 40, 75), 0)).unwrap();
        paths.push(path);
    }
    join_files(&paths).output
}

#[test]
fn split_writes_one_small_pdf_per_page_in_a_new_folder() {
    let dir = temp_dir();
    let joined = three_page_pdf(&dir);
    let outcome = run_on(Operation::Split, &joined, &never_asked).unwrap();
    assert!(outcome.is_folder);
    assert!(outcome.output.to_string_lossy().ends_with("_pagine"));
    let mut names: Vec<PathBuf> = fs::read_dir(&outcome.output).unwrap().map(|e| e.unwrap().path()).collect();
    names.sort();
    assert_eq!(names.len(), 3);
    for (path, text) in names.iter().zip(["PaginaUno", "PaginaDue", "PaginaTre"]) {
        let page = fs::read(path).unwrap();
        assert!(page.len() <= DEFAULT_LIMIT);
        assert_eq!(Pdf::new(page.clone()).unwrap().pages().len(), 1);
        for other in ["PaginaUno", "PaginaDue", "PaginaTre"] {
            assert_eq!(contains_text(&page, other), other == text, "{other} in {}", path.display());
        }
    }
    let again = run_on(Operation::Split, &joined, &never_asked).unwrap();
    assert!(again.output.to_string_lossy().ends_with("_pagine (2)"));
}

#[test]
fn split_reduces_a_heavy_page_keeping_its_text() {
    let src = build_pdf("PaginaPesante", &dct_dict(2600, 2000), &noisy_jpeg(2600, 2000, 90), 0);
    let input = write_temp("pesante.pdf", &src);
    let outcome = run_on(Operation::Split, &input, &never_asked).unwrap();
    let page = fs::read_dir(&outcome.output).unwrap().next().unwrap().unwrap().path();
    let page = fs::read(page).unwrap();
    assert!(page.len() <= DEFAULT_LIMIT);
    assert!(contains_text(&page, "PaginaPesante"));
}

#[test]
fn text_zone_adds_a_text_layer_to_scans() {
    let input = write_temp("scan.pdf", &scanned_pdf("Documento di prova per il riconoscimento"));
    let outcome = run_on(Operation::Text, &input, &never_asked).unwrap();
    assert!(outcome.summary.contains("parole riconosciute"), "riepilogo: {}", outcome.summary);
    let out = checked_output(&outcome, &input);
    assert!(contains_text(&out, "beginbfrange"), "manca la mappa ToUnicode");
    assert!(contains_text(&out, "3 Tr"), "manca lo strato invisibile");
    assert_eq!(Pdf::new(out).unwrap().pages().len(), 1);
}

#[test]
fn text_zone_leaves_native_text_alone() {
    let input = write_temp("nativo.pdf", &build_pdf("TestoVero", &dct_dict(40, 40), &noisy_jpeg(40, 40, 75), 0));
    let outcome = run_on(Operation::Text, &input, &never_asked).unwrap();
    assert!(outcome.summary.contains("testo già presente"), "riepilogo: {}", outcome.summary);
    assert!(contains_text(&fs::read(&outcome.output).unwrap(), "TestoVero"));
}

#[test]
fn text_zone_reads_images() {
    let scan = scanned_pdf("Fotografia di un documento importante");
    let pdf = raster::open(scan).unwrap();
    let image = raster::render_rgb(&pdf.pages()[0], 200);
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(image.as_raw(), image.width(), image.height(), ExtendedColorType::Rgb8)
        .unwrap();
    let input = write_temp("foto.png", &png);
    let outcome = run_on(Operation::Text, &input, &never_asked).unwrap();
    assert!(outcome.summary.contains("parole riconosciute"), "riepilogo: {}", outcome.summary);
    checked_output(&outcome, &input);
}

#[test]
fn memory_source_saves_only_where_chosen() {
    let dir = temp_dir();
    let target = dir.join("scelto.pdf");
    let source = Source::from_memory(r"..\..\allegato.png", noisy_png(50, 50));
    assert_eq!(source.name, "allegato.png");
    let file = load(source).unwrap();
    let outcome = run_with(Operation::Compress, &file, &Destination::File(target.clone()), callbacks(&never_asked), false).unwrap();
    assert_eq!(outcome.output, target);
    assert!(target.exists());
    let entries: Vec<_> = fs::read_dir(&dir).unwrap().collect();
    assert_eq!(entries.len(), 1, "nessun altro file deve comparire");
}

#[test]
fn office_files_are_recognized_by_extension() {
    assert_eq!(detect_kind("a.docx", b"PK\x03\x04xxxx"), Some(Kind::Word));
    assert_eq!(detect_kind("a.DOC", b"\xD0\xCF\x11\xE0xx"), Some(Kind::Word));
    assert_eq!(detect_kind("a.xlsx", b"PK\x03\x04xxxx"), Some(Kind::Excel));
    assert_eq!(detect_kind("a.zip", b"PK\x03\x04xxxx"), None);
}

#[test]
fn format_uses_decimal_comma() {
    assert_eq!(format_mb(999_223), "1,00 MB");
    assert_eq!(format_mb(3_441_385), "3,44 MB");
}

/// Prova sui PDF di una cartella: `TOOLSPDF_CORPUS=cartella cargo test corpus -- --ignored --nocapture`.
/// Nessuna operazione deve far cadere l'app o produrre un PDF illeggibile o sopra il limite.
#[test]
#[ignore]
fn corpus() {
    let folder = PathBuf::from(std::env::var_os("TOOLSPDF_CORPUS").expect("TOOLSPDF_CORPUS non impostata"));
    let output_dir = std::env::var_os("TOOLSPDF_CORPUS_OUT").map(PathBuf::from);
    let (mut done, mut refused) = (0, 0);
    let mut broken = Vec::new();
    for entry in fs::read_dir(folder).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| !e.eq_ignore_ascii_case("pdf")) {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let bytes = fs::read(&path).unwrap();
        for op in [Operation::Compress, Operation::Text, Operation::Split] {
            let dir = output_dir.clone().map_or_else(temp_dir, |base| {
                let d = base.join(format!("{name}-{op:?}"));
                fs::create_dir_all(&d).unwrap();
                d
            });
            let input = dir.join(&name);
            fs::write(&input, &bytes).unwrap();
            let started = std::time::Instant::now();
            match run_on(op, &input, &|_| true) {
                Ok(outcome) => {
                    done += 1;
                    let files: Vec<PathBuf> = if outcome.is_folder {
                        fs::read_dir(&outcome.output).unwrap().map(|e| e.unwrap().path()).collect()
                    } else {
                        vec![outcome.output.clone()]
                    };
                    for file in files {
                        let out = fs::read(&file).unwrap();
                        if out.len() > DEFAULT_LIMIT {
                            broken.push(format!("{name} {op:?}: {} byte", out.len()));
                        }
                        if Pdf::new(out).is_err() && Pdf::new(bytes.clone()).is_ok() {
                            broken.push(format!("{name} {op:?}: risultato non apribile"));
                        }
                    }
                    println!("ok   {name} {op:?} {:.1}s: {}", started.elapsed().as_secs_f32(), outcome.summary);
                }
                Err(Error::Message(text)) => {
                    refused += 1;
                    println!("rif. {name} {op:?} {:.1}s: {text}", started.elapsed().as_secs_f32());
                }
                Err(Error::Cancelled) => broken.push(format!("{name} {op:?}: annullato senza motivo")),
            }
            if output_dir.is_none() {
                let _ = fs::remove_dir_all(&dir);
            }
        }
    }
    println!("riusciti {done}, rifiutati con messaggio {refused}, problemi {}", broken.len());
    assert!(broken.is_empty(), "{broken:#?}");
}

/// Conversione vera con Office: `TOOLSPDF_OFFICE=cartella cargo test real_office -- --ignored --nocapture`.
/// La cartella contiene relazione.docx e conti.xlsx e non deve essere temporanea.
#[test]
#[ignore]
fn real_office() {
    let folder = PathBuf::from(std::env::var_os("TOOLSPDF_OFFICE").expect("TOOLSPDF_OFFICE non impostata"));
    let _ = unsafe { windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED) };
    let cb = callbacks(&never_asked);

    for name in ["relazione.docx", "conti.xlsx"] {
        // File gia' su disco: nasce accanto all'originale.
        let path = folder.join(name);
        let file = load(Source::from_path(path.clone())).unwrap();
        let outcome = run(Operation::Compress, &file, &Destination::NextTo(path.clone()), cb).expect(name);
        println!("disco    {name}: {} -> {}", outcome.summary, outcome.output.display());
        assert_eq!(Pdf::new(fs::read(&outcome.output).unwrap()).unwrap().pages().len(), 1);

        // Allegato in memoria: copia di lavoro nella cartella scelta, poi cancellata.
        let bytes = fs::read(&path).unwrap();
        let target = folder.join(format!("memoria_{name}.pdf"));
        let file = load(Source::from_memory(name, bytes)).unwrap();
        let outcome = run(Operation::Compress, &file, &Destination::File(target.clone()), cb).expect(name);
        println!("memoria  {name}: {}", outcome.summary);
        assert!(target.exists());
    }

    // Dividi e Unisci passano dalla stessa conversione.
    let word = folder.join("relazione.docx");
    let file = load(Source::from_path(word.clone())).unwrap();
    let split = run(Operation::Split, &file, &Destination::NextTo(word.clone()), cb).unwrap();
    println!("dividi   {}", split.summary);
    let files = vec![load(Source::from_path(word.clone())).unwrap(), load(Source::from_path(folder.join("conti.xlsx"))).unwrap()];
    let joined = run_join(&files, &Destination::NextTo(word), cb).unwrap();
    println!("unisci   {} -> {}", joined.summary, joined.output.display());
    assert_eq!(Pdf::new(fs::read(&joined.output).unwrap()).unwrap().pages().len(), 2);

    let leftovers: Vec<_> = fs::read_dir(&folder).unwrap().map(|e| e.unwrap().file_name()).filter(|n| n.to_string_lossy().starts_with("~toolspdf")).collect();
    assert!(leftovers.is_empty(), "file di lavoro rimasti: {leftovers:?}");
}

#[test]
fn limit_labels_follow_the_setting() {
    assert_eq!(limit(), DEFAULT_LIMIT);
    assert_eq!(limit_label(), "1 MB");
    assert_eq!(limit_suffix(), "_1MB");
}

#[test]
fn preview_renders_every_page_and_labels_office_files() {
    let dir = temp_dir();
    let joined = three_page_pdf(&dir);
    let file = load(Source::from_path(joined)).unwrap();
    let shown = preview::preview(&file, 40, 120).unwrap();
    assert_eq!(shown.pages, 3);
    assert_eq!(shown.thumbs.len(), 3);
    assert_eq!(shown.label, "3 pagine");
    let thumb = &shown.thumbs[0];
    assert!(thumb.width > 100 && thumb.width <= 130, "larghezza {}", thumb.width);
    assert_eq!(thumb.bgra.len(), (thumb.width * thumb.height * 4) as usize);

    let image = load(Source::from_memory("a.png", noisy_png(300, 200))).unwrap();
    let shown = preview::preview(&image, 40, 120).unwrap();
    assert_eq!((shown.pages, shown.thumbs.len(), shown.label.as_str()), (1, 1, "immagine"));

    let word = Loaded { name: "a.docx".into(), path: None, data: b"PK".to_vec(), kind: Kind::Word };
    let shown = preview::preview(&word, 40, 120).unwrap();
    assert!(shown.thumbs.is_empty());
    assert_eq!(shown.label, "Word");
}

// ---------- zona Strumenti ----------

/// PDF di prova con pagine A4 numerate nel contenuto ("Foglio 1", "Foglio 2"...).
fn pages_pdf(count: usize) -> Vec<u8> {
    use lopdf::{dictionary, Stream};
    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let font = doc.add_object(dictionary! { "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica" });
    let mut kids = Vec::new();
    for i in 0..count {
        let content = format!("BT /F1 24 Tf 72 720 Td (Foglio {}) Tj ET", i + 1);
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.into_bytes()));
        let page = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
            "Contents" => content_id,
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font } },
        });
        kids.push(Object::Reference(page));
    }
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => count as i64 }));
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog);
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

/// Testo di ogni pagina del risultato, contenuti decompressi e uniti.
fn page_texts(pdf: &[u8]) -> Vec<Vec<u8>> {
    let doc = Document::load_mem(pdf).unwrap();
    doc.get_pages().values().map(|id| doc.get_page_content(*id)).collect()
}

fn has_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn hex_of(text: &str) -> Vec<u8> {
    text.bytes().map(|b| format!("{b:02X}")).collect::<String>().into_bytes()
}

#[test]
fn tools_rotate_and_delete_pages() {
    let mut plan = edit::Plan::new(3);
    plan.keep[1] = false;
    plan.rotation[2] = 90;
    let out = edit::apply(&pages_pdf(3), &plan).unwrap();
    let doc = Document::load_mem(&out).unwrap();
    let pages: Vec<_> = doc.get_pages().values().copied().collect();
    assert_eq!(pages.len(), 2);
    let rotate = |id| doc.get_dictionary(id).unwrap().get(b"Rotate").ok().and_then(|o| o.as_i64().ok()).unwrap_or(0);
    assert_eq!(rotate(pages[0]), 0);
    assert_eq!(rotate(pages[1]), 90);
    let texts = page_texts(&out);
    assert!(has_bytes(&texts[0], b"Foglio 1"));
    assert!(has_bytes(&texts[1], b"Foglio 3"));
    assert!(edit::apply(&pages_pdf(1), &edit::Plan { keep: vec![false], ..edit::Plan::new(1) }).is_err());
}

#[test]
fn tools_extract_keeps_only_selected_pages() {
    let out = edit::extract(&pages_pdf(4), &edit::Plan::new(4), &[1, 3]).unwrap();
    let texts = page_texts(&out);
    assert_eq!(texts.len(), 2);
    assert!(has_bytes(&texts[0], b"Foglio 2"));
    assert!(has_bytes(&texts[1], b"Foglio 4"));
}

#[test]
fn tools_numbers_and_stamp_on_every_page() {
    let mut plan = edit::Plan::new(3);
    plan.keep[0] = false;
    plan.numbers = true;
    plan.stamp = Some("RISERVATO".into());
    let out = edit::apply(&pages_pdf(3), &plan).unwrap();
    let texts = page_texts(&out);
    assert!(has_bytes(&texts[0], &hex_of("Pagina 1 di 2")));
    assert!(has_bytes(&texts[1], &hex_of("Pagina 2 di 2")));
    assert!(texts.iter().all(|t| has_bytes(t, &hex_of("RISERVATO")) && has_bytes(t, b"/TPDFgs gs")));
    // Il contenuto originale resta, chiuso tra q e Q.
    assert!(has_bytes(&texts[0], b"Foglio 2"));
}

#[test]
fn tools_signature_png_keeps_transparency_on_chosen_page() {
    let mut png = Vec::new();
    let image = image::RgbaImage::from_fn(200, 80, |x, _| if x < 100 { image::Rgba([0, 0, 0, 255]) } else { image::Rgba([0, 0, 0, 0]) });
    image::DynamicImage::ImageRgba8(image).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
    let mut plan = edit::Plan::new(3);
    plan.signature = Some(edit::Signature { image: png, page: None, position: 8, size: 1 });
    let out = edit::apply(&pages_pdf(3), &plan).unwrap();
    let texts = page_texts(&out);
    assert!(!has_bytes(&texts[0], b"/TPDFsig Do"));
    assert!(has_bytes(&texts[2], b"/TPDFsig Do"));
    let doc = Document::load_mem(&out).unwrap();
    assert!(doc.objects.values().any(|o| matches!(o, Object::Stream(s) if s.dict.has(b"SMask"))));
}

#[test]
fn tools_password_round_trip() {
    let mut plan = edit::Plan::new(2);
    plan.password = Some("Segreta-1".into());
    let locked = edit::apply(&pages_pdf(2), &plan).unwrap();
    assert!(edit::is_protected(&locked));
    assert!(!edit::is_protected(&pages_pdf(2)));
    assert!(edit::unlock(&locked, "sbagliata").is_err());
    let open = edit::unlock(&locked, "Segreta-1").unwrap();
    assert!(!edit::is_protected(&open));
    let texts = page_texts(&open);
    assert_eq!(texts.len(), 2);
    assert!(has_bytes(&texts[1], b"Foglio 2"));
    // hayro deve saperlo aprire con la password, come gli altri lettori.
    assert!(raster::open(locked).is_err());
}

#[test]
fn tools_pages_to_jpegs() {
    let mut plan = edit::Plan::new(3);
    plan.keep[1] = false;
    let none = |_: f64, _: &str| {};
    let images = edit::to_jpegs(&pages_pdf(3), &plan, &none).unwrap();
    assert_eq!(images.len(), 2);
    assert!(images.iter().all(|j| j.starts_with(&[0xFF, 0xD8])));
}


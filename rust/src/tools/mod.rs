//! Operazioni su documenti: comprimi, dividi, testo selezionabile, unisci.
//! Tutto lavora in memoria: nessun file passa dalle cartelle temporanee.

mod docx;
pub mod edit;
mod form_text;
mod imaging;
mod join;
mod keep_text;
pub mod location;
mod ocr;
mod office;
pub mod preview;
mod pdf_write;
mod raster;
pub mod signature;
mod split;
mod text_layer;
mod textize;
#[cfg(test)]
mod tests;

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{Cursor, ErrorKind, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Limite predefinito in decimale: 1 000 000 byte stanno sotto "1 MB" anche per i
/// portali che contano 1 048 576.
pub const DEFAULT_LIMIT: usize = 1_000_000;

static LIMIT: AtomicUsize = AtomicUsize::new(DEFAULT_LIMIT);
static ALWAYS_OCR: AtomicBool = AtomicBool::new(true);

/// Attiva o disattiva l'OCR in Comprimi, Dividi e Unisci. La zona Testo lo fa sempre.
pub fn set_always_ocr(on: bool) {
    ALWAYS_OCR.store(on, Ordering::Relaxed);
}

/// OCR nelle altre zone: se attivo nelle impostazioni e se Windows ha una lingua con OCR.
/// Senza OCR le zone lavorano come prima, senza errori.
fn extra_ocr() -> bool {
    ALWAYS_OCR.load(Ordering::Relaxed) && ocr::available()
}

/// Imposta il limite di peso scelto nelle impostazioni, in MB decimali.
pub fn set_limit_mb(mb: u32) {
    LIMIT.store(mb.max(1) as usize * 1_000_000, Ordering::Relaxed);
}

pub fn limit() -> usize {
    LIMIT.load(Ordering::Relaxed)
}

/// "1 MB", "5 MB": per i testi.
pub fn limit_label() -> String {
    format!("{} MB", limit() / 1_000_000)
}

/// "_1MB", "_5MB": per i nomi dei file prodotti.
pub fn limit_suffix() -> String {
    format!("_{}MB", limit() / 1_000_000)
}

const UNSUPPORTED: &str = "formato non supportato: usa PDF, immagini, Word o Excel";

#[derive(Debug)]
pub enum Error {
    /// L'utente ha rifiutato un passaggio.
    Cancelled,
    Message(String),
}

impl Error {
    fn msg(text: impl Into<String>) -> Self {
        Error::Message(text.into())
    }
}

/// Avanzamento (0..1) e breve descrizione della fase.
pub type Progress<'a> = &'a dyn Fn(f64, &str);
/// Chiede se convertire le pagine in immagini; riceve il minimo raggiunto
/// mantenendo il testo, 0 se il PDF non si puo' ottimizzare cosi'.
pub type Confirm<'a> = &'a dyn Fn(u64) -> bool;

#[derive(Clone, Copy)]
pub struct Callbacks<'a> {
    pub progress: Progress<'a>,
    pub confirm: Confirm<'a>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Operation {
    Compress,
    Split,
    Text,
    /// PDF, scansioni e immagini in docx.
    Word,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Pdf,
    Image,
    Word,
    Excel,
}

/// File da elaborare: su disco oppure ricevuto in memoria da Outlook.
pub struct Source {
    pub name: String,
    pub path: Option<PathBuf>,
    pub bytes: Option<Vec<u8>>,
}

impl Source {
    pub fn from_path(path: PathBuf) -> Self {
        let name = path.file_name().unwrap_or(path.as_os_str()).to_string_lossy().into_owned();
        Source { name, path: Some(path), bytes: None }
    }

    /// Il nome arriva da un'altra applicazione: si tiene solo l'ultima parte,
    /// cosi' non puo' indicare altre cartelle.
    pub fn from_memory(name: &str, bytes: Vec<u8>) -> Self {
        let clean = Path::new(name).file_name().map_or_else(|| "allegato".to_string(), |n| n.to_string_lossy().into_owned());
        Source { name: clean, path: None, bytes: Some(bytes) }
    }
}

pub struct Loaded {
    pub name: String,
    pub path: Option<PathBuf>,
    pub data: Vec<u8>,
    pub kind: Kind,
}

pub enum Destination {
    /// Accanto al file originale, senza mai sovrascrivere.
    NextTo(PathBuf),
    /// Percorso scelto nel dialogo, che ha gia' chiesto conferma per sovrascrivere.
    File(PathBuf),
    /// Cartella scelta nel dialogo: serve a Dividi.
    Folder(PathBuf),
}

impl Destination {
    /// Cartella dove Office puo' appoggiare i file di lavoro.
    fn work_dir(&self) -> PathBuf {
        match self {
            Destination::NextTo(path) | Destination::File(path) => {
                path.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf)
            }
            Destination::Folder(folder) => folder.clone(),
        }
    }
}

pub struct Outcome {
    /// File da mostrare, oppure cartella se is_folder.
    pub output: PathBuf,
    pub is_folder: bool,
    pub summary: String,
}

/// Legge il file e ne riconosce il tipo, senza ancora elaborarlo.
pub fn load(source: Source) -> Result<Loaded, Error> {
    let Source { name, path, bytes } = source;
    let data = match (bytes, &path) {
        (Some(bytes), _) => bytes,
        (None, Some(path)) => {
            let meta = fs::metadata(path).map_err(|e| Error::msg(format!("file non leggibile: {e}")))?;
            if meta.is_dir() {
                return Err(Error::msg("le cartelle non sono supportate"));
            }
            fs::read(path).map_err(|e| Error::msg(format!("file non leggibile: {e}")))?
        }
        (None, None) => return Err(Error::msg("file vuoto")),
    };
    let kind = detect_kind(&name, &data).ok_or_else(|| Error::msg(UNSUPPORTED))?;
    Ok(Loaded { name, path, data, kind })
}

fn detect_kind(name: &str, data: &[u8]) -> Option<Kind> {
    if data[..data.len().min(1024)].windows(5).any(|w| w == b"%PDF-") {
        return Some(Kind::Pdf);
    }
    if image::ImageReader::new(Cursor::new(data)).with_guessed_format().ok()?.format().is_some() {
        return Some(Kind::Image);
    }
    let ext = Path::new(name).extension()?.to_string_lossy().to_lowercase();
    match ext.as_str() {
        "doc" | "docx" | "docm" | "rtf" | "odt" => Some(Kind::Word),
        "xls" | "xlsx" | "xlsm" | "xlsb" | "ods" => Some(Kind::Excel),
        _ => None,
    }
}

/// Esegue un'operazione su un file. Un PDF malformato non deve far cadere la barra.
pub fn run(op: Operation, file: &Loaded, dest: &Destination, cb: Callbacks) -> Result<Outcome, Error> {
    run_with(op, file, dest, cb, op != Operation::Text && extra_ocr())
}

/// Come run, con la scelta sull'OCR gia' fatta: la zona Testo lo fa comunque.
fn run_with(op: Operation, file: &Loaded, dest: &Destination, cb: Callbacks, with_ocr: bool) -> Result<Outcome, Error> {
    guarded(|| match op {
        Operation::Compress => run_compress(file, dest, cb, with_ocr),
        Operation::Split => run_split(file, dest, cb, with_ocr),
        Operation::Text => run_text(file, dest, cb),
        Operation::Word => run_word(file, dest, cb),
    })
}

/// Unisce piu' file in un PDF che sta nel limite.
pub fn run_join(files: &[Loaded], dest: &Destination, cb: Callbacks) -> Result<Outcome, Error> {
    join_with(files, dest, cb, extra_ocr())
}

fn join_with(files: &[Loaded], dest: &Destination, cb: Callbacks, with_ocr: bool) -> Result<Outcome, Error> {
    guarded(|| {
        let total: usize = files.iter().map(|f| f.data.len()).sum();
        let merged = join::merge(files, dest, cb)?;
        let (out, note) = if with_ocr {
            let after_merge = |p: f64, text: &str| (cb.progress)(0.5 + 0.5 * p, text);
            textize::from_pdf(merged, Callbacks { progress: &after_merge, confirm: cb.confirm })?
        } else {
            (cb.progress)(0.6, &format!("Riduzione a {}…", limit_label()));
            compress_pdf(merged, cb)?
        };
        let output = save(dest, &format!("_unito{}", limit_suffix()), &out, cb)?;
        Ok(Outcome { output, is_folder: false, summary: sizes_summary(total, out.len(), &note) })
    })
}

fn guarded(action: impl FnOnce() -> Result<Outcome, Error>) -> Result<Outcome, Error> {
    panic::catch_unwind(AssertUnwindSafe(action))
        .unwrap_or_else(|_| Err(Error::msg("errore interno durante la lettura del file")))
}

/// Documento pronto per le operazioni: PDF, o immagine da incorporare.
enum Prepared<'a> {
    Pdf(Vec<u8>),
    Image(&'a [u8]),
}

fn prepare<'a>(file: &'a Loaded, dest: &Destination, cb: Callbacks) -> Result<Prepared<'a>, Error> {
    match file.kind {
        Kind::Pdf => Ok(Prepared::Pdf(file.data.clone())),
        Kind::Image => Ok(Prepared::Image(&file.data)),
        Kind::Word | Kind::Excel => Ok(Prepared::Pdf(office::to_pdf(file, &dest.work_dir(), cb.progress)?)),
    }
}

fn run_compress(file: &Loaded, dest: &Destination, cb: Callbacks, with_ocr: bool) -> Result<Outcome, Error> {
    let (out, note) = match prepare(file, dest, cb)? {
        Prepared::Pdf(data) if with_ocr => textize::from_pdf(data, cb)?,
        Prepared::Pdf(data) => compress_pdf(data, cb)?,
        Prepared::Image(data) if with_ocr => textize::from_image(data, cb)?,
        Prepared::Image(data) => (imaging::compress_image(data, 0, cb.progress)?, String::new()),
    };
    let output = save(dest, &limit_suffix(), &out, cb)?;
    Ok(Outcome { output, is_folder: false, summary: sizes_summary(file.data.len(), out.len(), &note) })
}

fn run_text(file: &Loaded, dest: &Destination, cb: Callbacks) -> Result<Outcome, Error> {
    let (out, note) = match prepare(file, dest, cb)? {
        Prepared::Pdf(data) => textize::from_pdf(data, cb)?,
        Prepared::Image(data) => textize::from_image(data, cb)?,
    };
    let output = save(dest, &limit_suffix(), &out, cb)?;
    Ok(Outcome { output, is_folder: false, summary: sizes_summary(file.data.len(), out.len(), &note) })
}

fn run_split(file: &Loaded, dest: &Destination, cb: Callbacks, with_ocr: bool) -> Result<Outcome, Error> {
    let mut ocr_note = String::new();
    let (pages, converted) = match prepare(file, dest, cb)? {
        Prepared::Pdf(data) if with_ocr => {
            // Prima il testo su tutto il documento, poi la divisione: ogni pagina lo eredita.
            let first_half = |p: f64, text: &str| (cb.progress)(0.5 * p, text);
            let with_text = textize::add_text(&data, &first_half)?;
            drop(data);
            ocr_note = with_text.note;
            let second_half = |p: f64, text: &str| (cb.progress)(0.3 + 0.7 * p, text);
            split::split(&with_text.bytes, Callbacks { progress: &second_half, confirm: cb.confirm })?
        }
        Prepared::Pdf(data) => split::split(&data, cb)?,
        Prepared::Image(data) if with_ocr => {
            let (page, note) = textize::from_image(data, cb)?;
            ocr_note = note;
            (vec![page], 0)
        }
        Prepared::Image(data) => (vec![imaging::compress_image(data, 0, cb.progress)?], 0),
    };
    (cb.progress)(0.98, "Salvataggio…");
    let folder = save_pages(dest, &file.name, &pages)
        .map_err(|e| Error::msg(format!("salvataggio non riuscito: {e}")))?;
    let mut summary = match pages.len() {
        1 => "1 pagina".to_string(),
        n => format!("{n} pagine"),
    };
    if !ocr_note.is_empty() {
        summary.push_str(" · ");
        summary.push_str(&ocr_note);
    }
    if converted > 0 {
        summary.push_str(&format!(" · {converted} convertite con OCR"));
    }
    Ok(Outcome { output: folder, is_folder: true, summary })
}

/// PDF con testo vero: Word ricostruisce il documento modificabile. Scansioni e immagini:
/// docx con le pagine come immagini e il testo OCR sopra, identico all'originale.
/// Il documento Word non ha limite di peso.
fn run_word(file: &Loaded, dest: &Destination, cb: Callbacks) -> Result<Outcome, Error> {
    let (docx, note) = match file.kind {
        Kind::Image => (docx::from_image(&file.data, cb.progress)?, String::new()),
        Kind::Pdf => {
            let original = textize::has_original_text(&file.data)?;
            if original && office::word_available() {
                let bytes = office::pdf_to_docx(&file.data, &dest.work_dir(), cb.progress)?;
                let output = save_as(dest, "", "docx", &bytes, cb)?;
                let summary = format!("{} → {} · convertito con Word", format_mb(file.data.len() as u64), format_mb(bytes.len() as u64));
                return Ok(Outcome { output, is_folder: false, summary });
            }
            let note = if original { "Word non installato: pagine come immagini" } else { "" };
            (docx::from_pdf(&file.data, cb.progress)?, note.to_string())
        }
        Kind::Word => return Err(Error::msg("è già un documento Word")),
        Kind::Excel => return Err(Error::msg("un foglio Excel non si converte in Word")),
    };
    let output = save_as(dest, "", "docx", &docx.bytes, cb)?;
    let mut summary = match docx.pages {
        1 => "1 pagina identica".to_string(),
        n => format!("{n} pagine identiche"),
    };
    match docx.words {
        Some(words) => summary.push_str(&format!(" · {words} parole riconosciute")),
        None => summary.push_str(" · OCR non disponibile: solo immagini"),
    }
    if !note.is_empty() {
        summary.push_str(" · ");
        summary.push_str(&note);
    }
    Ok(Outcome { output, is_folder: false, summary })
}

fn sizes_summary(input: usize, output: usize, note: &str) -> String {
    let mut text = format!("{} → {}", format_mb(input as u64), format_mb(output as u64));
    if !note.is_empty() {
        text.push_str(" · ");
        text.push_str(note);
    }
    text
}

fn compress_pdf(data: Vec<u8>, cb: Callbacks) -> Result<(Vec<u8>, String), Error> {
    if data.len() <= limit() {
        return Ok((data, format!("già sotto {}", limit_label())));
    }
    let kept = match keep_text::shrink(&data, cb.progress) {
        Ok(kept) if kept.len() <= limit() => return Ok((kept, "testo mantenuto".into())),
        Ok(kept) => kept.len() as u64,
        Err(keep_text::ShrinkError::Password) => return Err(Error::msg("PDF protetto da password")),
        Err(keep_text::ShrinkError::Other) => 0,
    };
    if !(cb.confirm)(kept) {
        return Err(Error::Cancelled);
    }
    let out = raster::rasterize(data, true, cb.progress)?;
    let note = out.note();
    Ok((out.bytes, note))
}

fn save(dest: &Destination, suffix: &str, data: &[u8], cb: Callbacks) -> Result<PathBuf, Error> {
    if data.len() > limit() {
        return Err(Error::msg(format!("impossibile scendere sotto {} (minimo {})", limit_label(), format_mb(data.len() as u64))));
    }
    save_as(dest, suffix, "pdf", data, cb)
}

/// Salva senza controllare il peso: serve ai documenti Word.
fn save_as(dest: &Destination, suffix: &str, extension: &str, data: &[u8], cb: Callbacks) -> Result<PathBuf, Error> {
    (cb.progress)(0.98, "Salvataggio…");
    let result = match dest {
        Destination::NextTo(input) => write_new(input, suffix, extension, data),
        Destination::File(path) => fs::write(path, data).map(|()| path.clone()),
        Destination::Folder(_) => Err(std::io::Error::other("destinazione non valida")),
    };
    result.map_err(|e| Error::msg(format!("salvataggio non riuscito: {e}")))
}

/// Crea <nome><suffisso>.<estensione> senza mai sovrascrivere: se esiste aggiunge un numero.
fn write_new(input: &Path, suffix: &str, extension: &str, data: &[u8]) -> std::io::Result<PathBuf> {
    let mut base = OsString::from(input.with_extension(""));
    base.push(suffix);
    for i in 1..10000 {
        let mut name = base.clone();
        if i > 1 {
            name.push(format!(" ({i})"));
        }
        name.push(".");
        name.push(extension);
        let path = PathBuf::from(name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                if let Err(e) = file.write_all(data) {
                    drop(file);
                    let _ = fs::remove_file(&path);
                    return Err(e);
                }
                return Ok(path);
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("troppi file con lo stesso nome"))
}

/// Crea <nome>_pagine con un PDF per pagina, in una cartella sempre nuova.
fn save_pages(dest: &Destination, name: &str, pages: &[Vec<u8>]) -> std::io::Result<PathBuf> {
    save_files(dest, name, "_pagine", "pdf", pages)
}

/// Crea <nome><suffisso> (sempre nuova) con un file <nome>_pagNNN.<estensione> per pagina.
fn save_files(dest: &Destination, name: &str, folder_suffix: &str, extension: &str, pages: &[Vec<u8>]) -> std::io::Result<PathBuf> {
    let stem = file_stem(name);
    let parent = dest.work_dir();
    let mut folder = parent.join(format!("{stem}{folder_suffix}"));
    let mut attempt = 1;
    loop {
        match fs::create_dir(&folder) {
            Ok(()) => break,
            Err(e) if e.kind() == ErrorKind::AlreadyExists && attempt < 10000 => {
                attempt += 1;
                folder = parent.join(format!("{stem}{folder_suffix} ({attempt})"));
            }
            Err(e) => return Err(e),
        }
    }
    for (i, page) in pages.iter().enumerate() {
        fs::write(folder.join(format!("{stem}_pag{:03}.{extension}", i + 1)), page)?;
    }
    Ok(folder)
}

/// Nome senza estensione; "documento" se manca.
pub fn file_stem(name: &str) -> String {
    Path::new(name).file_stem().map_or_else(|| "documento".to_string(), |s| s.to_string_lossy().into_owned())
}

/// Byte in MB decimali con la virgola.
pub fn format_mb(n: u64) -> String {
    format!("{:.2} MB", n as f64 / 1e6).replace('.', ",")
}

//! Converte Word ed Excel in PDF con Office installato sul PC, senza mostrarlo.
//!
//! Office lavora solo su file veri: gli appoggi stanno nella cartella di destinazione,
//! nascosti e per pochi secondi, mai nelle cartelle temporanee. Le macro restano disattivate.

use std::fs;
use std::path::{Path, PathBuf};
use std::ptr::null_mut;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::Duration;

use windows::core::{BSTR, GUID, HRESULT, HSTRING, PCWSTR};
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_DWORD, RRF_RT_REG_DWORD};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::Foundation::{DISP_E_EXCEPTION, RPC_E_CALL_REJECTED};
use windows::Win32::Storage::FileSystem::{SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN};
use windows::Win32::System::Com::{
    CLSIDFromProgID, CoCreateInstance, IDispatch, CLSCTX_LOCAL_SERVER, DISPATCH_FLAGS, DISPATCH_METHOD, DISPATCH_PROPERTYGET, DISPATCH_PROPERTYPUT, DISPPARAMS,
};
use windows::Win32::System::Ole::DISPID_PROPERTYPUT;

use super::{Error, Kind, Loaded, Progress};

/// msoAutomationSecurityForceDisable: un allegato non deve poter eseguire macro.
const FORCE_DISABLE_MACROS: i32 = 3;
const WD_FORMAT_PDF: i32 = 17;
const WD_FORMAT_DOCX: i32 = 16;
const XL_TYPE_PDF: i32 = 0;
const WD_DO_NOT_SAVE: i32 = 0;
/// Password fittizia: su un file protetto Office fallisce invece di mostrare una richiesta invisibile.
const DUMMY_PASSWORD: &str = "ToolsPDF-nessuna-password";

static NEXT_FILE: AtomicU32 = AtomicU32::new(0);

/// File di appoggio: nascosto e cancellato anche in caso di errore.
struct WorkFile(PathBuf);

impl WorkFile {
    fn new(folder: &Path, extension: &str) -> Self {
        let n = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
        WorkFile(folder.join(format!("~toolspdf_{}_{n}.{extension}", std::process::id())))
    }

    fn hide(&self) {
        let path = HSTRING::from(self.0.as_path());
        let _ = unsafe { SetFileAttributesW(&path, FILE_ATTRIBUTE_HIDDEN) };
    }
}

impl Drop for WorkFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub fn to_pdf(file: &Loaded, work_dir: &Path, progress: Progress) -> Result<Vec<u8>, Error> {
    let extension = Path::new(&file.name).extension().map_or_else(String::new, |e| e.to_string_lossy().into_owned());
    let output = WorkFile::new(work_dir, "pdf");

    // Un file gia' su disco si apre dov'e', in sola lettura. Un allegato ricevuto in memoria
    // viene appoggiato nella cartella scelta dall'utente.
    let memory_copy = match &file.path {
        Some(_) => None,
        None => {
            let copy = WorkFile::new(work_dir, &extension);
            fs::write(&copy.0, &file.data).map_err(|e| Error::msg(format!("cartella non scrivibile: {e}")))?;
            copy.hide();
            Some(copy)
        }
    };
    let input: &Path = match (&file.path, &memory_copy) {
        (Some(path), _) => path,
        (None, Some(copy)) => &copy.0,
        (None, None) => return Err(Error::msg("file vuoto")),
    };
    let input = fs::canonicalize(input).unwrap_or_else(|_| input.to_path_buf());
    let input = input.to_string_lossy();
    let input = input.strip_prefix(r"\\?\").unwrap_or(&input);
    let target = absolute(&output.0);

    match file.kind {
        Kind::Word => {
            progress(0.1, "Conversione con Word…");
            word_to_pdf(input, &target)?;
        }
        Kind::Excel => {
            progress(0.1, "Conversione con Excel…");
            excel_to_pdf(input, &target)?;
        }
        _ => return Err(Error::msg("formato non convertibile")),
    }
    fs::read(&output.0).map_err(|_| Error::msg("Office non ha prodotto il PDF: il documento potrebbe essere vuoto"))
}

fn absolute(path: &Path) -> String {
    let parent = path.parent().and_then(|p| fs::canonicalize(p).ok());
    let full = match (parent, path.file_name()) {
        (Some(parent), Some(name)) => parent.join(name),
        _ => path.to_path_buf(),
    };
    let text = full.to_string_lossy().into_owned();
    text.strip_prefix(r"\\?\").map_or(text.clone(), str::to_string)
}

/// Word invisibile, senza avvisi e con le macro disattivate.
fn start_word() -> Result<OfficeApp, Error> {
    let app = OfficeApp::start("Word.Application", "Word")?;
    let word = &app.0;
    let _ = word.put("Visible", VARIANT::from(false));
    let _ = word.put("DisplayAlerts", VARIANT::from(0i32));
    word.put("AutomationSecurity", VARIANT::from(FORCE_DISABLE_MACROS)).map_err(|_| macro_error())?;
    Ok(app)
}

/// Apre in sola lettura; con un PDF Word lo converte in documento modificabile.
fn open_in_word(word: &Disp, input: &str) -> Result<Disp, Error> {
    let documents = word.get_object("Documents").map_err(com_error)?;
    documents
        .call_object("Open", &[
            VARIANT::from(input),
            VARIANT::from(false),
            VARIANT::from(true),
            VARIANT::from(false),
            VARIANT::from(DUMMY_PASSWORD),
        ])
        .map_err(|_| Error::msg("documento non apribile: danneggiato o protetto da password"))
}

fn word_to_pdf(input: &str, output: &str) -> Result<(), Error> {
    let app = start_word()?;
    let document = open_in_word(&app.0, input)?;
    let exported = document.call("ExportAsFixedFormat", &[VARIANT::from(output), VARIANT::from(WD_FORMAT_PDF)]);
    let _ = document.call("Close", &[VARIANT::from(WD_DO_NOT_SAVE)]);
    exported.map(|_| ()).map_err(com_error)
}

/// Vero se sul PC c'e' Word.
pub fn word_available() -> bool {
    let name = HSTRING::from("Word.Application");
    unsafe { CLSIDFromProgID(PCWSTR(name.as_ptr())) }.is_ok()
}

/// PDF in docx con la conversione di Word, che ricostruisce paragrafi, tabelle e immagini.
/// Il PDF si appoggia nascosto nella cartella di destinazione, mai in una temporanea.
pub fn pdf_to_docx(pdf: &[u8], work_dir: &Path, progress: Progress) -> Result<Vec<u8>, Error> {
    let input = WorkFile::new(work_dir, "pdf");
    fs::write(&input.0, pdf).map_err(|e| Error::msg(format!("cartella non scrivibile: {e}")))?;
    input.hide();
    let output = WorkFile::new(work_dir, "docx");
    let source = absolute(&input.0);
    let target = absolute(&output.0);

    progress(0.1, "Conversione con Word…");
    let app = start_word()?;
    let version = app.0.get_string("Version").unwrap_or_else(|| "16.0".into());
    let _warning = PdfWarningOff::new(&version);
    let document = open_in_word(&app.0, &source)?;
    progress(0.7, "Salvataggio in Word…");
    let saved = document.call("SaveAs2", &[VARIANT::from(target.as_str()), VARIANT::from(WD_FORMAT_DOCX)]);
    let _ = document.call("Close", &[VARIANT::from(WD_DO_NOT_SAVE)]);
    saved.map_err(com_error)?;
    fs::read(&output.0).map_err(|_| Error::msg("Word non ha prodotto il documento"))
}

/// Senza questa opzione Word, aprendo un PDF, aspetta una conferma su una finestra invisibile
/// e la conversione resta ferma. Il valore dell'utente torna com'era alla fine.
struct PdfWarningOff {
    key: HSTRING,
    previous: Option<u32>,
}

const PDF_WARNING_VALUE: &str = "DisableConvertPdfWarning";

impl PdfWarningOff {
    fn new(version: &str) -> Self {
        let key = HSTRING::from(format!(r"Software\Microsoft\Office\{version}\Word\Options"));
        let previous = read_dword(&key, PDF_WARNING_VALUE);
        if previous != Some(1) {
            write_dword(&key, PDF_WARNING_VALUE, 1);
        }
        PdfWarningOff { key, previous }
    }
}

impl Drop for PdfWarningOff {
    fn drop(&mut self) {
        let value = HSTRING::from(PDF_WARNING_VALUE);
        match self.previous {
            Some(1) => {}
            Some(old) => write_dword(&self.key, PDF_WARNING_VALUE, old),
            None => unsafe {
                RegDeleteKeyValueW(HKEY_CURRENT_USER, self.key.as_ptr(), value.as_ptr());
            },
        }
    }
}

fn read_dword(key: &HSTRING, name: &str) -> Option<u32> {
    let value = HSTRING::from(name);
    let mut data = 0u32;
    let mut size = size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr(), RRF_RT_REG_DWORD, null_mut(), (&raw mut data).cast(), &mut size)
    };
    (status == ERROR_SUCCESS).then_some(data)
}

fn write_dword(key: &HSTRING, name: &str, data: u32) {
    let value = HSTRING::from(name);
    unsafe {
        RegSetKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr(), REG_DWORD, (&raw const data).cast(), size_of::<u32>() as u32);
    }
}

fn excel_to_pdf(input: &str, output: &str) -> Result<(), Error> {
    let app = OfficeApp::start("Excel.Application", "Excel")?;
    let excel = &app.0;
    let _ = excel.put("Visible", VARIANT::from(false));
    let _ = excel.put("DisplayAlerts", VARIANT::from(false));
    excel.put("AutomationSecurity", VARIANT::from(FORCE_DISABLE_MACROS)).map_err(|_| macro_error())?;

    let workbooks = excel.get_object("Workbooks").map_err(com_error)?;
    let book = workbooks
        .call_object("Open", &[
            VARIANT::from(input),
            VARIANT::from(0i32),
            VARIANT::from(true),
            VARIANT::from(1i32),
            VARIANT::from(DUMMY_PASSWORD),
        ])
        .map_err(|_| Error::msg("foglio non apribile: danneggiato o protetto da password"))?;
    let exported = book.call("ExportAsFixedFormat", &[VARIANT::from(XL_TYPE_PDF), VARIANT::from(output)]);
    let _ = book.call("Close", &[VARIANT::from(false)]);
    exported.map(|_| ()).map_err(com_error)
}

fn macro_error() -> Error {
    Error::msg("impossibile disattivare le macro di Office: conversione annullata per sicurezza")
}

fn com_error(error: windows::core::Error) -> Error {
    if error.code() == DISP_E_EXCEPTION {
        return Error::msg("Office non ha potuto convertire il documento");
    }
    Error::msg(format!("errore di Office: {}", error.message()))
}

/// Istanza di Office riservata alla conversione: si chiude anche in caso di errore.
struct OfficeApp(Disp);

impl OfficeApp {
    fn start(prog_id: &str, label: &str) -> Result<Self, Error> {
        let missing = || Error::msg(format!("serve Microsoft {label} installato su questo PC"));
        let name = HSTRING::from(prog_id);
        let clsid: GUID = unsafe { CLSIDFromProgID(PCWSTR(name.as_ptr())) }.map_err(|_| missing())?;
        let dispatch: IDispatch = unsafe { CoCreateInstance(&clsid, None, CLSCTX_LOCAL_SERVER) }.map_err(|_| missing())?;
        Ok(OfficeApp(Disp(dispatch)))
    }
}

impl Drop for OfficeApp {
    fn drop(&mut self) {
        let _ = self.0.call("Quit", &[]);
    }
}

/// Chiamate per nome su un oggetto Office.
struct Disp(IDispatch);

impl Disp {
    fn id_of(&self, name: &str) -> windows::core::Result<i32> {
        let wide = HSTRING::from(name);
        let names = [PCWSTR(wide.as_ptr())];
        let mut id = 0;
        unsafe { self.0.GetIDsOfNames(&GUID::zeroed(), names.as_ptr(), 1, 0x0409, &mut id)? };
        Ok(id)
    }

    fn invoke(&self, name: &str, flags: DISPATCH_FLAGS, args: &[VARIANT]) -> windows::core::Result<VARIANT> {
        let id = self.id_of(name)?;
        // COM vuole gli argomenti in ordine inverso.
        let mut reversed: Vec<VARIANT> = args.iter().rev().cloned().collect();
        let mut named = DISPID_PROPERTYPUT;
        let params = DISPPARAMS {
            rgvarg: reversed.as_mut_ptr(),
            rgdispidNamedArgs: if flags == DISPATCH_PROPERTYPUT { &mut named } else { std::ptr::null_mut() },
            cArgs: reversed.len() as u32,
            cNamedArgs: u32::from(flags == DISPATCH_PROPERTYPUT),
        };
        // Office rifiuta le chiamate mentre e' occupato: si riprova per un po'.
        let mut attempts = 0;
        loop {
            let mut result = VARIANT::default();
            let outcome = unsafe { self.0.Invoke(id, &GUID::zeroed(), 0x0409, flags, &params, Some(&mut result), None, None) };
            match outcome {
                Ok(()) => return Ok(result),
                Err(e) if is_busy(e.code()) && attempts < 100 => {
                    attempts += 1;
                    thread::sleep(Duration::from_millis(200));
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn get_object(&self, name: &str) -> windows::core::Result<Disp> {
        let value = self.invoke(name, DISPATCH_PROPERTYGET, &[])?;
        Ok(Disp(IDispatch::try_from(&value)?))
    }

    fn call(&self, name: &str, args: &[VARIANT]) -> windows::core::Result<VARIANT> {
        self.invoke(name, DISPATCH_METHOD, args)
    }

    fn call_object(&self, name: &str, args: &[VARIANT]) -> windows::core::Result<Disp> {
        let value = self.call(name, args)?;
        Ok(Disp(IDispatch::try_from(&value)?))
    }

    fn get_string(&self, name: &str) -> Option<String> {
        let value = self.invoke(name, DISPATCH_PROPERTYGET, &[]).ok()?;
        BSTR::try_from(&value).ok().map(|text| text.to_string())
    }

    fn put(&self, name: &str, value: VARIANT) -> windows::core::Result<()> {
        self.invoke(name, DISPATCH_PROPERTYPUT, &[value]).map(|_| ())
    }
}

fn is_busy(code: HRESULT) -> bool {
    code == RPC_E_CALL_REJECTED
}


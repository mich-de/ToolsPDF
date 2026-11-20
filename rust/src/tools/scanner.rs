//! Scansione da scanner di sistema tramite WIA 2.0 (Windows Image Acquisition, COM di sistema).
//! Zero DLL esterne e zero file nelle cartelle temporanee: le pagine arrivano direttamente in memoria.

use std::ptr::null_mut;

use windows::core::{BSTR, GUID, HRESULT, HSTRING, PCWSTR};
use windows::Win32::System::Com::{
    CLSIDFromProgID, CoCreateInstance, CoInitializeEx, CoUninitialize, IDispatch, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, DISPATCH_FLAGS, DISPATCH_METHOD, DISPATCH_PROPERTYGET, DISPATCH_PROPERTYPUT, DISPPARAMS,
};
use windows::Win32::System::Ole::{
    SafeArrayAccessData, SafeArrayGetElemsize, SafeArrayGetLBound, SafeArrayGetUBound, SafeArrayUnaccessData, DISPID_PROPERTYPUT,
};
use windows::Win32::System::Variant::VARIANT;
use windows_sys::Win32::Foundation::SYSTEMTIME;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;

use super::imaging::{self, Raster};
use super::pdf_write::{self, JpegPage};
use super::Error;

/// WiaDeviceType.ScannerDeviceType.
const WIA_SCANNER_TYPE: i32 = 1;
/// Formato BMP lossless universalmente supportato da ogni driver WIA.
const WIA_FORMAT_BMP: &str = "{B96B3CAB-0728-11D3-9D7B-0000F81EF32E}";
/// WIA_ERROR_PAPER_EMPTY: l'alimentatore ADF ha finito i fogli.
const WIA_ERROR_PAPER_EMPTY: HRESULT = HRESULT(0x8021_0003u32 as i32);

/// ID proprieta' WIA (Device e Item).
const WIA_DPS_DOCUMENT_HANDLING_SELECT: &str = "3088";
const WIA_DPS_PAGES: &str = "3096";
const WIA_IPA_CUR_INTENT: &str = "6146";
const WIA_IPS_XRES: &str = "6147";
const WIA_IPS_YRES: &str = "6148";

/// Valori di WIA_DPS_DOCUMENT_HANDLING_SELECT.
const HANDLING_FEEDER: i32 = 1;
const HANDLING_FLATBED: i32 = 2;

/// Valori di WIA_IPA_CUR_INTENT.
const INTENT_COLOR: i32 = 1;
const INTENT_GRAY: i32 = 2;
const INTENT_TEXT: i32 = 4;

/// Limite di sicurezza per evitare cicli infiniti con driver difettosi.
const MAX_ADF_PAGES: usize = 200;

/// Scanner trovato sul sistema.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScannerInfo {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanSource {
    Flatbed,
    Feeder,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorMode {
    Color,
    Gray,
    BlackWhite,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScanOptions {
    pub source: ScanSource,
    pub color: ColorMode,
    pub dpi: u32,
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions { source: ScanSource::Flatbed, color: ColorMode::Color, dpi: 200 }
    }
}

pub const DPI_CHOICES: [u32; 3] = [150, 200, 300];

/// Guard RAII per COM su thread dedicato.
struct ComGuard(bool);

impl ComGuard {
    fn init() -> Self {
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        ComGuard(hr.is_ok())
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

/// Elenca gli scanner WIA installati sul PC. Vuoto se non ce ne sono o se WIA non e' disponibile.
pub fn list_scanners() -> Vec<ScannerInfo> {
    let _com = ComGuard::init();
    list_scanners_inner().unwrap_or_default()
}

fn list_scanners_inner() -> Result<Vec<ScannerInfo>, String> {
    let manager = create_wia("WIA.DeviceManager")?;
    let infos = manager.get_object("DeviceInfos").map_err(wia_err)?;
    let count = infos.get_i32("Count").unwrap_or(0);
    let mut found = Vec::new();
    for i in 1..=count {
        let Ok(info) = infos.get_object_indexed("Item", VARIANT::from(i)) else { continue };
        if info.get_i32("Type") != Some(WIA_SCANNER_TYPE) {
            continue;
        }
        let id = info.get_string("DeviceID").unwrap_or_default();
        if id.is_empty() {
            continue;
        }
        let name = read_property(&info, "Name").filter(|s| !s.trim().is_empty()).unwrap_or_else(|| format!("Scanner {}", found.len() + 1));
        found.push(ScannerInfo { id, name });
    }
    Ok(found)
}

/// Acquisisce una pagina (vetro) o tutte le pagine presenti nell'alimentatore ADF, in memoria.
pub fn scan(scanner_id: Option<&str>, options: ScanOptions) -> Result<Vec<Vec<u8>>, String> {
    let _com = ComGuard::init();
    let manager = create_wia("WIA.DeviceManager")?;
    let infos = manager.get_object("DeviceInfos").map_err(wia_err)?;
    let count = infos.get_i32("Count").unwrap_or(0);

    let mut chosen: Option<Disp> = None;
    for i in 1..=count {
        let Ok(info) = infos.get_object_indexed("Item", VARIANT::from(i)) else { continue };
        if info.get_i32("Type") != Some(WIA_SCANNER_TYPE) {
            continue;
        }
        let id = info.get_string("DeviceID").unwrap_or_default();
        if scanner_id.is_none_or(|wanted| wanted == id) {
            chosen = Some(info);
            break;
        }
    }
    let info = chosen.ok_or_else(|| "nessuno scanner disponibile: controlla che sia acceso e collegato".to_string())?;
    let device = info.call_object("Connect", &[]).map_err(|_| "impossibile collegarsi allo scanner: controlla che sia acceso".to_string())?;

    // Imposta vetro o alimentatore sul dispositivo (alcuni driver espongono la proprieta' sul device).
    let handling = match options.source {
        ScanSource::Flatbed => HANDLING_FLATBED,
        ScanSource::Feeder => HANDLING_FEEDER,
    };
    let _ = set_property(&device, WIA_DPS_DOCUMENT_HANDLING_SELECT, VARIANT::from(handling));
    if options.source == ScanSource::Feeder {
        let _ = set_property(&device, WIA_DPS_PAGES, VARIANT::from(0i32));
    }

    let items = device.get_object("Items").map_err(wia_err)?;
    let item = items.get_object_indexed("Item", VARIANT::from(1i32)).map_err(|_| "lo scanner non espone alcuna sorgente di scansione".to_string())?;

    // Imposta colore e risoluzione sull'item (best-effort: alcuni driver ignorano valori non supportati).
    let intent = match options.color {
        ColorMode::Color => INTENT_COLOR,
        ColorMode::Gray => INTENT_GRAY,
        ColorMode::BlackWhite => INTENT_TEXT,
    };
    let _ = set_property(&item, WIA_IPA_CUR_INTENT, VARIANT::from(intent));
    let dpi = options.dpi.clamp(75, 600) as i32;
    let _ = set_property(&item, WIA_IPS_XRES, VARIANT::from(dpi));
    let _ = set_property(&item, WIA_IPS_YRES, VARIANT::from(dpi));

    let mut pages = Vec::new();
    let max_pages = if options.source == ScanSource::Feeder { MAX_ADF_PAGES } else { 1 };
    for _ in 0..max_pages {
        let transfer = item
            .call_object("Transfer", &[VARIANT::from(WIA_FORMAT_BMP)])
            .or_else(|_| item.call_object("Transfer", &[]));
        let image_file = match transfer {
            Ok(file) => file,
            Err(err) if options.source == ScanSource::Feeder && !pages.is_empty() => {
                // Con l'alimentatore, WIA_ERROR_PAPER_EMPTY (o errore fine fogli) chiude il ciclo.
                let _ = err;
                break;
            }
            Err(err) if err.code() == WIA_ERROR_PAPER_EMPTY => {
                return Err("l'alimentatore è vuoto: inserisci i fogli o scegli Vetro".into());
            }
            Err(err) => {
                return Err(format!("scansione non riuscita: {}", wia_err(err)));
            }
        };
        let bytes = read_image_bytes(&image_file)?;
        if !bytes.is_empty() {
            pages.push(bytes);
        }
    }
    if pages.is_empty() {
        return Err("lo scanner non ha restituito alcuna pagina".into());
    }
    Ok(pages)
}

/// Assembla le immagini scansionate (BMP/PNG/JPEG) in un PDF multipagina in memoria,
/// pronto per entrare nella pipeline Testo (OCR) o Comprimi.
pub fn pages_to_pdf(raw_pages: &[Vec<u8>], dpi: u32) -> Result<Vec<u8>, Error> {
    if raw_pages.is_empty() {
        return Err(Error::msg("nessuna pagina scansionata"));
    }
    let dpi = f64::from(dpi.clamp(75, 600));
    let mut pages = Vec::with_capacity(raw_pages.len());
    for (idx, raw) in raw_pages.iter().enumerate() {
        let rgb = imaging::decode_rgb(raw).map_err(|_| Error::msg(format!("pagina {} non leggibile dallo scanner", idx + 1)))?;
        let width_pt = f64::from(rgb.width()) * 72.0 / dpi;
        let height_pt = f64::from(rgb.height()) * 72.0 / dpi;
        let jpeg = Raster::from_rgb(rgb)
            .encode_jpeg(92)
            .ok_or_else(|| Error::msg(format!("pagina {} non codificabile", idx + 1)))?;
        pages.push(JpegPage { data: jpeg, width_pt, height_pt });
    }
    pdf_write::write_jpeg_pdf(&pages).ok_or_else(|| Error::msg("impossibile assemblare il PDF della scansione"))
}

/// Nome predefinito per una scansione: scansione_AAAAMMGG_HHMMSS.pdf.
pub fn default_name() -> String {
    let mut now: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut now) };
    format!(
        "scansione_{:04}{:02}{:02}_{:02}{:02}{:02}.pdf",
        now.wYear, now.wMonth, now.wDay, now.wHour, now.wMinute, now.wSecond
    )
}

// ---------- COM / WIA helpers ----------

fn create_wia(prog_id: &str) -> Result<Disp, String> {
    let name = HSTRING::from(prog_id);
    let clsid: GUID = unsafe { CLSIDFromProgID(PCWSTR(name.as_ptr())) }.map_err(|_| "servizio scanner WIA di Windows non disponibile".to_string())?;
    let dispatch: IDispatch = unsafe { CoCreateInstance(&clsid, None, CLSCTX_INPROC_SERVER) }.map_err(|_| "impossibile avviare il servizio scanner WIA".to_string())?;
    Ok(Disp(dispatch))
}

fn read_property(owner: &Disp, key: &str) -> Option<String> {
    let props = owner.get_object("Properties").ok()?;
    let prop = props.get_object_indexed("Item", VARIANT::from(key)).ok()?;
    prop.get_string("Value")
}

fn set_property(owner: &Disp, key: &str, value: VARIANT) -> windows::core::Result<()> {
    let props = owner.get_object("Properties")?;
    let prop = props.get_object_indexed("Item", VARIANT::from(key))?;
    prop.put("Value", value)
}

fn read_image_bytes(image_file: &Disp) -> Result<Vec<u8>, String> {
    let file_data = image_file.get_object("FileData").map_err(wia_err)?;
    let binary = file_data.invoke("BinaryData", DISPATCH_PROPERTYGET, &[]).map_err(wia_err)?;
    extract_safearray_bytes(&binary).ok_or_else(|| "dati immagine non validi dallo scanner".into())
}

fn extract_safearray_bytes(variant: &VARIANT) -> Option<Vec<u8>> {
    unsafe {
        let raw = &variant.Anonymous.Anonymous;
        // VT_ARRAY (0x2000) | VT_UI1 (17)
        const VT_ARRAY_UI1: u16 = 0x2000 | 17;
        if raw.vt.0 != VT_ARRAY_UI1 {
            return None;
        }
        let psa = raw.Anonymous.parray;
        if psa.is_null() {
            return None;
        }
        if SafeArrayGetElemsize(psa) != 1 {
            return None;
        }
        let lo = SafeArrayGetLBound(psa, 1).ok()?;
        let hi = SafeArrayGetUBound(psa, 1).ok()?;
        if hi < lo {
            return Some(Vec::new());
        }
        let len = (hi - lo + 1) as usize;
        let mut ptr = null_mut();
        SafeArrayAccessData(psa, &mut ptr).ok()?;
        let slice = std::slice::from_raw_parts(ptr as *const u8, len);
        let bytes = slice.to_vec();
        let _ = SafeArrayUnaccessData(psa);
        Some(bytes)
    }
}

fn wia_err(error: windows::core::Error) -> String {
    let msg = error.message();
    if msg.is_empty() {
        format!("codice 0x{:08X}", error.code().0 as u32)
    } else {
        msg
    }
}

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
        let mut reversed: Vec<VARIANT> = args.iter().rev().cloned().collect();
        let mut named = DISPID_PROPERTYPUT;
        let params = DISPPARAMS {
            rgvarg: reversed.as_mut_ptr(),
            rgdispidNamedArgs: if flags == DISPATCH_PROPERTYPUT { &mut named } else { null_mut() },
            cArgs: reversed.len() as u32,
            cNamedArgs: u32::from(flags == DISPATCH_PROPERTYPUT),
        };
        let mut result = VARIANT::default();
        unsafe { self.0.Invoke(id, &GUID::zeroed(), 0x0409, flags, &params, Some(&mut result), None, None)? };
        Ok(result)
    }

    fn get_object(&self, name: &str) -> windows::core::Result<Disp> {
        let value = self.invoke(name, DISPATCH_PROPERTYGET, &[])?;
        Ok(Disp(IDispatch::try_from(&value)?))
    }

    fn get_object_indexed(&self, name: &str, index: VARIANT) -> windows::core::Result<Disp> {
        let value = self.invoke(name, DISPATCH_PROPERTYGET, &[index])?;
        Ok(Disp(IDispatch::try_from(&value)?))
    }

    fn call_object(&self, name: &str, args: &[VARIANT]) -> windows::core::Result<Disp> {
        let value = self.invoke(name, DISPATCH_METHOD, args)?;
        Ok(Disp(IDispatch::try_from(&value)?))
    }

    fn get_string(&self, name: &str) -> Option<String> {
        let value = self.invoke(name, DISPATCH_PROPERTYGET, &[]).ok()?;
        BSTR::try_from(&value).ok().map(|text| text.to_string())
    }

    fn get_i32(&self, name: &str) -> Option<i32> {
        let value = self.invoke(name, DISPATCH_PROPERTYGET, &[]).ok()?;
        i32::try_from(&value).ok()
    }

    fn put(&self, name: &str, value: VARIANT) -> windows::core::Result<()> {
        self.invoke(name, DISPATCH_PROPERTYPUT, &[value]).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, Rgb, RgbImage};
    use std::io::Cursor;

    #[test]
    fn scanned_pages_assemble_into_multipage_pdf() {
        let mut page1 = Vec::new();
        let img1 = RgbImage::from_pixel(200, 300, Rgb([240, 240, 240]));
        img1.write_to(&mut Cursor::new(&mut page1), ImageFormat::Bmp).unwrap();

        let mut page2 = Vec::new();
        let img2 = RgbImage::from_pixel(200, 300, Rgb([200, 220, 240]));
        img2.write_to(&mut Cursor::new(&mut page2), ImageFormat::Png).unwrap();

        let pdf = pages_to_pdf(&[page1, page2], 200).unwrap();
        let doc = lopdf::Document::load_mem(&pdf).unwrap();
        assert_eq!(doc.get_pages().len(), 2);
        assert!(pages_to_pdf(&[], 200).is_err());
    }

    #[test]
    fn default_scan_name_has_expected_pattern() {
        let name = default_name();
        assert!(name.starts_with("scansione_"));
        assert!(name.ends_with(".pdf"));
    }
}


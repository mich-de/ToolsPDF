//! Aggiornamento da GitHub: unico accesso alla rete dell'app, nessun documento inviato.
//! Si legge version.txt dell'ultima release (riga 1 versione, riga 2 SHA-256 dell'exe),
//! poi si scarica l'exe accanto a quello in uso (mai nelle cartelle temporanee), si verifica
//! l'impronta e si scambiano i file: Windows permette di rinominare un exe in esecuzione.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Networking::WinHttp::*;
use windows_sys::Win32::Security::Cryptography::{BCryptHash, BCRYPT_SHA256_ALG_HANDLE};

pub const REPO: &str = "mich-de/ToolsPDF";
const HOST: &str = "github.com";
const ASSET: &str = "ToolsPDF.exe";
const MAX_DOWNLOAD: usize = 64 * 1024 * 1024;

pub struct Release {
    pub version: String,
    sha256: [u8; 32],
}

/// Ultima release pubblicata. /releases/latest/download/ non passa dall'API di GitHub:
/// niente limite di richieste per gli uffici dietro lo stesso indirizzo.
pub fn latest() -> Result<Release, String> {
    let text = get(&format!("/{REPO}/releases/latest/download/version.txt"), &|_| {})?;
    parse_version_file(&String::from_utf8_lossy(&text))
}

pub fn parse_version_file(text: &str) -> Result<Release, String> {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let version = lines.next().map(|v| v.trim_start_matches('v').to_string()).filter(|v| parse(v).is_some()).ok_or("version.txt senza versione")?;
    let hex = lines.next().ok_or("version.txt senza impronta")?;
    let bytes: Vec<u8> = (0..hex.len())
        .step_by(2)
        .filter_map(|i| hex.get(i..i + 2).and_then(|pair| u8::from_str_radix(pair, 16).ok()))
        .collect();
    let sha256: [u8; 32] = bytes.try_into().map_err(|_| "impronta SHA-256 non valida")?;
    Ok(Release { version, sha256 })
}

fn parse(version: &str) -> Option<(u32, u32, u32)> {
    let mut parts = version.trim_start_matches('v').split('.').map(|p| p.parse::<u32>().ok());
    let triple = (parts.next()??, parts.next()??, parts.next().unwrap_or(Some(0))?);
    parts.next().is_none().then_some(triple)
}

/// La versione pubblicata e' piu' recente di quella in uso?
pub fn is_newer(candidate: &str, current: &str) -> bool {
    matches!((parse(candidate), parse(current)), (Some(a), Some(b)) if a > b)
}

/// Scarica, verifica e mette al posto dell'exe in uso. La versione nuova parte al prossimo avvio.
pub fn install(release: &Release, progress: &dyn Fn(f64)) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let data = get(&format!("/{REPO}/releases/download/v{}/{ASSET}", release.version), progress)?;
    if !data.starts_with(b"MZ") || sha256(&data)? != release.sha256 {
        return Err("file scaricato non valido: aggiornamento annullato".into());
    }
    let (new, old) = (sibling(&exe, ".new"), sibling(&exe, ".old"));
    fs::write(&new, &data).map_err(|e| format!("cartella dell'exe non scrivibile: {e}"))?;
    let _ = fs::remove_file(&old);
    if let Err(e) = fs::rename(&exe, &old) {
        let _ = fs::remove_file(&new);
        return Err(format!("exe non sostituibile: {e}"));
    }
    if let Err(e) = fs::rename(&new, &exe) {
        let _ = fs::rename(&old, &exe);
        let _ = fs::remove_file(&new);
        return Err(format!("exe non sostituibile: {e}"));
    }
    Ok(())
}

/// All'avvio toglie i resti di un aggiornamento precedente. Se il vecchio processo
/// non e' ancora chiuso la rimozione fallisce e si riprova la volta dopo.
pub fn cleanup() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = fs::remove_file(sibling(&exe, ".old"));
        let _ = fs::remove_file(sibling(&exe, ".new"));
    }
}

fn sibling(exe: &Path, suffix: &str) -> PathBuf {
    let mut name = OsString::from(exe.as_os_str());
    name.push(suffix);
    PathBuf::from(name)
}

fn sha256(data: &[u8]) -> Result<[u8; 32], String> {
    let mut out = [0u8; 32];
    let status = unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, null(), 0, data.as_ptr(), data.len() as u32, out.as_mut_ptr(), 32) };
    if status != 0 {
        return Err("SHA-256 non disponibile".into());
    }
    Ok(out)
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Chiude gli handle WinHTTP anche in caso di errore.
struct Handle(*mut core::ffi::c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

/// GET HTTPS su github.com con il proxy di sistema di Windows; segue i rinvii verso i server dei file.
fn get(path: &str, progress: &dyn Fn(f64)) -> Result<Vec<u8>, String> {
    let offline = || "GitHub non raggiungibile".to_string();
    unsafe {
        let agent = wide(&format!("ToolsPDF/{}", env!("CARGO_PKG_VERSION")));
        let session = Handle(WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, null(), null(), 0));
        if session.0.is_null() {
            return Err(offline());
        }
        WinHttpSetTimeouts(session.0, 10_000, 10_000, 30_000, 60_000);
        let host = wide(HOST);
        let connect = Handle(WinHttpConnect(session.0, host.as_ptr(), INTERNET_DEFAULT_HTTPS_PORT, 0));
        let (verb, target) = (wide("GET"), wide(path));
        let request = Handle(WinHttpOpenRequest(connect.0, verb.as_ptr(), target.as_ptr(), null(), null(), null(), WINHTTP_FLAG_SECURE));
        if connect.0.is_null() || request.0.is_null() {
            return Err(offline());
        }
        if WinHttpSendRequest(request.0, null(), 0, null(), 0, 0, 0) == 0 || WinHttpReceiveResponse(request.0, null_mut()) == 0 {
            return Err(offline());
        }
        let status = query_number(request.0, WINHTTP_QUERY_STATUS_CODE).unwrap_or(0);
        match status {
            200 => {}
            404 => return Err("nessuna versione pubblicata".into()),
            other => return Err(format!("GitHub ha risposto {other}")),
        }
        let total = query_number(request.0, WINHTTP_QUERY_CONTENT_LENGTH).unwrap_or(0) as usize;
        let mut data = Vec::with_capacity(total.min(MAX_DOWNLOAD));
        loop {
            let mut available = 0u32;
            if WinHttpQueryDataAvailable(request.0, &mut available) == 0 {
                return Err("download interrotto".into());
            }
            if available == 0 {
                break;
            }
            let start = data.len();
            data.resize(start + available as usize, 0);
            let mut read = 0u32;
            if WinHttpReadData(request.0, data[start..].as_mut_ptr().cast(), available, &mut read) == 0 {
                return Err("download interrotto".into());
            }
            data.truncate(start + read as usize);
            if data.len() > MAX_DOWNLOAD {
                return Err("file troppo grande".into());
            }
            if total > 0 {
                progress(data.len() as f64 / total as f64);
            }
        }
        Ok(data)
    }
}

unsafe fn query_number(request: *mut core::ffi::c_void, info: u32) -> Option<u32> {
    let mut value = 0u32;
    let mut size = 4u32;
    let ok = unsafe { WinHttpQueryHeaders(request, info | WINHTTP_QUERY_FLAG_NUMBER, null(), (&mut value as *mut u32).cast(), &mut size, null_mut()) };
    (ok != 0).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_number() {
        assert!(is_newer("1.10.0", "1.9.3"));
        assert!(is_newer("v2.0", "1.9.9"));
        assert!(!is_newer("1.1.0", "1.1.0"));
        assert!(!is_newer("1.0.9", "1.1.0"));
        assert!(!is_newer("boh", "1.1.0"));
    }

    #[test]
    fn version_file_needs_version_and_hash() {
        let hash = "ab".repeat(32);
        let release = parse_version_file(&format!("v1.2.0\r\n{hash}\r\n")).unwrap();
        assert_eq!(release.version, "1.2.0");
        assert_eq!(release.sha256, [0xab; 32]);
        assert!(parse_version_file("1.2.0\n").is_err());
        assert!(parse_version_file("1.2.0\nzz").is_err());
        assert!(parse_version_file("ciao\nabab").is_err());
    }

    #[test]
    fn sha256_matches_known_value() {
        let digest = sha256(b"abc").unwrap();
        assert_eq!(digest[..4], [0xba, 0x78, 0x16, 0xbf]);
    }
}

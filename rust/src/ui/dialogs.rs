//! Dialoghi "Salva con nome" e "Scegli cartella". Non accettano cartelle temporanee.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use windows::core::{w, Interface, HSTRING};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FileOpenDialog, FileSaveDialog, FOLDERID_Documents, IFileDialog, IFileOpenDialog, IFileSaveDialog, IShellItem, SHCreateItemFromParsingName, SHGetKnownFolderPath,
    FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, KF_FLAG_DEFAULT, SIGDN_FILESYSPATH,
};

use super::win;
use crate::tools::location;
use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONWARNING, MB_OK, MB_TOPMOST};

const TEMP_REFUSED: &str = "Questa è una cartella temporanea: il file potrebbe sparire. Scegli un'altra cartella.";

/// Ultima cartella scelta in questa sessione, cosi' piu' allegati finiscono nello stesso posto.
static LAST_FOLDER: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn init_worker_thread() {
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
}

/// Chiede dove salvare un PDF. None se l'utente annulla.
pub fn pick_file(owner: windows_sys::Win32::Foundation::HWND, suggested: &str) -> Option<PathBuf> {
    loop {
        let path = show_save(owner, suggested).ok()??;
        let parent = path.parent().map_or_else(|| path.clone(), Path::to_path_buf);
        if accept_folder(owner, &parent) {
            return Some(path);
        }
    }
}

/// Chiede la cartella in cui creare la cartella folder_name (es. nome_pagine). None se l'utente annulla.
pub fn pick_folder(owner: windows_sys::Win32::Foundation::HWND, folder_name: &str) -> Option<PathBuf> {
    loop {
        let path = show_folder(owner, &format!("Dove creare la cartella {folder_name}?")).ok()??;
        if accept_folder(owner, &path) {
            return Some(path);
        }
    }
}

/// Chiede l'immagine della firma. Si legge e basta: le cartelle temporanee vanno bene.
pub fn pick_image(owner: windows_sys::Win32::Foundation::HWND) -> Option<PathBuf> {
    unsafe {
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let options = dialog.GetOptions().ok()?;
        dialog.SetOptions(options | FOS_FORCEFILESYSTEM).ok()?;
        dialog.SetTitle(w!("Scegli l'immagine della firma")).ok()?;
        let filters = [COMDLG_FILTERSPEC { pszName: w!("Immagini"), pszSpec: w!("*.png;*.jpg;*.jpeg;*.bmp;*.gif;*.webp;*.tif;*.tiff") }];
        dialog.SetFileTypes(&filters).ok()?;
        run_dialog(&dialog.cast().ok()?, owner).ok()?
    }
}

fn accept_folder(owner: windows_sys::Win32::Foundation::HWND, folder: &Path) -> bool {
    if location::is_temporary(folder) {
        win::message_box(owner, TEMP_REFUSED, super::APP_TITLE, MB_OK | MB_ICONWARNING | MB_TOPMOST);
        return false;
    }
    if let Ok(mut last) = LAST_FOLDER.lock() {
        *last = Some(folder.to_path_buf());
    }
    true
}

fn show_save(owner: windows_sys::Win32::Foundation::HWND, suggested: &str) -> windows::core::Result<Option<PathBuf>> {
    unsafe {
        let dialog: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)?;
        // Il tipo di file segue il nome proposto: documento Word o PDF.
        let (title, extension, filters) = if suggested.to_lowercase().ends_with(".docx") {
            (w!("Dove vuoi salvare il documento Word?"), w!("docx"), [COMDLG_FILTERSPEC { pszName: w!("Documento Word"), pszSpec: w!("*.docx") }])
        } else {
            (w!("Dove vuoi salvare il PDF?"), w!("pdf"), [COMDLG_FILTERSPEC { pszName: w!("Documento PDF"), pszSpec: w!("*.pdf") }])
        };
        dialog.SetTitle(title)?;
        dialog.SetFileName(&HSTRING::from(suggested))?;
        dialog.SetDefaultExtension(extension)?;
        dialog.SetFileTypes(&filters)?;
        run_dialog(&dialog.cast()?, owner)
    }
}

/// Chiede la cartella fissa dove salvare i file prodotti. None se l'utente annulla.
pub fn pick_output_folder(owner: windows_sys::Win32::Foundation::HWND) -> Option<PathBuf> {
    loop {
        let path = show_folder(owner, "Cartella dove salvare i file prodotti").ok()??;
        if accept_folder(owner, &path) {
            return Some(path);
        }
    }
}

fn show_folder(owner: windows_sys::Win32::Foundation::HWND, title: &str) -> windows::core::Result<Option<PathBuf>> {
    unsafe {
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
        let options = dialog.GetOptions()?;
        dialog.SetOptions(options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM)?;
        dialog.SetTitle(&HSTRING::from(title))?;
        run_dialog(&dialog.cast()?, owner)
    }
}

unsafe fn run_dialog(dialog: &IFileDialog, owner: windows_sys::Win32::Foundation::HWND) -> windows::core::Result<Option<PathBuf>> {
    unsafe {
        if let Some(start) = start_folder() {
            let _ = dialog.SetFolder(&start);
        }
        // L'annullamento arriva come errore: per noi significa solo nessuna scelta.
        if dialog.Show(Some(HWND(owner))).is_err() {
            return Ok(None);
        }
        let item = dialog.GetResult()?;
        let raw = item.GetDisplayName(SIGDN_FILESYSPATH)?;
        let text = raw.to_string().unwrap_or_default();
        CoTaskMemFree(Some(raw.0 as *const _));
        Ok((!text.is_empty()).then(|| PathBuf::from(text)))
    }
}

/// Si parte dall'ultima cartella scelta, altrimenti da Documenti: mai da una temporanea.
fn start_folder() -> Option<IShellItem> {
    let last = LAST_FOLDER.lock().ok().and_then(|last| last.clone());
    let path = last.or_else(documents_folder)?;
    unsafe { SHCreateItemFromParsingName(&HSTRING::from(path.as_path()), None).ok() }
}

fn documents_folder() -> Option<PathBuf> {
    unsafe {
        let raw = SHGetKnownFolderPath(&FOLDERID_Documents, KF_FLAG_DEFAULT, None).ok()?;
        let text = raw.to_string().ok();
        CoTaskMemFree(Some(raw.0 as *const _));
        text.map(PathBuf::from)
    }
}

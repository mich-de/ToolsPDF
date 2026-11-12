//! Appunti: la firma va come "PNG" (Word e Outlook tengono la trasparenza), come CF_DIBV5
//! con alfa e come CF_DIB appoggiato su bianco per le app che ignorano l'alfa.

use std::thread::sleep;
use std::time::Duration;

use image::RgbaImage;
use windows::core::w;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HWND as WinHwnd};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows_sys::Win32::Foundation::HWND;

use crate::tools::signature;

const CF_DIB: u32 = 8;
const CF_DIBV5: u32 = 17;

/// Mette la firma negli appunti. Falso se gli appunti restano occupati da un'altra app.
pub(crate) fn copy_signature(owner: HWND, png: &[u8], image: &RgbaImage) -> bool {
    unsafe {
        // Un'altra app puo' tenere aperti gli appunti per un attimo.
        let mut opened = false;
        for _ in 0..10 {
            if OpenClipboard(Some(WinHwnd(owner))).is_ok() {
                opened = true;
                break;
            }
            sleep(Duration::from_millis(30));
        }
        if !opened {
            return false;
        }
        let mut ok = EmptyClipboard().is_ok();
        let png_format = RegisterClipboardFormatW(w!("PNG"));
        if ok && png_format != 0 {
            ok &= put(png_format, png);
        }
        // Stessa risoluzione del PNG: in ogni formato la firma ha la stessa misura.
        let dpi = signature::png_dpi(png).unwrap_or(signature::BASE_DPI);
        ok &= put(CF_DIBV5, &signature::dib_v5(image, dpi));
        ok &= put(CF_DIB, &signature::dib_on_white(image, dpi));
        let _ = CloseClipboard();
        ok
    }
}

/// Copia i byte in memoria globale e la cede agli appunti.
unsafe fn put(format: u32, bytes: &[u8]) -> bool {
    unsafe {
        let Ok(memory) = GlobalAlloc(GMEM_MOVEABLE, bytes.len()) else { return false };
        let target = GlobalLock(memory) as *mut u8;
        if target.is_null() {
            let _ = GlobalFree(Some(memory));
            return false;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), target, bytes.len());
        let _ = GlobalUnlock(memory);
        if SetClipboardData(format, Some(HANDLE(memory.0))).is_err() {
            // Solo se gli appunti non l'hanno presa la memoria resta nostra.
            let _ = GlobalFree(Some(memory));
            return false;
        }
        true
    }
}

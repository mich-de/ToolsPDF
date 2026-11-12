//! Ricezione OLE del trascinamento: file veri da Esplora e file virtuali da Outlook.
//!
//! Gli allegati di Outlook non esistono su disco: arrivano come FileGroupDescriptorW piu'
//! FileContents e si leggono direttamente in memoria, senza passare da cartelle temporanee.

use std::cell::Cell;
use std::path::PathBuf;

use windows::core::{implement, Ref, Result as WinResult};
use windows::Win32::Foundation::{DV_E_FORMATETC, HGLOBAL, HWND, POINTL, S_OK};
use windows::Win32::System::Com::{IDataObject, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL, TYMED_ISTREAM, STREAM_SEEK_SET};
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::{
    IDropTarget, IDropTarget_Impl, OleInitialize, RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop, CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::{DragQueryFileW, CFSTR_FILECONTENTS, CFSTR_FILEDESCRIPTORW, FILEDESCRIPTORW, FILEGROUPDESCRIPTORW, HDROP};
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;

use crate::tools::Source;

const OUTLOOK_MESSAGE: &str = "email non elaborabile: trascina l'allegato, non il messaggio";

/// Finestra che riceve il trascinamento: la barra (zone) o il pannello (file aperto, firma, vassoio).
#[derive(Clone, Copy)]
pub enum Target {
    Bar,
    Panel,
}

#[implement(IDropTarget)]
struct DropTarget {
    hwnd: HWND,
    target: Target,
    accepts: Cell<bool>,
}

fn sys_hwnd(hwnd: HWND) -> windows_sys::Win32::Foundation::HWND {
    hwnd.0 as windows_sys::Win32::Foundation::HWND
}

impl DropTarget {
    fn zone_at(&self, pt: &POINTL) -> Option<usize> {
        // Una firma trascinata fuori dal pannello non deve finire nelle zone della barra.
        if super::signatures_panel::is_dragging() {
            return None;
        }
        match self.target {
            Target::Bar => super::zone_at_screen(sys_hwnd(self.hwnd), pt.x, pt.y),
            Target::Panel => super::panel::drop_zone_at(pt.x, pt.y),
        }
    }

    fn show_zone(&self, zone: Option<usize>) {
        match self.target {
            Target::Bar => super::set_drag_zone(sys_hwnd(self.hwnd), zone),
            Target::Panel => super::panel::set_drop_zone(zone),
        }
    }

    fn set_effect(&self, pt: &POINTL, effect: *mut DROPEFFECT) {
        let zone = if self.accepts.get() { self.zone_at(pt) } else { None };
        self.show_zone(zone);
        if !effect.is_null() {
            unsafe { *effect = if zone.is_some() { DROPEFFECT_COPY } else { DROPEFFECT_NONE } };
        }
    }
}

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(&self, data: Ref<IDataObject>, _keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> WinResult<()> {
        self.accepts.set(data.as_ref().is_some_and(has_supported_format));
        self.set_effect(pt, effect);
        Ok(())
    }

    fn DragOver(&self, _keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> WinResult<()> {
        self.set_effect(pt, effect);
        Ok(())
    }

    fn DragLeave(&self) -> WinResult<()> {
        self.show_zone(None);
        Ok(())
    }

    fn Drop(&self, data: Ref<IDataObject>, _keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> WinResult<()> {
        let zone = self.zone_at(pt);
        self.show_zone(None);
        if !effect.is_null() {
            unsafe { *effect = if zone.is_some() { DROPEFFECT_COPY } else { DROPEFFECT_NONE } };
        }
        if let (Some(zone), Some(data)) = (zone, data.as_ref()) {
            let (mut sources, errors) = read_sources(data);
            // In un trascinamento multiplo l'ordine dipende da Esplora: quello alfabetico e' prevedibile.
            sources.sort_by_key(|s: &Source| s.name.to_lowercase());
            match self.target {
                Target::Bar => super::on_drop(zone, sources, errors),
                Target::Panel => super::panel::on_drop(zone, sources, errors),
            }
        }
        Ok(())
    }
}

pub fn register(hwnd: windows_sys::Win32::Foundation::HWND, target: Target) -> Result<(), String> {
    let hwnd = HWND(hwnd);
    unsafe { OleInitialize(None) }.map_err(|e| format!("OLE: {}", e.message()))?;
    let target: IDropTarget = DropTarget { hwnd, target, accepts: Cell::new(false) }.into();
    unsafe { RegisterDragDrop(hwnd, &target) }.map_err(|e| format!("trascinamento: {}", e.message()))
}

pub fn revoke(hwnd: windows_sys::Win32::Foundation::HWND) {
    let _ = unsafe { RevokeDragDrop(HWND(hwnd)) };
}

fn format_etc(format: u16, index: i32, tymed: i32) -> FORMATETC {
    FORMATETC { cfFormat: format, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: index, tymed: tymed as u32 }
}

fn descriptor_format() -> u16 {
    unsafe { RegisterClipboardFormatW(CFSTR_FILEDESCRIPTORW) as u16 }
}

fn contents_format() -> u16 {
    unsafe { RegisterClipboardFormatW(CFSTR_FILECONTENTS) as u16 }
}

fn has_format(data: &IDataObject, format: u16) -> bool {
    let etc = format_etc(format, -1, TYMED_HGLOBAL.0);
    unsafe { data.QueryGetData(&etc) == S_OK }
}

fn has_supported_format(data: &IDataObject) -> bool {
    has_format(data, CF_HDROP.0) || has_format(data, descriptor_format())
}

/// File da elaborare ed errori (nome, messaggio) da mostrare nell'elenco.
fn read_sources(data: &IDataObject) -> (Vec<Source>, Vec<(String, String)>) {
    if let Some(paths) = read_paths(data) {
        return (paths.into_iter().map(Source::from_path).collect(), Vec::new());
    }
    read_virtual_files(data)
}

fn read_paths(data: &IDataObject) -> Option<Vec<PathBuf>> {
    let etc = format_etc(CF_HDROP.0, -1, TYMED_HGLOBAL.0);
    let mut medium = unsafe { data.GetData(&etc) }.ok()?;
    let drop = HDROP(unsafe { medium.u.hGlobal.0 });
    let count = unsafe { DragQueryFileW(drop, u32::MAX, None) };
    let mut paths = Vec::with_capacity(count as usize);
    for i in 0..count {
        let len = unsafe { DragQueryFileW(drop, i, None) } as usize;
        let mut buffer = vec![0u16; len + 1];
        let copied = unsafe { DragQueryFileW(drop, i, Some(&mut buffer)) } as usize;
        paths.push(PathBuf::from(String::from_utf16_lossy(&buffer[..copied])));
    }
    unsafe { ReleaseStgMedium(&mut medium) };
    (!paths.is_empty()).then_some(paths)
}

fn read_virtual_files(data: &IDataObject) -> (Vec<Source>, Vec<(String, String)>) {
    let mut sources = Vec::new();
    let mut errors = Vec::new();
    for (index, name) in descriptor_names(data).into_iter().enumerate() {
        match read_contents(data, index as i32) {
            Ok(bytes) => sources.push(Source::from_memory(&name, bytes)),
            Err(_) if name.to_lowercase().ends_with(".msg") => errors.push((name, OUTLOOK_MESSAGE.to_string())),
            Err(e) => errors.push((name, format!("allegato non leggibile: {}", e.message()))),
        }
    }
    (sources, errors)
}

fn descriptor_names(data: &IDataObject) -> Vec<String> {
    let etc = format_etc(descriptor_format(), -1, TYMED_HGLOBAL.0);
    let Ok(mut medium) = (unsafe { data.GetData(&etc) }) else { return Vec::new() };
    let handle: HGLOBAL = unsafe { medium.u.hGlobal };
    let mut names = Vec::new();
    unsafe {
        let group = GlobalLock(handle) as *const FILEGROUPDESCRIPTORW;
        if !group.is_null() {
            let available = (GlobalSize(handle).saturating_sub(4)) / size_of::<FILEDESCRIPTORW>();
            let count = ((*group).cItems as usize).min(available);
            let first = std::ptr::addr_of!((*group).fgd) as *const FILEDESCRIPTORW;
            for i in 0..count {
                let raw = std::ptr::read_unaligned(first.add(i)).cFileName;
                let len = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
                names.push(String::from_utf16_lossy(&raw[..len]));
            }
            let _ = GlobalUnlock(handle);
        }
        ReleaseStgMedium(&mut medium);
    }
    names
}

fn read_contents(data: &IDataObject, index: i32) -> WinResult<Vec<u8>> {
    let etc = format_etc(contents_format(), index, TYMED_HGLOBAL.0 | TYMED_ISTREAM.0);
    let mut medium = unsafe { data.GetData(&etc)? };
    let result = unsafe {
        if medium.tymed == TYMED_HGLOBAL.0 as u32 {
            read_global(medium.u.hGlobal)
        } else if medium.tymed == TYMED_ISTREAM.0 as u32 {
            match medium.u.pstm.as_ref() {
                Some(stream) => read_stream(stream),
                None => Err(DV_E_FORMATETC.into()),
            }
        } else {
            Err(DV_E_FORMATETC.into())
        }
    };
    unsafe { ReleaseStgMedium(&mut medium) };
    result
}

unsafe fn read_global(handle: HGLOBAL) -> WinResult<Vec<u8>> {
    unsafe {
        let pointer = GlobalLock(handle) as *const u8;
        if pointer.is_null() {
            return Err(DV_E_FORMATETC.into());
        }
        let bytes = std::slice::from_raw_parts(pointer, GlobalSize(handle)).to_vec();
        let _ = GlobalUnlock(handle);
        Ok(bytes)
    }
}

unsafe fn read_stream(stream: &windows::Win32::System::Com::IStream) -> WinResult<Vec<u8>> {
    unsafe {
        let _ = stream.Seek(0, STREAM_SEEK_SET, None);
        let mut bytes = Vec::new();
        let mut chunk = vec![0u8; 1 << 20];
        loop {
            let mut read = 0u32;
            stream.Read(chunk.as_mut_ptr().cast(), chunk.len() as u32, Some(&mut read)).ok()?;
            if read == 0 {
                return Ok(bytes);
            }
            bytes.extend_from_slice(&chunk[..read as usize]);
        }
    }
}

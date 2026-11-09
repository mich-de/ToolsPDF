//! Schermi collegati e scelta di quello che ospita la barra.

use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{LPARAM, RECT};
use windows_sys::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows_sys::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;

use crate::settings::Side;

#[derive(Clone)]
pub(crate) struct Monitor {
    /// Numero del dispositivo (\\.\DISPLAYn): resta lo stesso tra un avvio e l'altro.
    pub number: u32,
    pub rect: RECT,
    pub primary: bool,
    pub dpi: u32,
    /// Nome per le impostazioni, in base alla posizione da sinistra.
    pub label: String,
}

/// Schermi da sinistra a destra.
pub(crate) fn list() -> Vec<Monitor> {
    let mut found: Vec<Monitor> = Vec::new();
    unsafe {
        EnumDisplayMonitors(null_mut(), std::ptr::null(), Some(collect), &mut found as *mut Vec<Monitor> as LPARAM);
    }
    sort_and_label(&mut found);
    found
}

unsafe extern "system" fn collect(handle: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> i32 {
    unsafe {
        let found = &mut *(data as *mut Vec<Monitor>);
        let mut info: MONITORINFOEXW = std::mem::zeroed();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(handle, &mut info as *mut MONITORINFOEXW as *mut _) == 0 {
            return 1;
        }
        let len = info.szDevice.iter().position(|&c| c == 0).unwrap_or(info.szDevice.len());
        let device = String::from_utf16_lossy(&info.szDevice[..len]);
        let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
        if GetDpiForMonitor(handle, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) != 0 {
            dpi_x = 96;
        }
        found.push(Monitor {
            number: device_number(&device).unwrap_or(found.len() as u32 + 1),
            rect: info.monitorInfo.rcMonitor,
            primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
            dpi: dpi_x.max(48),
            label: String::new(),
        });
        1
    }
}

fn device_number(device: &str) -> Option<u32> {
    let digits: String = device.chars().rev().take_while(char::is_ascii_digit).collect::<Vec<_>>().into_iter().rev().collect();
    digits.parse().ok()
}

fn sort_and_label(monitors: &mut [Monitor]) {
    monitors.sort_by_key(|m| (m.rect.left, m.rect.top));
    let count = monitors.len();
    for (index, monitor) in monitors.iter_mut().enumerate() {
        let position = match (count, index) {
            (2, 0) => "Sinistro".to_string(),
            (2, _) => "Destro".to_string(),
            _ => format!("{}", index + 1),
        };
        monitor.label = if monitor.primary && count > 1 { format!("{position} (princ.)") } else { position };
    }
}

/// Schermo per la barra: quello scelto se e' collegato, altrimenti il bordo esterno
/// del desktop sul lato scelto.
pub(crate) fn target(side: Side, choice: Option<u32>) -> Option<Monitor> {
    pick(&list(), side, choice).cloned()
}

fn pick(monitors: &[Monitor], side: Side, choice: Option<u32>) -> Option<&Monitor> {
    if let Some(chosen) = choice.and_then(|n| monitors.iter().find(|m| m.number == n)) {
        return Some(chosen);
    }
    // A parita' di bordo vince il principale, poi quello piu' in alto.
    let primary_first = |m: &&Monitor| (!m.primary, m.rect.top);
    match side {
        Side::Right => monitors.iter().filter(|m| m.rect.right == monitors.iter().map(|o| o.rect.right).max().unwrap_or(0)).min_by_key(primary_first),
        Side::Left => monitors.iter().filter(|m| m.rect.left == monitors.iter().map(|o| o.rect.left).min().unwrap_or(0)).min_by_key(primary_first),
    }
}

/// Schermo che contiene il centro del riquadro.
pub(crate) fn containing(area: &RECT) -> Option<Monitor> {
    let (x, y) = ((area.left + area.right) / 2, (area.top + area.bottom) / 2);
    list().into_iter().find(|m| x >= m.rect.left && x < m.rect.right && y >= m.rect.top && y < m.rect.bottom)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(number: u32, left: i32, width: i32, primary: bool) -> Monitor {
        Monitor { number, rect: RECT { left, top: 0, right: left + width, bottom: 1440 }, primary, dpi: 120, label: String::new() }
    }

    #[test]
    fn automatic_uses_the_outer_edge_of_the_desktop() {
        // Come sul PC di prova: principale a sinistra, secondo schermo a destra.
        let mut list = vec![screen(2, 0, 2560, true), screen(1, 2560, 2560, false)];
        sort_and_label(&mut list);
        assert_eq!(pick(&list, Side::Right, None).unwrap().number, 1);
        assert_eq!(pick(&list, Side::Left, None).unwrap().number, 2);
        assert_eq!(list[0].label, "Sinistro (princ.)");
        assert_eq!(list[1].label, "Destro");
    }

    #[test]
    fn chosen_screen_wins_and_falls_back_when_unplugged() {
        let list = vec![screen(2, 0, 2560, true), screen(1, 2560, 2560, false), screen(3, -1920, 1920, false)];
        assert_eq!(pick(&list, Side::Right, Some(2)).unwrap().number, 2);
        assert_eq!(pick(&list, Side::Right, Some(9)).unwrap().number, 1);
        assert_eq!(pick(&list, Side::Left, Some(9)).unwrap().number, 3);
    }

    #[test]
    fn single_screen_and_device_numbers() {
        let list = vec![screen(1, 0, 1920, true)];
        assert_eq!(pick(&list, Side::Right, None).unwrap().number, 1);
        assert_eq!(device_number(r"\\.\DISPLAY12"), Some(12));
        assert_eq!(device_number("altro"), None);
    }
}

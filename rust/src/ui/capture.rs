//! Cattura di un'area dello schermo per le firme.
//! Lo schermo virtuale (tutti i monitor) viene copiato PRIMA di mostrare la finestra di
//! selezione: l'utente trascina un rettangolo sull'immagine congelata e scurita.
//! Coordinate fisiche (processo per-monitor DPI aware v2). La copia resta in memoria, mai su disco.

use std::cell::RefCell;
use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use std::thread::sleep;
use std::time::Duration;

use image::RgbImage;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, ReleaseCapture, SetCapture, VK_ESCAPE};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use super::paint::{bgra_info, draw_text, fill_round_rect};
use super::win::{rect, wide};
use super::{high_word, low_word, scale, UI};
use crate::tools::signature;

/// Lato minimo della selezione, in pixel fisici.
const MIN_SIDE: i32 = 8;
/// Quanto resta della luminosita' fuori dalla selezione.
const DIM: u32 = 45;
const ESC_TIMER: usize = 1;
const HINT: &str = "Trascina sulla firma  ·  Esc o tasto destro per annullare";

struct Shot {
    hwnd: HWND,
    width: i32,
    height: i32,
    bright: HBITMAP,
    bright_bits: *const u8,
    dark: HBITMAP,
    hint: RECT,
    anchor: Option<POINT>,
    current: POINT,
    result: Option<RECT>,
    done: bool,
}

thread_local! {
    static SHOT: RefCell<Option<Shot>> = const { RefCell::new(None) };
}

/// Nasconde le finestre indicate, copia lo schermo e fa scegliere un'area.
/// None se l'utente annulla o se la copia non riesce.
pub(crate) fn capture(hide: &[HWND]) -> Option<RgbImage> {
    if SHOT.with(|s| s.borrow().is_some()) {
        return None;
    }
    let hidden: Vec<HWND> = hide.iter().copied().filter(|&h| !h.is_null() && unsafe { IsWindowVisible(h) } != 0).collect();
    for &h in &hidden {
        unsafe { ShowWindow(h, SW_HIDE) };
    }
    if !hidden.is_empty() {
        // Il compositore deve ridisegnare lo schermo senza il pannello.
        sleep(Duration::from_millis(200));
    }
    let image = unsafe { run() };
    for &h in &hidden {
        unsafe { ShowWindow(h, SW_SHOWNOACTIVATE) };
    }
    image
}

unsafe fn run() -> Option<RgbImage> {
    unsafe {
        let (left, top) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
        let (width, height) = (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN));
        if width <= 0 || height <= 0 {
            return None;
        }
        let (bright, bright_bits) = grab(left, top, width, height)?;
        let Some(dark) = dimmed(bright_bits, width, height) else {
            DeleteObject(bright);
            return None;
        };
        let hwnd = create_window(left, top, width, height);
        if hwnd.is_null() {
            DeleteObject(bright);
            DeleteObject(dark);
            return None;
        }
        let hint = hint_rect(left, top);
        SHOT.with(|s| {
            *s.borrow_mut() = Some(Shot {
                hwnd,
                width,
                height,
                bright,
                bright_bits,
                dark,
                hint,
                anchor: None,
                current: POINT { x: 0, y: 0 },
                result: None,
                done: false,
            })
        });
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
        SetTimer(hwnd, ESC_TIMER, 50, None);

        let mut message: MSG = zeroed();
        while !SHOT.with(|s| s.borrow().as_ref().is_none_or(|s| s.done)) {
            match GetMessageW(&mut message, null_mut(), 0, 0) {
                0 => {
                    // WM_QUIT arrivato durante la selezione: va ripassato al ciclo principale.
                    PostQuitMessage(message.wParam as i32);
                    break;
                }
                -1 => break,
                _ => {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
        }

        let shot = SHOT.with(|s| s.borrow_mut().take())?;
        KillTimer(shot.hwnd, ESC_TIMER);
        DestroyWindow(shot.hwnd);
        let image = shot.result.and_then(|r| crop(shot.bright_bits, shot.width, r));
        DeleteObject(shot.bright);
        DeleteObject(shot.dark);
        image
    }
}

/// Copia dello schermo virtuale in una DIB 32 bit dall'alto in basso.
unsafe fn grab(left: i32, top: i32, width: i32, height: i32) -> Option<(HBITMAP, *const u8)> {
    unsafe {
        let screen = GetDC(null_mut());
        let (bitmap, bits) = dib(screen, width, height);
        if bitmap.is_null() {
            ReleaseDC(null_mut(), screen);
            return None;
        }
        let mem = CreateCompatibleDC(screen);
        let old = SelectObject(mem, bitmap);
        let ok = BitBlt(mem, 0, 0, width, height, screen, left, top, SRCCOPY | CAPTUREBLT) != 0;
        SelectObject(mem, old);
        DeleteDC(mem);
        ReleaseDC(null_mut(), screen);
        GdiFlush();
        if !ok {
            DeleteObject(bitmap);
            return None;
        }
        Some((bitmap, bits))
    }
}

/// Copia scurita, mostrata fuori dalla selezione.
unsafe fn dimmed(bits: *const u8, width: i32, height: i32) -> Option<HBITMAP> {
    unsafe {
        let screen = GetDC(null_mut());
        let (bitmap, out) = dib(screen, width, height);
        ReleaseDC(null_mut(), screen);
        if bitmap.is_null() {
            return None;
        }
        let len = width as usize * height as usize * 4;
        let source = std::slice::from_raw_parts(bits, len);
        let target = std::slice::from_raw_parts_mut(out as *mut u8, len);
        for (t, &s) in target.iter_mut().zip(source) {
            *t = (s as u32 * DIM / 100) as u8;
        }
        Some(bitmap)
    }
}

unsafe fn dib(dc: HDC, width: i32, height: i32) -> (HBITMAP, *const u8) {
    let info = bgra_info(width, height);
    let mut bits = null_mut();
    let bitmap = unsafe { CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0) };
    if bitmap.is_null() || bits.is_null() {
        return (null_mut(), null());
    }
    (bitmap, bits as *const u8)
}

/// Area scelta, da BGRA a RGB.
unsafe fn crop(bits: *const u8, width: i32, r: RECT) -> Option<RgbImage> {
    let (w, h) = ((r.right - r.left) as usize, (r.bottom - r.top) as usize);
    let stride = width as usize * 4;
    let mut bgra = Vec::with_capacity(w * h * 4);
    for y in r.top as usize..r.bottom as usize {
        let start = y * stride + r.left as usize * 4;
        bgra.extend_from_slice(unsafe { std::slice::from_raw_parts(bits.add(start), w * 4) });
    }
    signature::from_bgra(w as u32, h as u32, &bgra)
}

unsafe fn create_window(left: i32, top: i32, width: i32, height: i32) -> HWND {
    unsafe {
        let instance = GetModuleHandleW(null());
        let class_name = wide("ToolsPdfCapture");
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: null_mut(),
            hCursor: LoadCursorW(null_mut(), IDC_CROSS),
            hbrBackground: null_mut(),
            lpszMenuName: null(),
            lpszClassName: class_name.as_ptr(),
            hIconSm: null_mut(),
        };
        // Alla seconda cattura la classe esiste gia': va bene cosi'.
        RegisterClassExW(&class);
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class_name.as_ptr(),
            null(),
            WS_POPUP,
            left, top, width, height,
            null_mut(), null_mut(), instance, null(),
        )
    }
}

/// Riquadro del suggerimento in alto al centro dello schermo del cursore, in coordinate della finestra.
fn hint_rect(left: i32, top: i32) -> RECT {
    unsafe {
        let mut cursor = POINT { x: 0, y: 0 };
        GetCursorPos(&mut cursor);
        let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = zeroed();
        info.cbSize = size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(monitor, &mut info);
        let m = info.rcMonitor;
        let (w, h, gap) = UI.with(|ui| (scale(ui, 470), scale(ui, 36), scale(ui, 24)));
        let center = (m.left + m.right) / 2 - left;
        rect(center - w / 2, m.top - top + gap, center + w / 2, m.top - top + gap + h)
    }
}

fn selection(shot: &Shot) -> Option<RECT> {
    let a = shot.anchor?;
    let c = shot.current;
    Some(rect(
        a.x.min(c.x).clamp(0, shot.width),
        a.y.min(c.y).clamp(0, shot.height),
        a.x.max(c.x).clamp(0, shot.width),
        a.y.max(c.y).clamp(0, shot.height),
    ))
}

fn grown(r: RECT, by: i32) -> RECT {
    rect(r.left - by, r.top - by, r.right + by, r.bottom + by)
}

fn finish(result: Option<RECT>) {
    SHOT.with(|s| {
        if let Some(shot) = s.borrow_mut().as_mut() {
            shot.result = result;
            shot.done = true;
        }
    });
}

unsafe fn paint(hwnd: HWND) {
    unsafe {
        let mut ps: PAINTSTRUCT = zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);
        SHOT.with(|s| {
            let s = s.borrow();
            let Some(shot) = s.as_ref() else { return };
            let r = ps.rcPaint;
            let mem = CreateCompatibleDC(hdc);
            let old = SelectObject(mem, shot.dark);
            BitBlt(hdc, r.left, r.top, r.right - r.left, r.bottom - r.top, mem, r.left, r.top, SRCCOPY);
            if let Some(sel) = selection(shot) {
                let mut inside = rect(0, 0, 0, 0);
                if IntersectRect(&mut inside, &sel, &r) != 0 {
                    SelectObject(mem, shot.bright);
                    BitBlt(hdc, inside.left, inside.top, inside.right - inside.left, inside.bottom - inside.top, mem, inside.left, inside.top, SRCCOPY);
                }
                let white = CreateSolidBrush(0x00FF_FFFF);
                FrameRect(hdc, &grown(sel, 1), white);
                DeleteObject(white);
            }
            SelectObject(mem, old);
            DeleteDC(mem);

            fill_round_rect(hdc, shot.hint, (shot.hint.bottom - shot.hint.top) / 2, 0x0020_2020);
            SetBkMode(hdc, TRANSPARENT as i32);
            let font = UI.with(|ui| ui.fonts.get().body_bold);
            draw_text(hdc, HINT, shot.hint, font, 0x00FF_FFFF, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        });
        EndPaint(hwnd, &ps);
    }
}

/// Ridisegna la vecchia e la nuova selezione, e il suggerimento se coperto.
unsafe fn update(hwnd: HWND, before: Option<RECT>, after: Option<RECT>, hint: RECT) {
    for r in [before, after].into_iter().flatten() {
        unsafe { InvalidateRect(hwnd, &grown(r, 2), 0) };
    }
    unsafe { InvalidateRect(hwnd, &hint, 0) };
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match message {
            WM_ERASEBKGND => 1,
            WM_PAINT => {
                paint(hwnd);
                0
            }
            WM_LBUTTONDOWN => {
                let point = POINT { x: low_word(lparam), y: high_word(lparam) };
                let changed = SHOT.with(|s| {
                    s.borrow_mut().as_mut().map(|shot| {
                        let before = selection(shot);
                        shot.anchor = Some(point);
                        shot.current = point;
                        (before, shot.hint)
                    })
                });
                if let Some((before, hint)) = changed {
                    SetCapture(hwnd);
                    update(hwnd, before, None, hint);
                }
                0
            }
            WM_MOUSEMOVE => {
                let point = POINT { x: low_word(lparam), y: high_word(lparam) };
                let changed = SHOT.with(|s| {
                    s.borrow_mut().as_mut().filter(|shot| shot.anchor.is_some()).map(|shot| {
                        let before = selection(shot);
                        shot.current = point;
                        (before, selection(shot), shot.hint)
                    })
                });
                if let Some((before, after, hint)) = changed {
                    update(hwnd, before, after, hint);
                }
                0
            }
            WM_LBUTTONUP => {
                ReleaseCapture();
                let chosen = SHOT.with(|s| s.borrow().as_ref().and_then(selection));
                match chosen {
                    Some(r) if r.right - r.left >= MIN_SIDE && r.bottom - r.top >= MIN_SIDE => finish(Some(r)),
                    // Un clic senza trascinare: si ricomincia.
                    _ => {
                        let reset = SHOT.with(|s| {
                            s.borrow_mut().as_mut().map(|shot| (selection(shot), shot.anchor.take(), shot.hint))
                        });
                        if let Some((before, _, hint)) = reset {
                            update(hwnd, before, None, hint);
                        }
                    }
                }
                0
            }
            WM_RBUTTONUP => {
                finish(None);
                0
            }
            WM_KEYDOWN if wparam == VK_ESCAPE as usize => {
                finish(None);
                0
            }
            // Senza il fuoco della tastiera Esc non arriverebbe come messaggio.
            WM_TIMER if wparam == ESC_TIMER => {
                if GetAsyncKeyState(VK_ESCAPE as i32) < 0 {
                    finish(None);
                }
                0
            }
            WM_DPICHANGED => 0,
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

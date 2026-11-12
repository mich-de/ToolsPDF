//! Disegno GDI della barra: intestazione, zone di rilascio, tasti ed elenco dei file.
//! Le primitive qui sotto servono anche al pannello.

use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{COLORREF, ERROR_SUCCESS, HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect;

use super::win::{rect, wide, RectExt};
use super::{
    panel, scale, tray_len, with_rows, Hover, Row, RowState, Ui, APP_TITLE, VERSION, ZONE_COMPRESS, ZONE_COUNT, ZONE_JOIN, ZONE_SIGNATURES, ZONE_SPLIT, ZONE_TEXT, ZONE_TOOLS, ZONE_WORD,
};
use crate::settings::Theme;
use crate::tools;

const HEADER_HEIGHT: i32 = 48;
pub(crate) const MARGIN: i32 = 12;
const ZONE_HEIGHT: i32 = 58;
const ZONE_GAP: i32 = 8;
const JOIN_BAR_HEIGHT: i32 = 36;
const CAPTION_HEIGHT: i32 = 26;
pub const ROW_HEIGHT: i32 = 84;
const CLOSE_SIZE: i32 = 32;
const ICON_BUTTON: i32 = 28;
const FOLDER_SIZE: i32 = 28;
const PROGRESS_HEIGHT: i32 = 6;

const fn rgb(hex: u32) -> COLORREF {
    ((hex >> 16) & 0xFF) | (hex & 0xFF00) | ((hex & 0xFF) << 16)
}

#[derive(Clone, Copy)]
pub(crate) struct Palette {
    pub bg: COLORREF,
    pub card: COLORREF,
    pub fg: COLORREF,
    pub muted: COLORREF,
    pub border: COLORREF,
    pub primary: COLORREF,
    pub on_primary: COLORREF,
    pub tint: COLORREF,
    pub ok: COLORREF,
    pub err: COLORREF,
    pub track: COLORREF,
}

const LIGHT: Palette = Palette {
    bg: rgb(0xF8FAFC),
    card: rgb(0xFFFFFF),
    fg: rgb(0x0F172A),
    muted: rgb(0x475569),
    border: rgb(0xCBD5E1),
    primary: rgb(0x2563EB),
    on_primary: rgb(0xFFFFFF),
    tint: rgb(0xDBEAFE),
    ok: rgb(0x15803D),
    err: rgb(0xDC2626),
    track: rgb(0xE2E8F0),
};

const DARK: Palette = Palette {
    bg: rgb(0x0F172A),
    card: rgb(0x1E293B),
    fg: rgb(0xF1F5F9),
    muted: rgb(0x94A3B8),
    border: rgb(0x475569),
    primary: rgb(0x60A5FA),
    on_primary: rgb(0x0F172A),
    tint: rgb(0x1E3A8A),
    ok: rgb(0x4ADE80),
    err: rgb(0xF87171),
    track: rgb(0x334155),
};

pub(crate) fn palette_for(theme: Theme) -> Palette {
    match theme {
        Theme::Light => LIGHT,
        Theme::Dark => DARK,
        Theme::System => {
            if windows_uses_dark_apps() {
                DARK
            } else {
                LIGHT
            }
        }
    }
}

fn windows_uses_dark_apps() -> bool {
    let key = wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let value = wide("AppsUseLightTheme");
    let mut data: u32 = 1;
    let mut size = size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            (&mut data as *mut u32).cast(),
            &mut size,
        )
    };
    status == ERROR_SUCCESS && data == 0
}

#[derive(Clone, Copy)]
pub(crate) struct FontSet {
    pub title: HFONT,
    pub body_bold: HFONT,
    pub small: HFONT,
}

impl Default for FontSet {
    fn default() -> Self {
        FontSet { title: null_mut(), body_bold: null_mut(), small: null_mut() }
    }
}

impl FontSet {
    pub fn rebuild(&mut self, ui: &Ui) {
        for font in [self.title, self.body_bold, self.small] {
            if !font.is_null() {
                unsafe { DeleteObject(font) };
            }
        }
        self.title = new_font(ui, 15, FW_SEMIBOLD);
        self.body_bold = new_font(ui, 13, FW_SEMIBOLD);
        self.small = new_font(ui, 12, FW_NORMAL);
    }
}

fn new_font(ui: &Ui, size: i32, weight: u32) -> HFONT {
    let face = wide("Segoe UI");
    unsafe {
        CreateFontW(
            -scale(ui, size), 0, 0, 0, weight as i32, 0, 0, 0,
            DEFAULT_CHARSET as u32, OUT_DEFAULT_PRECIS as u32, CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_QUALITY as u32, 0, face.as_ptr(),
        )
    }
}

pub(crate) fn client_rect(hwnd: HWND) -> RECT {
    let mut client = rect(0, 0, 0, 0);
    unsafe { GetClientRect(hwnd, &mut client) };
    client
}

pub(crate) fn close_rect(hwnd: HWND, ui: &Ui) -> RECT {
    let client = client_rect(hwnd);
    let size = scale(ui, CLOSE_SIZE);
    let gap = (scale(ui, HEADER_HEIGHT) - size) / 2;
    rect(client.right - size - gap, gap, client.right - gap, gap + size)
}

fn header_button(hwnd: HWND, ui: &Ui, position: i32) -> RECT {
    let close = close_rect(hwnd, ui);
    let size = scale(ui, ICON_BUTTON);
    let right = close.left - scale(ui, 2) - position * (size + scale(ui, 2));
    let top = (scale(ui, HEADER_HEIGHT) - size) / 2;
    rect(right - size, top, right, top + size)
}

pub(crate) fn settings_rect(hwnd: HWND, ui: &Ui) -> RECT {
    header_button(hwnd, ui, 0)
}

pub(crate) fn info_rect(hwnd: HWND, ui: &Ui) -> RECT {
    header_button(hwnd, ui, 1)
}

/// Riquadro di una zona; quella di Unisci contiene anche i tasti.
pub(crate) fn zone_rect(hwnd: HWND, ui: &Ui, index: usize) -> RECT {
    let s = |v| scale(ui, v);
    let client = client_rect(hwnd);
    let top = s(HEADER_HEIGHT) + index as i32 * (s(ZONE_HEIGHT) + s(ZONE_GAP));
    let extra = if index == ZONE_JOIN { s(JOIN_BAR_HEIGHT) } else { 0 };
    rect(s(MARGIN), top, client.right - s(MARGIN), top + s(ZONE_HEIGHT) + extra)
}

fn join_bar(hwnd: HWND, ui: &Ui) -> RECT {
    let zone = zone_rect(hwnd, ui, ZONE_JOIN);
    rect(zone.left + scale(ui, 8), zone.bottom - scale(ui, JOIN_BAR_HEIGHT), zone.right - scale(ui, 8), zone.bottom - scale(ui, 8))
}

pub(crate) fn join_button(hwnd: HWND, ui: &Ui) -> RECT {
    let bar = join_bar(hwnd, ui);
    rect(bar.left, bar.top, (bar.left + bar.right) / 2 - scale(ui, 3), bar.bottom)
}

pub(crate) fn clear_button(hwnd: HWND, ui: &Ui) -> RECT {
    let bar = join_bar(hwnd, ui);
    rect((bar.left + bar.right) / 2 + scale(ui, 3), bar.top, bar.right, bar.bottom)
}

fn rows_top(hwnd: HWND, ui: &Ui) -> i32 {
    zone_rect(hwnd, ui, ZONE_COUNT - 1).bottom + scale(ui, CAPTION_HEIGHT)
}

fn folder_button(ui: &Ui, right: i32, row_y: i32) -> RECT {
    let size = scale(ui, FOLDER_SIZE);
    let x = right - scale(ui, MARGIN) - size;
    rect(x, row_y, x + size, row_y + size)
}

/// Riga il cui tasto cartella sta sotto il punto indicato.
pub(crate) fn folder_at(hwnd: HWND, ui: &Ui, x: i32, y: i32) -> Option<usize> {
    let top = rows_top(hwnd, ui);
    if y < top {
        return None;
    }
    let client = client_rect(hwnd);
    let row_height = scale(ui, ROW_HEIGHT);
    with_rows(|rows| {
        let mut row_y = top - ui.scroll.get();
        for index in (0..rows.len()).rev() {
            if rows[index].output.is_some() && folder_button(ui, client.right, row_y).contains(x, y) {
                return Some(index);
            }
            row_y += row_height;
        }
        None
    })
    .flatten()
}

/// Intestazione di un'immagine BGRA 32 bit dall'alto in basso, per StretchDIBits e CreateDIBSection.
pub(crate) fn bgra_info(width: i32, height: i32) -> BITMAPINFO {
    BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [RGBQUAD { rgbBlue: 0, rgbGreen: 0, rgbRed: 0, rgbReserved: 0 }],
    }
}

/// Esegue draw su un buffer fuori schermo e lo copia in un colpo: niente sfarfallio.
pub(crate) fn buffered(hwnd: HWND, draw: impl FnOnce(HDC, RECT)) {
    unsafe {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let hdc = BeginPaint(hwnd, &mut ps);
        let client = client_rect(hwnd);
        let (w, h) = (client.right, client.bottom);
        let mem = CreateCompatibleDC(hdc);
        let bitmap = CreateCompatibleBitmap(hdc, w.max(1), h.max(1));
        let old = SelectObject(mem, bitmap);
        SetBkMode(mem, TRANSPARENT as i32);
        draw(mem, client);
        BitBlt(hdc, 0, 0, w, h, mem, 0, 0, SRCCOPY);
        SelectObject(mem, old);
        DeleteObject(bitmap);
        DeleteDC(mem);
        EndPaint(hwnd, &ps);
    }
}

pub(crate) fn paint(hwnd: HWND, ui: &Ui) {
    buffered(hwnd, |dc, client| {
        let colors = ui.palette.get();
        fill_rect(dc, client, colors.bg);
        paint_rows(dc, hwnd, ui, client.right, client.bottom);
        paint_top(dc, hwnd, ui, client.right);
        let edge = if crate::settings::current().side == crate::settings::Side::Left { client.right - scale(ui, 1) } else { 0 };
        fill_rect(dc, rect(edge, 0, edge + scale(ui, 1), client.bottom), colors.border);
    });
}

/// Parte fissa in alto: copre le righe che scorrono sotto.
fn paint_top(dc: HDC, hwnd: HWND, ui: &Ui, w: i32) {
    let s = |v| scale(ui, v);
    let colors = ui.palette.get();
    let fonts = ui.fonts.get();
    fill_rect(dc, rect(0, 0, w, rows_top(hwnd, ui)), colors.bg);

    let info = info_rect(hwnd, ui);
    let text_right = info.left - s(4);
    draw_text(dc, APP_TITLE, rect(s(MARGIN), s(5), text_right, s(26)), fonts.title, colors.fg, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS);
    draw_text(dc, &format!("versione {VERSION}"), rect(s(MARGIN), s(25), text_right, s(42)), fonts.small, colors.muted, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS);

    let hover = ui.hover.get();
    let mode = panel::mode();
    let settings = settings_rect(hwnd, ui);
    let close = close_rect(hwnd, ui);
    for (r, active) in [
        (info, hover == Hover::Info || mode == panel::Mode::Info),
        (settings, hover == Hover::Settings || mode == panel::Mode::Settings),
        (close, hover == Hover::Close),
    ] {
        if active {
            fill_round_rect(dc, r, s(6), colors.track);
        }
    }
    paint_info_icon(dc, ui, info);
    paint_gear_icon(dc, ui, settings);
    let inset = s(11);
    with_pen(dc, PS_SOLID, s(1) + 1, colors.fg, || {
        line(dc, close.left + inset, close.top + inset, close.right - inset, close.bottom - inset);
        line(dc, close.right - inset, close.top + inset, close.left + inset, close.bottom - inset);
    });

    for index in 0..ZONE_COUNT {
        paint_zone(dc, hwnd, ui, index);
    }
    let last = zone_rect(hwnd, ui, ZONE_COUNT - 1);
    draw_text(dc, "PDF, immagini, Word, Excel", rect(s(MARGIN), last.bottom, w - s(MARGIN), rows_top(hwnd, ui)),
        fonts.small, colors.muted, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_CENTER | DT_END_ELLIPSIS);
}

/// Cerchio con la "i".
fn paint_info_icon(dc: HDC, ui: &Ui, r: RECT) {
    let s = |v| scale(ui, v);
    let colors = ui.palette.get();
    let (cx, cy) = ((r.left + r.right) / 2, (r.top + r.bottom) / 2);
    let radius = s(8);
    with_pen(dc, PS_SOLID, s(1) + 1, colors.fg, || unsafe {
        let old = SelectObject(dc, GetStockObject(NULL_BRUSH));
        Ellipse(dc, cx - radius, cy - radius, cx + radius + 1, cy + radius + 1);
        SelectObject(dc, old);
        line(dc, cx, cy - s(1), cx, cy + s(5));
        line(dc, cx, cy - s(5), cx, cy - s(4));
    });
}

/// Ingranaggio: anello con otto denti.
fn paint_gear_icon(dc: HDC, ui: &Ui, r: RECT) {
    let s = |v| scale(ui, v);
    let colors = ui.palette.get();
    let (cx, cy) = ((r.left + r.right) / 2, (r.top + r.bottom) / 2);
    let (inner, outer) = (s(6) as f64, s(9) as f64);
    with_pen(dc, PS_SOLID, s(1) + 1, colors.fg, || unsafe {
        let old = SelectObject(dc, GetStockObject(NULL_BRUSH));
        let ring = inner as i32;
        Ellipse(dc, cx - ring, cy - ring, cx + ring + 1, cy + ring + 1);
        let hole = s(2);
        Ellipse(dc, cx - hole, cy - hole, cx + hole + 1, cy + hole + 1);
        SelectObject(dc, old);
        for step in 0..8 {
            let angle = step as f64 * std::f64::consts::FRAC_PI_4;
            let (sin, cos) = angle.sin_cos();
            line(dc, cx + (cos * inner) as i32, cy + (sin * inner) as i32, cx + (cos * outer) as i32, cy + (sin * outer) as i32);
        }
    });
}

fn paint_zone(dc: HDC, hwnd: HWND, ui: &Ui, index: usize) {
    let s = |v| scale(ui, v);
    let colors = ui.palette.get();
    let fonts = ui.fonts.get();
    let zone = zone_rect(hwnd, ui, index);
    let active = ui.drag_zone.get() == Some(index);
    let hovered = (index == ZONE_JOIN && ui.hover.get() == Hover::JoinZone)
        || (index == ZONE_TOOLS && (ui.hover.get() == Hover::ToolsZone || panel::mode() == panel::Mode::Tools))
        || (index == ZONE_SIGNATURES && (ui.hover.get() == Hover::SignaturesZone || panel::mode() == panel::Mode::Signatures));

    fill_round_rect(dc, zone, s(8), if active || hovered { colors.tint } else { colors.card });
    let style = if active { PS_SOLID } else { PS_DASH };
    with_pen(dc, style, if active { s(2) } else { 1 }, colors.primary, || unsafe {
        let old = SelectObject(dc, GetStockObject(NULL_BRUSH));
        RoundRect(dc, zone.left, zone.top, zone.right, zone.bottom, s(8), s(8));
        SelectObject(dc, old);
    });

    let height = s(ZONE_HEIGHT);
    let icon = rect(zone.left + s(10), zone.top + (height - s(24)) / 2, zone.left + s(34), zone.top + (height + s(24)) / 2);
    paint_zone_icon(dc, ui, index, icon);

    let tray = tray_len();
    let (title, subtitle) = match index {
        ZONE_COMPRESS => ("Comprimi", format!("massimo {}", tools::limit_label())),
        ZONE_SPLIT => ("Dividi", "un PDF per pagina".to_string()),
        ZONE_TEXT => ("Testo", "scansioni selezionabili".to_string()),
        ZONE_WORD => ("Word", "PDF e scansioni in docx".to_string()),
        ZONE_TOOLS => ("Strumenti", "ruota, timbro, firma…".to_string()),
        ZONE_SIGNATURES => ("Firme", "cattura, copia, trascina".to_string()),
        _ => (
            "Unisci",
            match tray {
                0 => "aggiungi i file".to_string(),
                1 => "1 file: aggiungine altri".to_string(),
                n => format!("{n} file: clic per ordinarli"),
            },
        ),
    };
    let left = zone.left + s(42);
    let right = zone.right - s(6);
    draw_text(dc, title, rect(left, zone.top + s(10), right, zone.top + s(29)), fonts.body_bold, colors.fg, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS);
    draw_text(dc, &subtitle, rect(left, zone.top + s(29), right, zone.top + s(48)), fonts.small, colors.muted, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS);

    if index == ZONE_JOIN {
        let hover = ui.hover.get();
        paint_button(dc, ui, join_button(hwnd, ui), "Unisci", tray >= 2, true, hover == Hover::Join);
        paint_button(dc, ui, clear_button(hwnd, ui), "Svuota", tray >= 1, false, hover == Hover::Clear);
    }
}

pub(crate) fn paint_button(dc: HDC, ui: &Ui, r: RECT, label: &str, enabled: bool, primary: bool, hover: bool) {
    let colors = ui.palette.get();
    let fonts = ui.fonts.get();
    let (fill, text) = match (enabled, primary) {
        (false, _) => (colors.track, colors.muted),
        (true, true) => (colors.primary, colors.on_primary),
        (true, false) => (if hover { colors.tint } else { colors.track }, colors.fg),
    };
    fill_round_rect(dc, r, scale(ui, 6), fill);
    draw_text(dc, label, r, fonts.body_bold, text, DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX | DT_END_ELLIPSIS);
}

/// Icone a linee, nitide a ogni DPI.
fn paint_zone_icon(dc: HDC, ui: &Ui, index: usize, r: RECT) {
    let s = |v| scale(ui, v);
    let colors = ui.palette.get();
    let cx = (r.left + r.right) / 2;
    with_pen(dc, PS_SOLID, s(1) + 1, colors.primary, || match index {
        // Freccia che scende su una base: riduzione.
        ZONE_COMPRESS => {
            line(dc, cx, r.top + s(2), cx, r.bottom - s(7));
            line(dc, cx - s(6), r.bottom - s(13), cx, r.bottom - s(7));
            line(dc, cx + s(6), r.bottom - s(13), cx, r.bottom - s(7));
            line(dc, r.left + s(3), r.bottom - s(2), r.right - s(3), r.bottom - s(2));
        }
        // Due fogli sfalsati: pagine separate.
        ZONE_SPLIT => {
            outline(dc, r.left + s(3), r.top + s(2), r.left + s(14), r.top + s(16));
            outline(dc, r.left + s(10), r.top + s(8), r.left + s(21), r.top + s(22));
        }
        // Foglio con righe di testo.
        ZONE_TEXT => {
            outline(dc, r.left + s(4), r.top + s(1), r.right - s(4), r.bottom - s(1));
            for y in [7, 12, 17] {
                line(dc, r.left + s(8), r.top + s(y), r.right - s(8), r.top + s(y));
            }
        }
        // Foglio con una W: documento Word.
        ZONE_WORD => {
            outline(dc, r.left + s(3), r.top + s(1), r.right - s(3), r.bottom - s(1));
            let (top, bottom) = (r.top + s(7), r.bottom - s(7));
            let (left, right) = (r.left + s(6), r.right - s(6));
            let step = (right - left) / 4;
            line(dc, left, top, left + step, bottom);
            line(dc, left + step, bottom, left + 2 * step, top + s(4));
            line(dc, left + 2 * step, top + s(4), left + 3 * step, bottom);
            line(dc, left + 3 * step, bottom, right, top);
        }
        // Tre cursori di regolazione: modifiche al documento.
        ZONE_TOOLS => {
            let (left, right) = (r.left + s(3), r.right - s(3));
            for (y, knob) in [(6, 15), (12, 7), (18, 12)] {
                let y = r.top + s(y);
                line(dc, left, y, right, y);
                let x = r.left + s(knob);
                line(dc, x, y - s(3), x, y + s(3));
            }
        }
        // Svolazzo di firma su una riga.
        ZONE_SIGNATURES => {
            let base = r.bottom - s(4);
            line(dc, r.left + s(2), base, r.right - s(2), base);
            let points = [(2, 15), (6, 6), (9, 16), (12, 8), (15, 14), (18, 10), (22, 12)];
            for pair in points.windows(2) {
                let ((x1, y1), (x2, y2)) = (pair[0], pair[1]);
                line(dc, r.left + s(x1), r.top + s(y1), r.left + s(x2), r.top + s(y2));
            }
        }
        // Piu': aggiunta al vassoio.
        _ => {
            let cy = (r.top + r.bottom) / 2;
            line(dc, cx - s(9), cy, cx + s(9), cy);
            line(dc, cx, cy - s(9), cx, cy + s(9));
        }
    });
}

pub(crate) fn outline(dc: HDC, left: i32, top: i32, right: i32, bottom: i32) {
    line(dc, left, top, right, top);
    line(dc, right, top, right, bottom);
    line(dc, right, bottom, left, bottom);
    line(dc, left, bottom, left, top);
}

/// Mostra per primo l'ultimo file aggiunto, cosi' resta visibile senza scorrere.
fn paint_rows(dc: HDC, hwnd: HWND, ui: &Ui, w: i32, h: i32) {
    let row_height = scale(ui, ROW_HEIGHT);
    let top = rows_top(hwnd, ui);
    with_rows(|rows| {
        let max_scroll = (rows.len() as i32 * row_height - (h - top)).max(0);
        let offset = ui.scroll.get().clamp(0, max_scroll);
        ui.scroll.set(offset);
        let mut y = top - offset;
        for index in (0..rows.len()).rev() {
            if y + row_height > top && y < h {
                paint_row(dc, ui, &rows[index], index, y, w);
            }
            y += row_height;
        }
    });
}

fn paint_row(dc: HDC, ui: &Ui, row: &Row, index: usize, y: i32, w: i32) {
    let s = |v| scale(ui, v);
    let colors = ui.palette.get();
    let fonts = ui.fonts.get();
    let (left, right) = (s(MARGIN), w - s(MARGIN));
    let (text_color, prefix, fill) = match row.state {
        RowState::Done => (colors.ok, "OK  ", colors.ok),
        RowState::Failed => (colors.err, "Errore  ", colors.err),
        RowState::Cancelled => (colors.muted, "", colors.muted),
        _ => (colors.muted, "", colors.primary),
    };

    let mut name_right = right;
    if row.output.is_some() {
        let button = folder_button(ui, w, y);
        name_right = button.left - s(4);
        if ui.hover.get() == Hover::Folder(index) {
            fill_round_rect(dc, button, s(6), colors.track);
        }
        paint_folder_icon(dc, ui, button);
    }
    draw_text(dc, &row.name, rect(left, y + s(4), name_right, y + s(24)), fonts.body_bold, colors.fg,
        DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS);

    let bar = rect(left, y + s(30), right, y + s(30) + s(PROGRESS_HEIGHT));
    fill_round_rect(dc, bar, s(PROGRESS_HEIGHT), colors.track);
    let width = ((bar.right - bar.left) as f64 * row.pct) as i32;
    if row.state != RowState::Queued && width > 0 {
        let done = rect(bar.left, bar.top, bar.left + width.max(s(PROGRESS_HEIGHT)), bar.bottom);
        fill_round_rect(dc, done, s(PROGRESS_HEIGHT), fill);
    }

    let status = format!("{prefix}{}", row.status);
    draw_text(dc, &status, rect(left, y + s(40), right, y + s(ROW_HEIGHT) - s(2)), fonts.small, text_color,
        DT_WORDBREAK | DT_EDITCONTROL | DT_END_ELLIPSIS | DT_NOPREFIX);
}

/// Cartella con linguetta: apre dove e' stato salvato il file.
fn paint_folder_icon(dc: HDC, ui: &Ui, r: RECT) {
    let s = |v| scale(ui, v);
    let colors = ui.palette.get();
    let (l, t, right, b) = (r.left + s(6), r.top + s(8), r.right - s(6), r.bottom - s(7));
    with_pen(dc, PS_SOLID, s(1) + 1, colors.primary, || {
        line(dc, l, t, l + s(6), t);
        line(dc, l + s(6), t, l + s(8), t + s(3));
        line(dc, l + s(8), t + s(3), right, t + s(3));
        line(dc, right, t + s(3), right, b);
        line(dc, right, b, l, b);
        line(dc, l, b, l, t);
    });
}

pub(crate) fn fill_rect(dc: HDC, r: RECT, color: COLORREF) {
    unsafe {
        let brush = CreateSolidBrush(color);
        FillRect(dc, &r, brush);
        DeleteObject(brush);
    }
}

pub(crate) fn fill_round_rect(dc: HDC, r: RECT, radius: i32, color: COLORREF) {
    unsafe {
        let brush = CreateSolidBrush(color);
        let old = SelectObject(dc, brush);
        with_pen(dc, PS_SOLID, 1, color, || {
            RoundRect(dc, r.left, r.top, r.right, r.bottom, radius, radius);
        });
        SelectObject(dc, old);
        DeleteObject(brush);
    }
}

pub(crate) fn with_pen(dc: HDC, style: PEN_STYLE, width: i32, color: COLORREF, draw: impl FnOnce()) {
    unsafe {
        let pen = CreatePen(style, width, color);
        let old = SelectObject(dc, pen);
        draw();
        SelectObject(dc, old);
        DeleteObject(pen);
    }
}

pub(crate) fn line(dc: HDC, x1: i32, y1: i32, x2: i32, y2: i32) {
    unsafe {
        MoveToEx(dc, x1, y1, null_mut());
        LineTo(dc, x2, y2);
    }
}

pub(crate) fn draw_text(dc: HDC, text: &str, mut r: RECT, font: HFONT, color: COLORREF, flags: DRAW_TEXT_FORMAT) {
    let utf16: Vec<u16> = text.encode_utf16().collect();
    if utf16.is_empty() {
        return;
    }
    unsafe {
        let old = SelectObject(dc, font);
        SetTextColor(dc, color);
        DrawTextW(dc, utf16.as_ptr(), utf16.len() as i32, &mut r, flags);
        SelectObject(dc, old);
    }
}

/// Altezza che occupa un testo a capo automatico nella larghezza data.
pub(crate) fn text_height(dc: HDC, text: &str, width: i32, font: HFONT) -> i32 {
    let utf16: Vec<u16> = text.encode_utf16().collect();
    if utf16.is_empty() {
        return 0;
    }
    let mut r = rect(0, 0, width, 0);
    unsafe {
        let old = SelectObject(dc, font);
        DrawTextW(dc, utf16.as_ptr(), utf16.len() as i32, &mut r, DT_WORDBREAK | DT_CALCRECT | DT_NOPREFIX);
        SelectObject(dc, old);
    }
    r.bottom - r.top
}

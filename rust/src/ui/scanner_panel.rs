//! Pannello Scanner: usa lo scanner WIA di sistema (se collegato) per acquisire pagine
//! da vetro o alimentatore ADF in memoria e inviarle direttamente a Testo (OCR) o Comprimi.

use std::sync::Mutex;
use std::thread;

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::*;

use super::paint::{client_rect, draw_text, fill_round_rect, outline, paint_button, text_height, with_pen, MARGIN};
use super::panel::HEADER;
use super::signatures_panel::popup;
use super::win::{rect, RectExt};
use super::{refresh_scanners, request_repaint, scale, scanners_list, send_scanned_pdf, Ui};
use crate::tools::scanner::{self, ColorMode, ScanOptions, ScanSource, DPI_CHOICES};
use crate::tools::Operation;

const FIELD: i32 = 30;
const LABEL: i32 = 20;
const BUTTON: i32 = 36;
const SMALL_BUTTON: i32 = 28;
const CHOICE_H: i32 = 30;

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum ScanHit {
    Nothing,
    Device,
    Refresh,
    Source(ScanSource),
    Color(ColorMode),
    Dpi(u32),
    Acquire,
    RemoveLast,
    ClearPages,
    SendText,
    SendCompress,
}

struct ScanState {
    selected_id: Option<String>,
    options: ScanOptions,
    /// Pagine grezze acquisite in memoria (BMP/PNG/JPEG) con i rispettivi DPI.
    pages: Vec<(Vec<u8>, u32)>,
    busy: bool,
    note: String,
    note_err: bool,
}

impl Default for ScanState {
    fn default() -> Self {
        ScanState {
            selected_id: None,
            options: ScanOptions::default(),
            pages: Vec::new(),
            busy: false,
            note: String::new(),
            note_err: false,
        }
    }
}

static STATE: Mutex<Option<ScanState>> = Mutex::new(None);

fn with_state<T>(action: impl FnOnce(&mut ScanState) -> T) -> T {
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let state = guard.get_or_insert_with(ScanState::default);
    action(state)
}

struct Layout {
    dev_label: RECT,
    device: RECT,
    refresh: RECT,
    src_label: RECT,
    src_flatbed: RECT,
    src_feeder: RECT,
    col_label: RECT,
    col_choices: [RECT; 3],
    dpi_label: RECT,
    dpi_choices: [RECT; 3],
    acquire: RECT,
    status_box: RECT,
    remove_last: RECT,
    clear_pages: RECT,
    send_text: RECT,
    send_compress: RECT,
    note: RECT,
}

fn layout(hwnd: HWND, ui: &Ui) -> Layout {
    let s = |v| scale(ui, v);
    let client = client_rect(hwnd);
    let (left, right) = (s(MARGIN), client.right - s(MARGIN));
    let w = right - left;
    let mut y = s(HEADER) + s(MARGIN);

    let dev_label = rect(left, y, right, y + s(LABEL));
    y = dev_label.bottom + s(2);
    let refresh_w = s(78);
    let device = rect(left, y, right - refresh_w - s(6), y + s(FIELD));
    let refresh = rect(device.right + s(6), y, right, y + s(FIELD));
    y = device.bottom + s(12);

    let src_label = rect(left, y, right, y + s(LABEL));
    y = src_label.bottom + s(2);
    let mid = (left + right) / 2;
    let src_flatbed = rect(left, y, mid - s(3), y + s(CHOICE_H));
    let src_feeder = rect(mid + s(3), y, right, y + s(CHOICE_H));
    y = src_flatbed.bottom + s(12);

    let col_label = rect(left, y, right, y + s(LABEL));
    y = col_label.bottom + s(2);
    let third = w / 3;
    let col_choices = [
        rect(left, y, left + third - s(3), y + s(CHOICE_H)),
        rect(left + third + s(2), y, left + 2 * third - s(2), y + s(CHOICE_H)),
        rect(left + 2 * third + s(3), y, right, y + s(CHOICE_H)),
    ];
    y = col_choices[0].bottom + s(12);

    let dpi_label = rect(left, y, right, y + s(LABEL));
    y = dpi_label.bottom + s(2);
    let dpi_choices = [
        rect(left, y, left + third - s(3), y + s(CHOICE_H)),
        rect(left + third + s(2), y, left + 2 * third - s(2), y + s(CHOICE_H)),
        rect(left + 2 * third + s(3), y, right, y + s(CHOICE_H)),
    ];
    y = dpi_choices[0].bottom + s(14);

    let acquire = rect(left, y, right, y + s(BUTTON));
    y = acquire.bottom + s(10);

    let status_box = rect(left, y, right, y + s(44));
    y = status_box.bottom + s(6);
    let remove_last = rect(left, y, mid - s(3), y + s(SMALL_BUTTON));
    let clear_pages = rect(mid + s(3), y, right, y + s(SMALL_BUTTON));
    y = remove_last.bottom + s(16);

    let send_text = rect(left, y, right, y + s(BUTTON));
    y = send_text.bottom + s(8);
    let send_compress = rect(left, y, right, y + s(BUTTON));
    y = send_compress.bottom + s(12);

    let note = rect(left, y, right, (client.bottom - s(MARGIN)).max(y + s(40)));
    Layout {
        dev_label, device, refresh, src_label, src_flatbed, src_feeder,
        col_label, col_choices, dpi_label, dpi_choices,
        acquire, status_box, remove_last, clear_pages, send_text, send_compress, note,
    }
}

pub(super) fn hit_test(hwnd: HWND, ui: &Ui, x: i32, y: i32) -> ScanHit {
    let l = layout(hwnd, ui);
    let (busy, has_pages) = with_state(|st| (st.busy, !st.pages.is_empty()));
    if busy {
        return ScanHit::Nothing;
    }
    if l.device.contains(x, y) { return ScanHit::Device; }
    if l.refresh.contains(x, y) { return ScanHit::Refresh; }
    if l.src_flatbed.contains(x, y) { return ScanHit::Source(ScanSource::Flatbed); }
    if l.src_feeder.contains(x, y) { return ScanHit::Source(ScanSource::Feeder); }
    for (r, mode) in l.col_choices.iter().zip([ColorMode::Color, ColorMode::Gray, ColorMode::BlackWhite]) {
        if r.contains(x, y) { return ScanHit::Color(mode); }
    }
    for (r, dpi) in l.dpi_choices.iter().zip(DPI_CHOICES) {
        if r.contains(x, y) { return ScanHit::Dpi(dpi); }
    }
    if l.acquire.contains(x, y) { return ScanHit::Acquire; }
    if has_pages && l.remove_last.contains(x, y) { return ScanHit::RemoveLast; }
    if has_pages && l.clear_pages.contains(x, y) { return ScanHit::ClearPages; }
    if l.send_text.contains(x, y) { return ScanHit::SendText; }
    if l.send_compress.contains(x, y) { return ScanHit::SendCompress; }
    ScanHit::Nothing
}

pub(super) fn paint(dc: HDC, hwnd: HWND, ui: &Ui, _client: RECT, hover: ScanHit) {
    let s = |v| scale(ui, v);
    let colors = ui.palette.get();
    let fonts = ui.fonts.get();
    let l = layout(hwnd, ui);
    let scanners = scanners_list();

    let (selected_name, opts, page_count, busy, note, note_err) = with_state(|st| {
        let chosen = st
            .selected_id
            .as_deref()
            .and_then(|id| scanners.iter().find(|sc| sc.id == id))
            .or_else(|| scanners.first());
        let name = chosen.map_or_else(|| "Nessuno scanner trovato".to_string(), |sc| sc.name.clone());
        if st.selected_id.is_none() {
            st.selected_id = chosen.map(|sc| sc.id.clone());
        }
        (name, st.options, st.pages.len(), st.busy, st.note.clone(), st.note_err)
    });
    let has_scanner = !scanners.is_empty();

    draw_text(dc, "Scanner", l.dev_label, fonts.body_bold, colors.fg, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);
    fill_round_rect(dc, l.device, s(6), if hover == ScanHit::Device && scanners.len() > 1 { colors.track } else { colors.card });
    with_pen(dc, PS_SOLID, 1, colors.border, || outline(dc, l.device.left, l.device.top, l.device.right - 1, l.device.bottom - 1));
    let name_box = rect(l.device.left + s(8), l.device.top, l.device.right - s(20), l.device.bottom);
    draw_text(dc, &selected_name, name_box, fonts.small, if has_scanner { colors.fg } else { colors.muted }, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS);
    if scanners.len() > 1 {
        let arrow = rect(l.device.right - s(20), l.device.top, l.device.right - s(4), l.device.bottom);
        draw_text(dc, "▾", arrow, fonts.small, colors.fg, DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX);
    }
    paint_button(dc, ui, l.refresh, "Aggiorna", !busy, false, hover == ScanHit::Refresh);

    draw_text(dc, "Sorgente", l.src_label, fonts.body_bold, colors.fg, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);
    paint_choice(dc, ui, l.src_flatbed, "Vetro (1 pag.)", opts.source == ScanSource::Flatbed, hover == ScanHit::Source(ScanSource::Flatbed));
    paint_choice(dc, ui, l.src_feeder, "Alimentatore (ADF)", opts.source == ScanSource::Feeder, hover == ScanHit::Source(ScanSource::Feeder));

    draw_text(dc, "Colore", l.col_label, fonts.body_bold, colors.fg, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);
    for ((r, mode), label) in l.col_choices.iter().zip([ColorMode::Color, ColorMode::Gray, ColorMode::BlackWhite]).zip(["Colore", "Grigi", "B/N"]) {
        paint_choice(dc, ui, *r, label, opts.color == mode, hover == ScanHit::Color(mode));
    }

    draw_text(dc, "Risoluzione", l.dpi_label, fonts.body_bold, colors.fg, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);
    for (r, dpi) in l.dpi_choices.iter().zip(DPI_CHOICES) {
        paint_choice(dc, ui, *r, &format!("{dpi} DPI"), opts.dpi == dpi, hover == ScanHit::Dpi(dpi));
    }

    let acquire_label = match (busy, opts.source, page_count) {
        (true, _, _) => "Scansione in corso…",
        (false, ScanSource::Feeder, 0) => "Acquisisci fogli da ADF",
        (false, ScanSource::Feeder, _) => "Aggiungi fogli da ADF",
        (false, ScanSource::Flatbed, 0) => "Acquisisci pagina",
        (false, ScanSource::Flatbed, _) => "Aggiungi un'altra pagina",
    };
    paint_button(dc, ui, l.acquire, acquire_label, has_scanner && !busy, false, hover == ScanHit::Acquire);

    fill_round_rect(dc, l.status_box, s(6), colors.card);
    with_pen(dc, PS_SOLID, 1, colors.border, || outline(dc, l.status_box.left, l.status_box.top, l.status_box.right - 1, l.status_box.bottom - 1));
    let summary = match page_count {
        0 => "Nessuna pagina acquisita.\nPuoi acquisire più pagine o premere subito sotto.".to_string(),
        1 => "1 pagina pronta in memoria (nessun file temporaneo).".to_string(),
        n => format!("{n} pagine pronte in memoria (nessun file temporaneo)."),
    };
    draw_text(dc, &summary, rect(l.status_box.left + s(8), l.status_box.top + s(4), l.status_box.right - s(8), l.status_box.bottom - s(4)), fonts.small, colors.muted, DT_WORDBREAK | DT_NOPREFIX);

    paint_button(dc, ui, l.remove_last, "Togli ultima", page_count > 0 && !busy, false, hover == ScanHit::RemoveLast);
    paint_button(dc, ui, l.clear_pages, "Svuota", page_count > 0 && !busy, false, hover == ScanHit::ClearPages);

    let text_label = if page_count > 0 { "Invia a Testo (OCR)" } else { "Scansiona → Testo (OCR)" };
    let comp_label = if page_count > 0 { "Invia a Comprimi" } else { "Scansiona → Comprimi" };
    paint_button(dc, ui, l.send_text, text_label, has_scanner && !busy, true, hover == ScanHit::SendText);
    paint_button(dc, ui, l.send_compress, comp_label, has_scanner && !busy, true, hover == ScanHit::SendCompress);

    if !note.is_empty() {
        let h = text_height(dc, &note, l.note.right - l.note.left, fonts.small);
        draw_text(dc, &note, rect(l.note.left, l.note.top, l.note.right, l.note.top + h), fonts.small, if note_err { colors.err } else { colors.ok }, DT_WORDBREAK | DT_NOPREFIX);
    }
}

fn paint_choice(dc: HDC, ui: &Ui, r: RECT, label: &str, selected: bool, hover: bool) {
    let colors = ui.palette.get();
    let fonts = ui.fonts.get();
    let fill = if selected { colors.primary } else if hover { colors.tint } else { colors.track };
    let fg = if selected { colors.on_primary } else { colors.fg };
    fill_round_rect(dc, r, scale(ui, 6), fill);
    draw_text(dc, label, r, fonts.small, fg, DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX | DT_END_ELLIPSIS);
}

pub(super) fn on_click(hwnd: HWND, ui: &Ui, hit: ScanHit) {
    match hit {
        ScanHit::Device => pick_device(hwnd, ui),
        ScanHit::Refresh => {
            with_state(|st| {
                st.note = "Ricerca scanner…".into();
                st.note_err = false;
            });
            refresh_scanners();
        }
        ScanHit::Source(src) => with_state(|st| st.options.source = src),
        ScanHit::Color(col) => with_state(|st| st.options.color = col),
        ScanHit::Dpi(dpi) => with_state(|st| st.options.dpi = dpi),
        ScanHit::Acquire => start_scan(None),
        ScanHit::RemoveLast => with_state(|st| {
            st.pages.pop();
            st.note.clear();
        }),
        ScanHit::ClearPages => with_state(|st| {
            st.pages.clear();
            st.note.clear();
        }),
        ScanHit::SendText => send_or_scan(Operation::Text),
        ScanHit::SendCompress => send_or_scan(Operation::Compress),
        ScanHit::Nothing => {}
    }
}

fn pick_device(hwnd: HWND, ui: &Ui) {
    let scanners = scanners_list();
    if scanners.len() <= 1 {
        return;
    }
    let l = layout(hwnd, ui);
    let mut at = POINT { x: l.device.left, y: l.device.bottom };
    unsafe { ClientToScreen(hwnd, &mut at) };
    let items: Vec<(usize, String)> = scanners.iter().enumerate().map(|(i, sc)| (i + 1, sc.name.clone())).collect();
    let choice = popup(hwnd, &items, at);
    if let Some(sc) = choice.checked_sub(1).and_then(|i| scanners.get(i)) {
        with_state(|st| st.selected_id = Some(sc.id.clone()));
    }
}

fn send_or_scan(op: Operation) {
    let existing = with_state(|st| {
        if st.busy || st.pages.is_empty() {
            return None;
        }
        Some(std::mem::take(&mut st.pages))
    });
    if let Some(pages) = existing {
        dispatch_pages(op, pages);
    } else {
        start_scan(Some(op));
    }
}

fn start_scan(send_after: Option<Operation>) {
    let params = with_state(|st| {
        if st.busy {
            return None;
        }
        st.busy = true;
        st.note = "Scansione in corso…".into();
        st.note_err = false;
        Some((st.selected_id.clone(), st.options))
    });
    let Some((id, options)) = params else { return };
    request_repaint();
    thread::spawn(move || {
        let result = scanner::scan(id.as_deref(), options);
        match result {
            Ok(raw_pages) => {
                let added = raw_pages.len();
                let dpi = options.dpi;
                if let Some(op) = send_after {
                    let all_pages = with_state(|st| {
                        st.busy = false;
                        let mut combined = std::mem::take(&mut st.pages);
                        combined.extend(raw_pages.into_iter().map(|b| (b, dpi)));
                        st.note = format!("{} inviate alla barra.", page_word(combined.len()));
                        st.note_err = false;
                        combined
                    });
                    dispatch_pages(op, all_pages);
                } else {
                    with_state(|st| {
                        st.busy = false;
                        st.pages.extend(raw_pages.into_iter().map(|b| (b, dpi)));
                        st.note = format!("Aggiunte {} (totale: {}).", page_word(added), st.pages.len());
                        st.note_err = false;
                    });
                }
            }
            Err(err) => {
                with_state(|st| {
                    st.busy = false;
                    st.note = err;
                    st.note_err = true;
                });
            }
        }
        request_repaint();
    });
}

fn dispatch_pages(op: Operation, pages: Vec<(Vec<u8>, u32)>) {
    let dpi = pages.first().map_or(200, |(_, d)| *d);
    let raw: Vec<Vec<u8>> = pages.into_iter().map(|(b, _)| b).collect();
    match scanner::pages_to_pdf(&raw, dpi) {
        Ok(pdf) => {
            with_state(|st| {
                st.note = format!("{} inviate a {}.", page_word(raw.len()), if op == Operation::Text { "Testo (OCR)" } else { "Comprimi" });
                st.note_err = false;
            });
            send_scanned_pdf(op, pdf);
        }
        Err(err) => {
            with_state(|st| {
                st.note = match err {
                    crate::tools::Error::Message(m) => m,
                    crate::tools::Error::Cancelled => "Operazione annullata".into(),
                };
                st.note_err = true;
            });
        }
    }
    request_repaint();
}

pub(super) fn on_scanners_refreshed(count: usize) {
    with_state(|st| {
        if st.note == "Ricerca scanner…" {
            st.note = match count {
                0 => "Nessuno scanner rilevato.".into(),
                1 => "1 scanner rilevato.".into(),
                n => format!("{n} scanner rilevati."),
            };
            st.note_err = count == 0;
        }
    });
}

fn page_word(n: usize) -> String {
    if n == 1 { "1 pagina".into() } else { format!("{n} pagine") }
}


//! Editor della foto tessera nel pannello (zona "Foto tessera"): formato con guide ICAO,
//! trascina = sposta, rotella = zoom, messa a fuoco, sfondo bianco o trasparente, Salva.
//! Il lavoro sulle immagini sta in tools/portrait.rs.

use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use super::paint::{
    bgra_info, client_rect, draw_text, eraser_radius, fill_round_rect, line, outline, paint_brush, paint_button, paint_eraser_size, paint_slider, set_eraser_size_at, slider_fraction, text_height, with_pen, MARGIN,
};
use super::panel::HEADER;
use super::signatures_panel::popup;
use super::win::{rect, Cell2, RectExt};
use super::{beside, clipboard, dialogs, error_text, scale, Ui, SLOT_PHOTO};
use crate::tools::portrait::{self, Background, Crop, Photo, Stroke, DPI, FORMATS, SHARPEN_DEFAULT, SHARPEN_MAX};
use crate::tools::signature;
use crate::tools::Source;

const FIELD: i32 = 30;
const LABEL: i32 = 18;
const SLIDER: i32 = 28;
const SMALL_BUTTON: i32 = 26;
const BUTTON: i32 = 36;
const MAX_FRAME: i32 = 340;
const CHECKER: i32 = 6;
const DRAG_THRESHOLD: i32 = 3;
/// Lo slider dello zoom va da 4 volte piu' largo a 4 volte piu' stretto dell'inquadratura Adatta.
const ZOOM_SPAN: f32 = 4.0;
/// Ogni scatto della rotella.
const WHEEL_STEP: f32 = 1.12;
const HINT: &str = "trascina = sposta, rotella = zoom";
const ERASE_HINT: &str = "trascina = cancella, rotella = zoom (solo vista)";
/// Zoom massimo della vista con la gomma, rispetto al ritaglio.
const ERASE_ZOOM_MAX: f32 = 6.0;
const EMPTY: &str = "Trascina qui una foto (o sulla zona Foto tessera della barra).\n\nLo sfondo si stacca da solo; scegli il formato, sistema il volto sulle guide e premi Salva foto.";

/// Zona di rilascio del pannello in modalita' Foto tessera.
pub(crate) const DROP_PHOTO: usize = 4;

/// Elemento dell'editor sotto il mouse.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum PhotoHit {
    Nothing,
    Format,
    Frame,
    Zoom,
    Fit,
    /// Gomma magica, Gomma (attivano o spengono), Annulla l'ultima passata.
    Magic,
    Eraser,
    Undo,
    /// Misura della gomma: al posto dello zoom con la gomma attiva.
    Size,
    Sharpen,
    Background(Background),
    Save,
    Discard,
}

struct Draft {
    photo: Photo,
    /// Nome e percorso del file d'origine (senza percorso se arriva da Outlook).
    name: String,
    path: Option<PathBuf>,
    format: usize,
    crop: Crop,
    /// Inquadratura di Adatta: riferimento dello slider zoom.
    fit: Crop,
    background: Background,
    sharpen: f32,
    /// Gomma attiva (Some(true) = magica) e passata in corso (punti del pannello).
    tool: Option<bool>,
    live: Vec<POINT>,
    /// Con la gomma: zoom solo di vista (il ritaglio crop non cambia). None = si vede il ritaglio.
    view: Option<Crop>,
    /// Anteprima composta e cio' da cui dipende.
    cache: RefCell<Option<(Crop, i32, i32, Background, Vec<u8>)>>,
}

#[derive(Clone, Copy)]
enum Drag {
    Pan { start: POINT, crop: Crop, moved: bool },
    Zoom,
    Size,
    Sharpen,
}

struct PhotoUi {
    draft: RefCell<Option<Draft>>,
    /// Formato, messa a fuoco e sfondo dell'ultima foto: valgono per la successiva.
    format: Cell2<usize>,
    sharpen: Cell2<f32>,
    background: Cell2<Background>,
    drag: Cell2<Option<Drag>>,
    /// Mouse sul riquadro, per il cerchio della gomma.
    pointer: Cell2<Option<POINT>>,
    note: RefCell<(String, bool)>,
}

thread_local! {
    static PHOTO: PhotoUi = PhotoUi {
        draft: RefCell::new(None),
        format: Cell2::new(0),
        sharpen: Cell2::new(SHARPEN_DEFAULT),
        background: Cell2::new(Background::White),
        drag: Cell2::new(None),
        pointer: Cell2::new(None),
        note: RefCell::new((String::new(), false)),
    };
}

fn note(text: &str, error: bool) {
    PHOTO.with(|g| *g.note.borrow_mut() = (text.to_string(), error));
}

pub(super) fn has_photo() -> bool {
    PHOTO.with(|g| g.draft.borrow().is_some())
}

/// Foto rilasciata sulla zona o sul pannello: sfondo staccato (mezzo secondo, cursore attesa).
/// Se non va, resta la foto di prima e compare l'errore.
pub(super) fn load_dropped(source: Source) {
    if let Err(error) = load(source) {
        note(&format!("Foto non utilizzabile: {error}"), true);
    }
}

fn load(source: Source) -> Result<(), String> {
    let Source { path, bytes, name, .. } = source;
    let data = match (bytes, &path) {
        (Some(bytes), _) => bytes,
        (None, Some(path)) => fs::read(path).map_err(|e| e.to_string())?,
        (None, None) => return Err("file vuoto".into()),
    };
    unsafe { SetCursor(LoadCursorW(null_mut(), IDC_WAIT)) };
    let sharpen = PHOTO.with(|g| g.sharpen.get());
    let photo = portrait::decode(&data).and_then(|image| Photo::new(image, sharpen)).map_err(error_text)?;
    let format = PHOTO.with(|g| g.format.get()).min(FORMATS.len() - 1);
    let background = PHOTO.with(|g| g.background.get());
    let fit = photo.fit(&FORMATS[format]);
    let draft = Draft { photo, name, path, format, crop: fit, fit, background, sharpen, tool: None, live: Vec::new(), view: None, cache: RefCell::new(None) };
    PHOTO.with(|g| *g.draft.borrow_mut() = Some(draft));
    note("", false);
    Ok(())
}

fn with_draft<T>(action: impl FnOnce(&mut Draft) -> T) -> Option<T> {
    PHOTO.with(|g| g.draft.borrow_mut().as_mut().map(action))
}

impl Draft {
    /// Parte della foto nel riquadro: la vista della gomma o il ritaglio.
    fn shown(&self) -> Crop {
        self.view.unwrap_or(self.crop)
    }
}

// ---------- geometria ----------

struct Layout {
    format_label: RECT,
    format: RECT,
    frame: RECT,
    hint: RECT,
    magic: RECT,
    eraser: RECT,
    undo: RECT,
    zoom_label: RECT,
    zoom: RECT,
    /// Riga intera dello zoom: con la gomma attiva ospita la misura della gomma.
    size: RECT,
    fit: RECT,
    sharpen_label: RECT,
    sharpen: RECT,
    warning: RECT,
    white: RECT,
    clear: RECT,
    save: RECT,
    discard: RECT,
    note: RECT,
}

fn layout(hwnd: HWND, ui: &Ui) -> Layout {
    let s = |v| scale(ui, v);
    let client = client_rect(hwnd);
    let (left, right) = (s(MARGIN), client.right - s(MARGIN));
    let top = s(HEADER) + s(MARGIN);
    let format_label = rect(left, top, left + s(60), top + s(FIELD));
    let format = rect(format_label.right, top, right, top + s(FIELD));

    // Spazio sotto il riquadro: suggerimento, gomme, zoom, messa a fuoco, avviso, sfondo, tasti, nota.
    let below = s(LABEL) + s(SMALL_BUTTON) + s(6) + s(SLIDER) + s(4) + s(SLIDER) + s(LABEL) + s(8) + s(SMALL_BUTTON) + s(12) + s(BUTTON) + s(8) + s(36);
    let room_h = (client.bottom - s(MARGIN) - below - (format.bottom + s(10))).max(s(80));
    let ratio = PHOTO.with(|g| {
        let format = g.draft.borrow().as_ref().map_or(0, |d| d.format);
        FORMATS[format].height_mm / FORMATS[format].width_mm
    });
    let room_w = right - left;
    let mut fh = ((room_w as f32 * ratio) as i32).min(s(MAX_FRAME)).min(room_h);
    let mut fw = (fh as f32 / ratio) as i32;
    if fw > room_w {
        fw = room_w;
        fh = (fw as f32 * ratio) as i32;
    }
    let fx = left + (room_w - fw) / 2;
    let frame = rect(fx, format.bottom + s(10), fx + fw, format.bottom + s(10) + fh);

    let hint = rect(left, frame.bottom, right, frame.bottom + s(LABEL));
    // Gomme: stesse proporzioni della riga nelle Firme.
    let (a, b) = (left + (right - left) * 2 / 5, left + (right - left) * 7 / 10);
    let tools = hint.bottom + s(SMALL_BUTTON);
    let magic = rect(left, hint.bottom, a - s(3), tools);
    let eraser = rect(a + s(3), hint.bottom, b - s(3), tools);
    let undo = rect(b + s(3), hint.bottom, right, tools);
    let label_w = s(96);
    let zoom_label = rect(left, tools + s(6), left + label_w, tools + s(6) + s(SLIDER));
    let fit = rect(right - s(70), zoom_label.top + (s(SLIDER) - s(SMALL_BUTTON)) / 2, right, zoom_label.top + (s(SLIDER) + s(SMALL_BUTTON)) / 2);
    let zoom = rect(zoom_label.right, zoom_label.top, fit.left - s(8), zoom_label.bottom);
    let size = rect(left, zoom_label.top, right, zoom_label.bottom);
    let sharpen_label = rect(left, zoom.bottom + s(4), left + label_w, zoom.bottom + s(4) + s(SLIDER));
    let sharpen = rect(sharpen_label.right, sharpen_label.top, right, sharpen_label.bottom);
    let warning = rect(left, sharpen.bottom, right, sharpen.bottom + s(LABEL));
    let middle = (left + right) / 2;
    let choices = warning.bottom + s(8);
    let white = rect(left, choices, middle - s(3), choices + s(SMALL_BUTTON));
    let clear = rect(middle + s(3), choices, right, choices + s(SMALL_BUTTON));
    let buttons = white.bottom + s(12);
    let save = rect(left, buttons, middle - s(4), buttons + s(BUTTON));
    let discard = rect(middle + s(4), buttons, right, buttons + s(BUTTON));
    let note = rect(left, save.bottom + s(8), right, client.bottom - s(MARGIN));
    Layout { format_label, format, frame, hint, magic, eraser, undo, zoom_label, zoom, size, fit, sharpen_label, sharpen, warning, white, clear, save, discard, note }
}

/// Posizione dello slider zoom (0 = foto lontana, 1 = vicina) per una larghezza del ritaglio.
fn zoom_fraction(draft: &Draft) -> f32 {
    let ratio = (draft.fit.w / draft.crop.w.max(1e-3)).max(1e-3);
    (0.5 + ratio.log(ZOOM_SPAN) / 2.0).clamp(0.0, 1.0)
}

fn width_for(draft: &Draft, t: f32) -> f32 {
    draft.fit.w / ZOOM_SPAN.powf(t * 2.0 - 1.0)
}

pub(super) fn hit_test(hwnd: HWND, ui: &Ui, x: i32, y: i32) -> PhotoHit {
    if !has_photo() {
        return PhotoHit::Nothing;
    }
    let l = layout(hwnd, ui);
    let areas = [
        (l.format, PhotoHit::Format),
        (l.frame, PhotoHit::Frame),
        (l.magic, PhotoHit::Magic),
        (l.eraser, PhotoHit::Eraser),
        (l.undo, PhotoHit::Undo),
        (l.zoom, PhotoHit::Zoom),
        (l.fit, PhotoHit::Fit),
        (l.sharpen, PhotoHit::Sharpen),
        (l.white, PhotoHit::Background(Background::White)),
        (l.clear, PhotoHit::Background(Background::Transparent)),
        (l.save, PhotoHit::Save),
        (l.discard, PhotoHit::Discard),
    ];
    if with_draft(|d| d.tool.is_some()) == Some(true) && l.size.contains(x, y) {
        return PhotoHit::Size;
    }
    let hit = areas.iter().find(|(r, _)| r.contains(x, y)).map_or(PhotoHit::Nothing, |(_, hit)| *hit);
    if hit == PhotoHit::Undo && with_draft(|d| d.photo.stroke_count()) == Some(0) {
        return PhotoHit::Nothing;
    }
    hit
}

// ---------- disegno ----------

pub(super) fn paint(dc: HDC, hwnd: HWND, ui: &Ui, client: RECT, hover: PhotoHit, drop: bool) {
    let s = |v| scale(ui, v);
    let colors = ui.palette.get();
    let fonts = ui.fonts.get();
    let l = layout(hwnd, ui);
    let area = rect(s(MARGIN), s(HEADER) + s(MARGIN), client.right - s(MARGIN), client.bottom - s(MARGIN));
    if drop {
        fill_round_rect(dc, area, s(8), colors.tint);
        with_pen(dc, PS_DASH, 1, colors.primary, || outline(dc, area.left, area.top, area.right - 1, area.bottom - 1));
        let text = "Rilascia la foto: lo sfondo si stacca da solo.";
        let inner = rect(area.left + s(16), area.top, area.right - s(16), area.bottom);
        let height = text_height(dc, text, inner.right - inner.left, fonts.body_bold);
        let top = inner.top + ((inner.bottom - inner.top) - height) / 2;
        draw_text(dc, text, rect(inner.left, top, inner.right, top + height), fonts.body_bold, colors.fg, DT_WORDBREAK | DT_NOPREFIX | DT_CENTER);
        return;
    }
    if !has_photo() {
        let (text, error) = PHOTO.with(|g| g.note.borrow().clone());
        let mut y = area.top;
        if !text.is_empty() {
            let height = text_height(dc, &text, area.right - area.left, fonts.small);
            draw_text(dc, &text, rect(area.left, y, area.right, y + height), fonts.small, if error { colors.err } else { colors.ok }, DT_WORDBREAK | DT_NOPREFIX);
            y += height + s(12);
        }
        draw_text(dc, EMPTY, rect(area.left, y, area.right, area.bottom), fonts.small, colors.muted, DT_WORDBREAK | DT_NOPREFIX);
        return;
    }
    let state = PHOTO.with(|g| {
        let draft = g.draft.borrow();
        let draft = draft.as_ref()?;
        blit(dc, draft, l.frame, s(CHECKER));
        Some((draft.format, zoom_fraction(draft), draft.sharpen, draft.background, draft.photo.low_resolution(draft.crop, &FORMATS[draft.format]), draft.tool, draft.photo.stroke_count()))
    });
    let Some((format, zoom, sharpen, background, low, tool, strokes)) = state else { return };
    let dragging = PHOTO.with(|g| g.drag.get());

    draw_text(dc, "Formato", l.format_label, fonts.small, colors.muted, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);
    fill_round_rect(dc, l.format, s(6), if hover == PhotoHit::Format { colors.track } else { colors.card });
    with_pen(dc, PS_SOLID, 1, colors.border, || outline(dc, l.format.left, l.format.top, l.format.right - 1, l.format.bottom - 1));
    let text_box = rect(l.format.left + s(8), l.format.top, l.format.right - s(22), l.format.bottom);
    draw_text(dc, FORMATS[format].name, text_box, fonts.small, colors.fg, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS);
    let arrow = rect(l.format.right - s(22), l.format.top, l.format.right - s(6), l.format.bottom);
    draw_text(dc, "▾", arrow, fonts.small, colors.fg, DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX);

    if tool.is_none() {
        paint_guides(dc, ui, l.frame, format);
    }
    PHOTO.with(|g| {
        if let Some(draft) = g.draft.borrow().as_ref().filter(|d| d.tool.is_some()) {
            let pointer = g.pointer.get().filter(|_| hover == PhotoHit::Frame || !draft.live.is_empty());
            paint_brush(dc, ui, l.frame, &draft.live, pointer);
        }
    });
    let hovered = hover == PhotoHit::Frame || matches!(dragging, Some(Drag::Pan { .. }));
    with_pen(dc, PS_SOLID, if hovered { s(2) } else { 1 }, if hovered { colors.primary } else { colors.border }, || {
        outline(dc, l.frame.left, l.frame.top, l.frame.right - 1, l.frame.bottom - 1)
    });
    draw_text(dc, if tool.is_some() { ERASE_HINT } else { HINT }, l.hint, fonts.small, colors.muted, DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX);
    paint_button(dc, ui, l.magic, "Gomma magica", true, tool == Some(true), hover == PhotoHit::Magic);
    paint_button(dc, ui, l.eraser, "Gomma", true, tool == Some(false), hover == PhotoHit::Eraser);
    let undo = if strokes > 0 { format!("Annulla ({strokes})") } else { "Annulla".to_string() };
    paint_button(dc, ui, l.undo, &undo, strokes > 0, false, hover == PhotoHit::Undo);

    if tool.is_some() {
        paint_eraser_size(dc, ui, l.size, hover == PhotoHit::Size || matches!(dragging, Some(Drag::Size)));
    } else {
        draw_text(dc, "Zoom", l.zoom_label, fonts.small, colors.muted, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);
        paint_slider(dc, ui, l.zoom, zoom, hover == PhotoHit::Zoom || matches!(dragging, Some(Drag::Zoom)));
        paint_button(dc, ui, l.fit, "Adatta", true, false, hover == PhotoHit::Fit);
    }
    draw_text(dc, "Messa a fuoco", l.sharpen_label, fonts.small, colors.muted, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);
    paint_slider(dc, ui, l.sharpen, sharpen / SHARPEN_MAX, hover == PhotoHit::Sharpen || matches!(dragging, Some(Drag::Sharpen)));
    if low {
        draw_text(dc, "Foto piccola: verrà sgranata", l.warning, fonts.small, colors.err, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS);
    }
    let white = background == Background::White;
    paint_button(dc, ui, l.white, "Sfondo bianco", true, white, hover == PhotoHit::Background(Background::White));
    paint_button(dc, ui, l.clear, "Trasparente", true, !white, hover == PhotoHit::Background(Background::Transparent));
    paint_button(dc, ui, l.save, "Salva foto", true, true, hover == PhotoHit::Save);
    paint_button(dc, ui, l.discard, "Scarta", true, false, hover == PhotoHit::Discard);

    let (text, error) = PHOTO.with(|g| g.note.borrow().clone());
    let (text, color) = if text.is_empty() {
        let size = FORMATS[format].pixels(DPI);
        (format!("Salva: {} x {} pixel a {DPI} DPI, nella cartella di output o accanto all'originale, e copia negli appunti.", size.0, size.1), colors.muted)
    } else {
        (text, if error { colors.err } else { colors.ok })
    };
    draw_text(dc, &text, l.note, fonts.small, color, DT_WORDBREAK | DT_NOPREFIX);
}

/// Anteprima del ritaglio, ricomposta solo se cambia qualcosa.
fn blit(dc: HDC, draft: &Draft, frame: RECT, cell: i32) {
    let (w, h) = (frame.right - frame.left, frame.bottom - frame.top);
    if w <= 0 || h <= 0 {
        return;
    }
    let mut cache = draft.cache.borrow_mut();
    let shown = draft.shown();
    let fresh = matches!(&*cache, Some((c, cw, ch, b, _)) if *c == shown && *cw == w && *ch == h && *b == draft.background);
    if !fresh {
        let bgra = draft.photo.preview(shown, w as u32, h as u32, draft.background, cell as u32);
        *cache = Some((shown, w, h, draft.background, bgra));
    }
    if let Some((.., bgra)) = cache.as_ref() {
        let info = bgra_info(w, h);
        unsafe { SetDIBitsToDevice(dc, frame.left, frame.top, w as u32, h as u32, 0, 0, 0, h as u32, bgra.as_ptr().cast(), &info, DIB_RGB_COLORS) };
    }
}

/// Ovale del volto (sommita'-mento), linea degli occhi, tacche dell'altezza minima e massima del mento.
fn paint_guides(dc: HDC, ui: &Ui, frame: RECT, format: usize) {
    let g = FORMATS[format].guides();
    let colors = ui.palette.get();
    let (w, h) = ((frame.right - frame.left) as f32, (frame.bottom - frame.top) as f32);
    let y = |f: f32| frame.top + (f * h) as i32;
    let cx = frame.left + (w / 2.0) as i32;
    let half = (g.oval_w * w / 2.0) as i32;
    let tick = scale(ui, 10);
    unsafe {
        let saved = SaveDC(dc);
        IntersectClipRect(dc, frame.left, frame.top, frame.right, frame.bottom);
        SetBkMode(dc, TRANSPARENT as i32);
        SelectObject(dc, GetStockObject(NULL_BRUSH));
        with_pen(dc, PS_DOT, 1, colors.primary, || {
            Ellipse(dc, cx - half, y(g.crown), cx + half, y(g.chin));
            line(dc, frame.left, y(g.eyes), frame.right, y(g.eyes));
        });
        with_pen(dc, PS_SOLID, scale(ui, 2), colors.primary, || {
            for f in [g.crown + g.head_min, g.crown + g.head_max] {
                line(dc, frame.left, y(f), frame.left + tick, y(f));
                line(dc, frame.right - tick, y(f), frame.right, y(f));
            }
        });
        RestoreDC(dc, saved);
    }
}

// ---------- input ----------

pub(super) fn mouse_down(hwnd: HWND, ui: &Ui, x: i32, y: i32) -> bool {
    let l = layout(hwnd, ui);
    let drag = match hit_test(hwnd, ui, x, y) {
        PhotoHit::Frame if with_draft(|d| d.tool.is_some()) == Some(true) => {
            with_draft(|d| d.live = vec![POINT { x, y }]);
            PHOTO.with(|g| g.pointer.set(Some(POINT { x, y })));
            unsafe { SetCapture(hwnd) };
            return true;
        }
        PhotoHit::Frame => with_draft(|d| Drag::Pan { start: POINT { x, y }, crop: d.crop, moved: false }),
        PhotoHit::Size => {
            set_eraser_size_at(ui, l.size, x);
            Some(Drag::Size)
        }
        PhotoHit::Zoom => {
            set_zoom(slider_fraction(ui, l.zoom, x));
            Some(Drag::Zoom)
        }
        PhotoHit::Sharpen => {
            with_draft(|d| d.sharpen = slider_fraction(ui, l.sharpen, x) * SHARPEN_MAX);
            Some(Drag::Sharpen)
        }
        _ => None,
    };
    let Some(drag) = drag else { return false };
    PHOTO.with(|g| g.drag.set(Some(drag)));
    unsafe { SetCapture(hwnd) };
    true
}

fn set_zoom(t: f32) {
    with_draft(|d| d.crop = d.crop.zoomed(width_for(d, t), 0.5, 0.5));
}

/// Movimento col tasto premuto. Vero se gestito.
pub(super) fn mouse_move(hwnd: HWND, ui: &Ui, x: i32, y: i32) -> bool {
    // Gomma: la passata in corso segue il mouse; senza tasto premuto solo il cerchio (e l'hover).
    let (erasing, stroking) = with_draft(|d| (d.tool.is_some(), !d.live.is_empty())).unwrap_or((false, false));
    if erasing {
        PHOTO.with(|g| g.pointer.set(Some(POINT { x, y })));
        if stroking {
            with_draft(|d| d.live.push(POINT { x, y }));
        }
        unsafe { InvalidateRect(hwnd, null(), 0) };
        if stroking {
            return true;
        }
    }
    let Some(drag) = PHOTO.with(|g| g.drag.get()) else { return false };
    let l = layout(hwnd, ui);
    match drag {
        Drag::Pan { start, crop, moved } => {
            let (dx, dy) = (x - start.x, y - start.y);
            if !moved && dx.abs() <= DRAG_THRESHOLD && dy.abs() <= DRAG_THRESHOLD {
                return true;
            }
            let k = crop.w / (l.frame.right - l.frame.left).max(1) as f32;
            with_draft(|d| d.crop = crop.moved(-dx as f32 * k, -dy as f32 * k));
            PHOTO.with(|g| g.drag.set(Some(Drag::Pan { start, crop, moved: true })));
        }
        Drag::Zoom => set_zoom(slider_fraction(ui, l.zoom, x)),
        Drag::Size => {
            set_eraser_size_at(ui, l.size, x);
        }
        Drag::Sharpen => {
            with_draft(|d| d.sharpen = slider_fraction(ui, l.sharpen, x) * SHARPEN_MAX);
        }
    }
    unsafe { InvalidateRect(hwnd, null(), 0) };
    true
}

/// Fine del trascinamento: la messa a fuoco e le passate di gomma si ricalcolano solo adesso.
/// Vero se gestito.
pub(super) fn mouse_up(hwnd: HWND, ui: &Ui) -> bool {
    if finish_stroke(hwnd, ui) {
        unsafe { ReleaseCapture() };
        return true;
    }
    let Some(drag) = PHOTO.with(|g| g.drag.replace(None)) else { return false };
    unsafe { ReleaseCapture() };
    if let Drag::Sharpen = drag {
        unsafe { SetCursor(LoadCursorW(null_mut(), IDC_WAIT)) };
        with_draft(|d| {
            d.photo.set_sharpen(d.sharpen);
            *d.cache.borrow_mut() = None;
            PHOTO.with(|g| g.sharpen.set(d.sharpen));
        });
    }
    true
}

/// Fine della passata: dai punti del pannello ai pixel della foto, poi sagoma ricalcolata.
/// Vero se c'era una passata.
fn finish_stroke(hwnd: HWND, ui: &Ui) -> bool {
    let frame = layout(hwnd, ui).frame;
    let radius = eraser_radius(ui) as f32;
    let Some(live) = with_draft(|d| std::mem::take(&mut d.live)).filter(|live| !live.is_empty()) else { return false };
    unsafe { SetCursor(LoadCursorW(null_mut(), IDC_WAIT)) };
    with_draft(|d| {
        let shown = d.shown();
        let k = shown.w / (frame.right - frame.left).max(1) as f32;
        let points = live.iter().map(|p| (shown.x + (p.x - frame.left) as f32 * k, shown.y + (p.y - frame.top) as f32 * k)).collect();
        d.photo.add_stroke(Stroke { points, radius: radius * k, magic: d.tool == Some(true) });
        *d.cache.borrow_mut() = None;
    });
    true
}

/// Rotella sul riquadro: zoom verso il mouse. Vero se gestita.
pub(super) fn zoom(hwnd: HWND, ui: &Ui, wheel: i32, point: POINT) -> bool {
    let frame = layout(hwnd, ui).frame;
    if !has_photo() || !frame.contains(point.x, point.y) {
        return false;
    }
    let fx = (point.x - frame.left) as f32 / (frame.right - frame.left).max(1) as f32;
    let fy = (point.y - frame.top) as f32 / (frame.bottom - frame.top).max(1) as f32;
    let factor = WHEEL_STEP.powf(-wheel as f32 / 120.0);
    with_draft(|d| {
        if d.tool.is_some() {
            let shown = d.shown();
            let w = (shown.w * factor).clamp(d.crop.w / ERASE_ZOOM_MAX, d.crop.w);
            d.view = if w >= d.crop.w { None } else { Some(shown.zoomed(w, fx, fy)) };
            return;
        }
        let (lo, hi) = (width_for(d, 1.0), width_for(d, 0.0));
        d.crop = d.crop.zoomed((d.crop.w * factor).clamp(lo, hi), fx, fy);
    });
    true
}

pub(super) fn on_click(hwnd: HWND, ui: &Ui, hit: PhotoHit) {
    match hit {
        PhotoHit::Format => pick_format(hwnd, ui),
        PhotoHit::Fit => {
            with_draft(|d| d.crop = d.fit);
        }
        PhotoHit::Background(background) => {
            with_draft(|d| d.background = background);
            PHOTO.with(|g| g.background.set(background));
        }
        PhotoHit::Magic | PhotoHit::Eraser => {
            // Stesso tasto due volte = spenta.
            let wanted = hit == PhotoHit::Magic;
            with_draft(|d| {
                d.tool = if d.tool == Some(wanted) { None } else { Some(wanted) };
                if d.tool.is_none() {
                    d.view = None;
                }
            });
        }
        PhotoHit::Undo => {
            unsafe { SetCursor(LoadCursorW(null_mut(), IDC_WAIT)) };
            with_draft(|d| {
                if d.photo.undo_stroke() {
                    *d.cache.borrow_mut() = None;
                }
            });
        }
        PhotoHit::Save => save(hwnd),
        PhotoHit::Discard => {
            PHOTO.with(|g| *g.draft.borrow_mut() = None);
            note("", false);
        }
        PhotoHit::Frame | PhotoHit::Zoom | PhotoHit::Size | PhotoHit::Sharpen | PhotoHit::Nothing => {}
    }
}

fn pick_format(hwnd: HWND, ui: &Ui) {
    let l = layout(hwnd, ui);
    let mut at = POINT { x: l.format.left, y: l.format.bottom };
    unsafe { ClientToScreen(hwnd, &mut at) };
    let items: Vec<(usize, String)> = FORMATS.iter().enumerate().map(|(i, f)| (i + 1, f.name.to_string())).collect();
    let choice = popup(hwnd, &items, at);
    if choice == 0 {
        return;
    }
    let format = choice - 1;
    PHOTO.with(|g| g.format.set(format));
    with_draft(|d| {
        d.format = format;
        d.fit = d.photo.fit(&FORMATS[format]);
        d.crop = d.fit;
        d.view = None;
    });
}

/// Azzera trascinamento/passata se il sistema toglie la cattura del mouse (WM_CAPTURECHANGED).
pub(super) fn cancel_capture() {
    PHOTO.with(|g| {
        g.drag.set(None);
        if let Some(d) = g.draft.borrow_mut().as_mut() {
            d.live.clear();
        }
    });
}

fn save(hwnd: HWND) {
    unsafe { SetCursor(LoadCursorW(null_mut(), IDC_WAIT)) };
    let Some((image, background, name, path)) = with_draft(|d| {
        let (w, h) = FORMATS[d.format].pixels(DPI);
        (d.photo.render(d.crop, w, h, d.background), d.background, d.name.clone(), d.path.clone())
    }) else {
        return;
    };
    let saved = portrait::encode(&image, background, DPI).map_err(error_text).and_then(|(data, ext)| match beside(SLOT_PHOTO, path.as_deref(), &name) {
        Some(crate::tools::Destination::NextTo(original)) => portrait::save_beside(&original, &data, ext).map_err(error_text),
        _ => {
            let stem = crate::tools::file_stem(&name);
            let Some(target) = dialogs::pick_file(hwnd, &format!("{stem}{}.{ext}", portrait::SUFFIX)) else { return Err(String::new()) };
            fs::write(&target, &data).map(|_| target).map_err(|e| e.to_string())
        }
    });
    let saved_path = match saved {
        Ok(path) => path,
        Err(error) if error.is_empty() => return,
        Err(error) => return note(&format!("Foto non salvata: {error}"), true),
    };
    let copied = signature::to_png(&image, DPI).is_ok_and(|png| clipboard::copy_signature(hwnd, &png, &image));
    let file = saved_path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let text = if copied { format!("Foto salvata ({file}) e copiata negli appunti.") } else { format!("Foto salvata ({file}). Appunti occupati: non copiata.") };
    note(&text, false);
}

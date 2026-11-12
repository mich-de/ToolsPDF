//! Pannello Firme: cattura di un'area dello schermo, anteprima con gomma magica e gomma normale
//! (per il testo stampato del modulo; con la gomma l'anteprima si ingrandisce e la rotella zooma) e slider "piu' chiara / piu' scura", spunte "Rendi la firma nera", "Migliora per la stampa"
//! e "Togli scritte, righe e puntini" (scritte con l'OCR ocrs), nota facoltativa, elenco delle
//! firme salvate. Clic su una firma = copia negli appunti; trascinamento = nel documento;
//! tasto destro = menu. Il pannello resta in panel.rs, che inoltra Mode::Signatures.

use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::ptr::{null, null_mut};

use image::{RgbImage, RgbaImage};
use windows_sys::Win32::Foundation::{HINSTANCE, HWND, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, VK_RETURN};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use super::paint::{bgra_info, client_rect, draw_text, fill_rect, fill_round_rect, outline, paint_button, text_height, with_pen, MARGIN};
use super::panel::{paint_remove, HEADER};
use super::tools_panel::{get_text, paint_check, paint_field, set_text};
use super::win::{self, rect, wide, Cell2, RectExt};
use super::{capture, clipboard, drag_source, error_text, main_hwnd, scale, Ui, APP_TITLE};
use crate::tools::signature::{self, Options, Prepared, Stroke, View, DARKNESS_RANGE, NOTE_MAX};
use crate::tools::Source;

/// Zona di rilascio del pannello Firme: l'immagine passa dalla pulizia e dall'anteprima.
pub(crate) const DROP_IMAGE: usize = 3;

const BUTTON: i32 = 36;
const NOTE: i32 = 38;
const ROW: i32 = 70;
const CAPTION: i32 = 20;
const ROW_GAP: i32 = 8;
const SMALL_BUTTON: i32 = 26;
const LABEL: i32 = 22;
const PREVIEW: i32 = 140;
const SLIDER: i32 = 28;
const CHECK: i32 = 26;
const FIELD: i32 = 26;
const CHECKER: i32 = 6;
const DRAG_THRESHOLD: i32 = 5;
/// Raggio della gomma sullo schermo.
const ERASER: i32 = 9;
/// Livelli di zoom della rotella con la gomma attiva.
const ZOOMS: [f32; 6] = [1.0, 1.5, 2.0, 3.0, 4.0, 6.0];
/// Spiegazione sotto l'anteprima grande, con la gomma attiva.
const ERASE_HINT: &str = "Gomma magica: passa sulle scritte stampate; toglie solo i pezzi che stanno tutti dentro la passata, i tratti della firma restano interi. Gomma: cancella tutto quello che tocca, per le lettere attaccate alla firma. Rotella: zoom.";
const EM_LIMITTEXT: u32 = 0x00C5;
const HINT: &str = "Clic: copia negli appunti · trascinala nel documento · tasto destro: altro";
const EMPTY: &str = "Nessuna firma salvata.\n\nPremi Cattura firma e trascina un riquadro sulla firma, oppure trascina qui una foto o una scansione della firma.";
/// Spunte dell'anteprima, nell'ordine di SigHit::Toggle.
const TOGGLES: [&str; 3] = ["Rendi la firma nera", "Migliora per la stampa", "Togli scritte, righe e puntini del modulo"];

const MENU_COPY: usize = 1;
const MENU_DELETE: usize = 2;
const MENU_FOLDER: usize = 3;

/// Elemento del pannello Firme sotto il mouse.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum SigHit {
    Nothing,
    Capture,
    Item(usize),
    Delete(usize),
    Slider,
    Toggle(usize),
    /// Tasti Gomma magica e Gomma (attivano o spengono), Annulla l'ultima passata, anteprima con
    /// la gomma attiva, Fine (spegne la gomma).
    Magic,
    Eraser,
    Undo,
    Preview,
    Done,
    Save,
    Discard,
}

/// Anteprima gia' composta per un riquadro di quella misura.
type Cache = RefCell<Option<(i32, i32, Vec<u8>)>>;

struct Saved {
    path: PathBuf,
    label: String,
    image: RgbaImage,
    cache: Cache,
}

/// Firma appena catturata o trascinata, non ancora salvata.
struct Draft {
    source: RgbImage,
    options: Options,
    /// Passate di gomma, in pixel della sorgente.
    strokes: Vec<Stroke>,
    /// Scritte stampate del modulo trovate con l'OCR, come passate di gomma magica: si cercano
    /// una volta sola, la prima volta che serve la spunta "Togli scritte...".
    printed: Option<Vec<Stroke>>,
    /// Firma pulita, DPI da scrivere nel PNG e posizione nella sorgente.
    cleaned: Result<Prepared, String>,
    cache: Cache,
    /// Gomma attiva (Some(true) = magica) e passata in corso (punti del pannello).
    tool: Option<bool>,
    live: Vec<POINT>,
    /// Con la gomma: misura del riquadro, inquadratura, livello di ZOOMS e anteprima composta.
    view: Option<(i32, i32, View)>,
    zoom: usize,
    view_cache: RefCell<Option<(View, Vec<u8>)>>,
}

impl Draft {
    fn new(source: RgbImage, options: Options) -> Self {
        let mut draft = Draft {
            source,
            options,
            strokes: Vec::new(),
            printed: None,
            cleaned: Err(String::new()),
            cache: RefCell::new(None),
            tool: None,
            live: Vec::new(),
            view: None,
            zoom: 0,
            view_cache: RefCell::new(None),
        };
        draft.refresh();
        draft
    }

    /// Ripulisce la sorgente con scelte e gomma attuali.
    fn refresh(&mut self) {
        if self.options.remove_print && self.printed.is_none() {
            unsafe { SetCursor(LoadCursorW(null_mut(), IDC_WAIT)) };
            self.printed = Some(signature::printed_strokes(&self.source));
        }
        let printed = if self.options.remove_print { self.printed.as_deref().unwrap_or(&[]) } else { &[] };
        let all: Vec<Stroke> = printed.iter().chain(&self.strokes).cloned().collect();
        self.cleaned = signature::prepare(&self.source, self.options, &all).map_err(error_text);
        *self.cache.borrow_mut() = None;
        *self.view_cache.borrow_mut() = None;
    }

    fn erasing(&self) -> bool {
        self.tool.is_some()
    }

    /// Accende o spegne una gomma; spegnendo si torna alla firma intera.
    fn set_tool(&mut self, tool: Option<bool>) {
        self.tool = tool;
        if tool.is_none() {
            self.view = None;
            self.zoom = 0;
            *self.view_cache.borrow_mut() = None;
        }
    }

    /// Inquadratura per il riquadro area: la tiene ferma tra una passata e l'altra (anche se il
    /// ritaglio della firma cambia); la rifa' se cambia la misura del riquadro.
    fn view_for(&mut self, area: RECT) -> Option<View> {
        let (w, h) = (area.right - area.left, area.bottom - area.top);
        if let Some((vw, vh, view)) = self.view {
            if (vw, vh) == (w, h) {
                return Some(view);
            }
        }
        let prepared = self.cleaned.as_ref().ok()?;
        let view = View::fit(prepared, w.max(1) as u32, h.max(1) as u32);
        self.view = Some((w, h, view));
        self.zoom = 0;
        Some(view)
    }

    /// Da un punto dell'anteprima grande al punto della sorgente, piu' i pixel di sorgente per
    /// pixel di schermo.
    fn to_source(&self, area: RECT, point: POINT) -> Option<(f32, f32, f32)> {
        let (_, _, view) = self.view?;
        let (sx, sy) = view.to_source((point.x - area.left) as f32, (point.y - area.top) as f32);
        Some((sx, sy, view.k))
    }
}

struct SigUi {
    items: RefCell<Vec<Saved>>,
    draft: RefCell<Option<Draft>>,
    /// Spunte dell'ultima anteprima: valgono per la successiva.
    defaults: Cell2<Options>,
    scroll: Cell2<i32>,
    /// Messaggio sotto il tasto Cattura e se e' un errore.
    note: RefCell<(String, bool)>,
    /// Firma premuta e punto del clic: diventa trascinamento oltre una soglia.
    press: Cell2<Option<(usize, POINT)>>,
    sliding: Cell2<bool>,
    dragging: Cell2<bool>,
    /// Ultima posizione del mouse sull'anteprima: cerchio della gomma.
    pointer: Cell2<POINT>,
    /// Campo della nota, sua posizione e font.
    edit: Cell2<HWND>,
    edit_rect: Cell2<Option<(i32, i32, i32, i32)>>,
    edit_font: Cell2<HFONT>,
}

thread_local! {
    static SIGS: SigUi = SigUi {
        items: RefCell::new(Vec::new()),
        draft: RefCell::new(None),
        defaults: Cell2::new(Options { darkness: 0, remove_print: false, for_print: true, solid: true }),
        scroll: Cell2::new(0),
        note: RefCell::new((String::new(), false)),
        press: Cell2::new(None),
        sliding: Cell2::new(false),
        dragging: Cell2::new(false),
        pointer: Cell2::new(POINT { x: 0, y: 0 }),
        edit: Cell2::new(null_mut()),
        edit_rect: Cell2::new(None),
        edit_font: Cell2::new(null_mut()),
    };
}

fn note(text: &str, error: bool) {
    SIGS.with(|g| *g.note.borrow_mut() = (text.to_string(), error));
}

/// Vero mentre una firma viene trascinata fuori: la barra e il pannello non la accettano.
pub(crate) fn is_dragging() -> bool {
    SIGS.with(|g| g.dragging.get())
}

/// Rilegge la cartella delle firme. Pochi PNG piccoli: si fa nel thread UI.
pub(super) fn reload() {
    let items: Vec<Saved> = signature::list()
        .into_iter()
        .filter_map(|path| {
            let image = signature::load(&path).ok()?;
            Some(Saved { label: signature::label(&path), path, image, cache: RefCell::new(None) })
        })
        .collect();
    SIGS.with(|g| *g.items.borrow_mut() = items);
}

fn has_draft() -> bool {
    SIGS.with(|g| g.draft.borrow().is_some())
}

fn start_draft(image: RgbImage) {
    let source = signature::limit(image);
    let options = Options { darkness: 0, ..SIGS.with(|g| g.defaults.get()) };
    let draft = Draft::new(source, options);
    SIGS.with(|g| *g.draft.borrow_mut() = Some(draft));
    set_text(SIGS.with(|g| g.edit.get()), "");
    note("", false);
}

/// Immagine trascinata sulla zona Firme o sul pannello: pulizia e anteprima come una cattura.
pub(crate) fn load_dropped(source: Source) {
    let Source { path, bytes, .. } = source;
    let data = match (bytes, &path) {
        (Some(bytes), _) => Ok(bytes),
        (None, Some(path)) => fs::read(path).map_err(|e| e.to_string()),
        (None, None) => Err("file vuoto".into()),
    };
    match data.and_then(|bytes| signature::decode(&bytes).map_err(error_text)) {
        Ok(image) => start_draft(image),
        Err(error) => note(&format!("Immagine non utilizzabile: {error}"), true),
    }
}

// ---------- campo della nota ----------

/// Crea il campo della nota, nascosto.
pub(super) fn create_children(panel: HWND, instance: HINSTANCE) {
    let class = wide("EDIT");
    let style = WS_CHILD | ES_AUTOHSCROLL as u32;
    unsafe {
        let edit = CreateWindowExW(0, class.as_ptr(), null(), style, 0, 0, 0, 0, panel, 120 as HMENU, instance, null());
        SendMessageW(edit, EM_LIMITTEXT, NOTE_MAX, 0);
        SIGS.with(|g| g.edit.set(edit));
    }
}

fn is_edit(hwnd: HWND) -> bool {
    !hwnd.is_null() && hwnd == SIGS.with(|g| g.edit.get())
}

/// Il pannello si attiva solo cliccando il campo: per scrivere serve.
pub(super) fn edit_under_cursor() -> bool {
    let mut point = POINT { x: 0, y: 0 };
    unsafe {
        GetCursorPos(&mut point);
        is_edit(WindowFromPoint(point))
    }
}

/// Invio nella nota salva la firma (e niente bip del campo a riga singola).
pub(crate) fn handle_key(message: &MSG) -> bool {
    if message.message != WM_KEYDOWN || message.wParam != VK_RETURN as usize || !is_edit(message.hwnd) {
        return false;
    }
    let panel = unsafe { GetParent(message.hwnd) };
    save(panel);
    super::panel::invalidate();
    true
}

/// Mostra il campo della nota sotto l'anteprima. Mai durante il disegno.
pub(super) fn sync_children(panel: HWND, ui: &Ui, visible: bool) {
    let edit = SIGS.with(|g| g.edit.get());
    if edit.is_null() {
        return;
    }
    let font = ui.fonts.get().small;
    if SIGS.with(|g| g.edit_font.replace(font)) != font {
        unsafe { SendMessageW(edit, WM_SETFONT, font as usize, 0) };
    }
    let editing = SIGS.with(|g| g.draft.borrow().as_ref().is_some_and(|d| !d.erasing()));
    let wanted = (visible && editing).then(|| layout(panel, ui).note_field);
    let key = wanted.map(|r| (r.left, r.top, r.right, r.bottom));
    if SIGS.with(|g| g.edit_rect.replace(key)) == key {
        return;
    }
    let s = |v| scale(ui, v);
    unsafe {
        match wanted {
            Some(field) => {
                let height = s(18);
                let top = field.top + (field.bottom - field.top - height) / 2;
                SetWindowPos(edit, null_mut(), field.left + s(8), top, field.right - field.left - s(16), height, SWP_NOZORDER | SWP_NOACTIVATE);
                ShowWindow(edit, SW_SHOWNA);
            }
            None => {
                ShowWindow(edit, SW_HIDE);
            }
        }
    }
}

// ---------- geometria ----------

struct Layout {
    capture: RECT,
    note: RECT,
    list: RECT,
    label: RECT,
    preview: RECT,
    magic: RECT,
    eraser: RECT,
    undo: RECT,
    done: RECT,
    erase_note: RECT,
    ends: RECT,
    slider: RECT,
    toggles: [RECT; 3],
    note_label: RECT,
    note_field: RECT,
    save: RECT,
    discard: RECT,
    draft_note: RECT,
}

fn layout(hwnd: HWND, ui: &Ui) -> Layout {
    let s = |v| scale(ui, v);
    let client = client_rect(hwnd);
    let (left, right) = (s(MARGIN), client.right - s(MARGIN));
    let top = s(HEADER) + s(MARGIN);

    let capture = rect(left, top, right, top + s(BUTTON));
    let note = rect(left, capture.bottom + s(4), right, capture.bottom + s(4) + s(NOTE));
    let list = rect(0, note.bottom + s(4), client.right, client.bottom - s(MARGIN));

    let label = rect(left, top, right, top + s(LABEL));
    let preview = rect(left, label.bottom, right, label.bottom + s(PREVIEW));
    let middle = client.right / 2;
    let row = |top: i32| {
        let (a, b) = (left + (right - left) * 2 / 5, left + (right - left) * 7 / 10);
        let bottom = top + s(SMALL_BUTTON) + s(4);
        (rect(left, top, a - s(3), bottom), rect(a + s(3), top, b - s(3), bottom), rect(b + s(3), top, right, bottom))
    };
    // Con la gomma l'anteprima prende il posto di slider, spunte, nota e Salva.
    let erasing = SIGS.with(|g| g.draft.borrow().as_ref().is_some_and(|d| d.erasing()));
    let done = rect(left, client.bottom - s(MARGIN) - s(BUTTON), right, client.bottom - s(MARGIN));
    let erase_note = rect(left, done.top - s(8) - s(72), right, done.top - s(8));
    let (preview, (magic, eraser, undo)) = if erasing {
        let tools = erase_note.top - s(8) - s(SMALL_BUTTON) - s(4);
        (rect(left, label.bottom, right, (tools - s(8)).max(preview.bottom)), row(tools))
    } else {
        (preview, row(preview.bottom + s(8)))
    };
    let ends = rect(left, eraser.bottom + s(8), right, eraser.bottom + s(8) + s(18));
    let slider = rect(left, ends.bottom, right, ends.bottom + s(SLIDER));
    let first = slider.bottom + s(6);
    let toggles = std::array::from_fn(|k| {
        let top = first + k as i32 * (s(CHECK) + s(2));
        rect(left, top, right, top + s(CHECK))
    });
    let field_top = toggles[2].bottom + s(8);
    let note_label = rect(left, field_top, left + s(44), field_top + s(FIELD));
    let note_field = rect(note_label.right, field_top, right, field_top + s(FIELD));
    let buttons = note_field.bottom + s(12);
    let save = rect(left, buttons, middle - s(4), buttons + s(BUTTON));
    let discard = rect(middle + s(4), buttons, right, buttons + s(BUTTON));
    let draft_note = rect(left, save.bottom + s(10), right, client.bottom - s(MARGIN));
    Layout { capture, note, list, label, preview, magic, eraser, undo, done, erase_note, ends, slider, toggles, note_label, note_field, save, discard, draft_note }
}

/// Riquadro della firma, tasto elimina e didascalia della riga indicata.
fn row_rects(ui: &Ui, l: &Layout, index: usize) -> (RECT, RECT, RECT) {
    let s = |v| scale(ui, v);
    let top = l.list.top + index as i32 * (s(ROW) + s(CAPTION) + s(ROW_GAP)) - SIGS.with(|g| g.scroll.get());
    let right = l.list.right - s(MARGIN);
    let remove = rect(right - s(SMALL_BUTTON), top + (s(ROW) - s(SMALL_BUTTON)) / 2, right, top + (s(ROW) + s(SMALL_BUTTON)) / 2);
    let image = rect(s(MARGIN), top, remove.left - s(6), top + s(ROW));
    (image, remove, rect(image.left, image.bottom, image.right, image.bottom + s(CAPTION)))
}

/// Asse dello slider: un po' di respiro ai lati per il pomello.
fn track(ui: &Ui, slider: RECT) -> (i32, i32) {
    (slider.left + scale(ui, 10), slider.right - scale(ui, 10))
}

fn darkness_at(ui: &Ui, l: &Layout, x: i32) -> i32 {
    let (a, b) = track(ui, l.slider);
    let t = (x - a).clamp(0, (b - a).max(1)) as f64 / (b - a).max(1) as f64;
    ((t * 2.0 - 1.0) * DARKNESS_RANGE as f64).round() as i32
}

pub(super) fn hit_test(hwnd: HWND, ui: &Ui, x: i32, y: i32) -> SigHit {
    let l = layout(hwnd, ui);
    if has_draft() {
        let (ok, erasing, strokes) = SIGS.with(|g| {
            let draft = g.draft.borrow();
            draft.as_ref().map_or((false, false, 0), |d| (d.cleaned.is_ok(), d.erasing(), d.strokes.len()))
        });
        if l.preview.contains(x, y) {
            return if erasing && ok { SigHit::Preview } else { SigHit::Nothing };
        }
        if l.magic.contains(x, y) {
            return if ok || erasing { SigHit::Magic } else { SigHit::Nothing };
        }
        if l.eraser.contains(x, y) {
            return if ok || erasing { SigHit::Eraser } else { SigHit::Nothing };
        }
        if l.undo.contains(x, y) {
            return if strokes > 0 { SigHit::Undo } else { SigHit::Nothing };
        }
        if erasing {
            return if l.done.contains(x, y) { SigHit::Done } else { SigHit::Nothing };
        }
        if l.slider.contains(x, y) {
            return SigHit::Slider;
        }
        if let Some(k) = l.toggles.iter().position(|r| r.contains(x, y)) {
            return SigHit::Toggle(k);
        }
        if l.save.contains(x, y) {
            return if ok { SigHit::Save } else { SigHit::Nothing };
        }
        if l.discard.contains(x, y) {
            return SigHit::Discard;
        }
        return SigHit::Nothing;
    }
    if l.capture.contains(x, y) {
        return SigHit::Capture;
    }
    if !l.list.contains(x, y) {
        return SigHit::Nothing;
    }
    let count = SIGS.with(|g| g.items.borrow().len());
    for index in 0..count {
        let (image, remove, caption) = row_rects(ui, &l, index);
        if remove.contains(x, y) {
            return SigHit::Delete(index);
        }
        if image.contains(x, y) || caption.contains(x, y) {
            return SigHit::Item(index);
        }
    }
    SigHit::Nothing
}

pub(super) fn scroll(delta: i32) {
    SIGS.with(|g| g.scroll.set(g.scroll.get() - delta));
}

/// Rotella sull'anteprima con la gomma attiva: zoom verso il punto sotto il mouse (coordinate
/// del pannello). Vero se gestita.
pub(super) fn zoom(hwnd: HWND, ui: &Ui, wheel: i32, point: POINT) -> bool {
    let area = layout(hwnd, ui).preview;
    if !area.contains(point.x, point.y) {
        return false;
    }
    SIGS.with(|g| {
        let mut draft = g.draft.borrow_mut();
        let Some(draft) = draft.as_mut().filter(|d| d.erasing()) else { return false };
        let Some(current) = draft.view_for(area) else { return true };
        let Ok(prepared) = draft.cleaned.as_ref() else { return true };
        let level = if wheel > 0 { (draft.zoom + 1).min(ZOOMS.len() - 1) } else { draft.zoom.saturating_sub(1) };
        let fit = View::fit(prepared, (area.right - area.left).max(1) as u32, (area.bottom - area.top).max(1) as u32);
        let view = if level == 0 {
            fit
        } else {
            current.zoom_at(fit.k / ZOOMS[level], (point.x - area.left) as f32, (point.y - area.top) as f32)
        };
        draft.zoom = level;
        draft.view = Some((area.right - area.left, area.bottom - area.top, view));
        true
    })
}

// ---------- disegno ----------

/// Disegna l'anteprima su scacchiera, ricomponendola solo se cambia la misura del riquadro.
fn blit_preview(dc: HDC, image: &RgbaImage, cache: &Cache, area: RECT, cell: i32) {
    let (w, h) = (area.right - area.left, area.bottom - area.top);
    if w <= 0 || h <= 0 {
        return;
    }
    let mut cached = cache.borrow_mut();
    if !matches!(&*cached, Some((cw, ch, _)) if *cw == w && *ch == h) {
        *cached = Some((w, h, signature::preview(image, w as u32, h as u32, cell as u32)));
    }
    if let Some((_, _, bgra)) = cached.as_ref() {
        let info = bgra_info(w, h);
        unsafe {
            SetDIBitsToDevice(dc, area.left, area.top, w as u32, h as u32, 0, 0, 0, h as u32, bgra.as_ptr().cast(), &info, DIB_RGB_COLORS);
        }
    }
}

/// Anteprima ingrandita della gomma, ricomposta solo se cambia l'inquadratura o la firma.
fn blit_view(dc: HDC, prepared: &Prepared, view: View, cache: &RefCell<Option<(View, Vec<u8>)>>, area: RECT, cell: i32) {
    let (w, h) = (area.right - area.left, area.bottom - area.top);
    if w <= 0 || h <= 0 {
        return;
    }
    let mut cached = cache.borrow_mut();
    if !matches!(&*cached, Some((v, bgra)) if *v == view && bgra.len() == (w * h * 4) as usize) {
        *cached = Some((view, signature::preview_view(prepared, view, w as u32, h as u32, cell as u32)));
    }
    if let Some((_, bgra)) = cached.as_ref() {
        let info = bgra_info(w, h);
        unsafe {
            SetDIBitsToDevice(dc, area.left, area.top, w as u32, h as u32, 0, 0, 0, h as u32, bgra.as_ptr().cast(), &info, DIB_RGB_COLORS);
        }
    }
}

pub(super) fn paint(dc: HDC, hwnd: HWND, ui: &Ui, client: RECT, hover: SigHit, drop: bool) {
    let s = |v| scale(ui, v);
    let colors = ui.palette.get();
    let fonts = ui.fonts.get();
    let l = layout(hwnd, ui);

    if drop {
        let area = rect(s(MARGIN), s(HEADER) + s(MARGIN), client.right - s(MARGIN), client.bottom - s(MARGIN));
        fill_round_rect(dc, area, s(8), colors.tint);
        with_pen(dc, PS_DASH, 1, colors.primary, || outline(dc, area.left, area.top, area.right - 1, area.bottom - 1));
        let text = "Rilascia l'immagine: la firma diventa nera su sfondo trasparente.";
        let inner = rect(area.left + s(16), area.top, area.right - s(16), area.bottom);
        let height = text_height(dc, text, inner.right - inner.left, fonts.body_bold);
        let top = inner.top + ((inner.bottom - inner.top) - height) / 2;
        draw_text(dc, text, rect(inner.left, top, inner.right, top + height), fonts.body_bold, colors.fg, DT_WORDBREAK | DT_NOPREFIX | DT_CENTER);
        return;
    }

    if has_draft() {
        paint_draft(dc, ui, &l, hover);
        return;
    }

    paint_button(dc, ui, l.capture, "Cattura firma", true, true, hover == SigHit::Capture);
    let (text, error) = SIGS.with(|g| g.note.borrow().clone());
    let count = SIGS.with(|g| g.items.borrow().len());
    let (text, color) = match (text.is_empty(), count) {
        (false, _) => (text, if error { colors.err } else { colors.ok }),
        (true, 0) => (String::new(), colors.muted),
        (true, _) => (HINT.to_string(), colors.muted),
    };
    draw_text(dc, &text, l.note, fonts.small, color, DT_WORDBREAK | DT_NOPREFIX | DT_END_ELLIPSIS);

    if count == 0 {
        let area = rect(s(MARGIN), l.list.top + s(8), client.right - s(MARGIN), l.list.bottom);
        draw_text(dc, EMPTY, area, fonts.small, colors.muted, DT_WORDBREAK | DT_NOPREFIX);
        return;
    }

    let step = s(ROW) + s(CAPTION) + s(ROW_GAP);
    let max_scroll = (count as i32 * step - s(ROW_GAP) - (l.list.bottom - l.list.top)).max(0);
    SIGS.with(|g| g.scroll.set(g.scroll.get().clamp(0, max_scroll)));
    unsafe {
        let saved = SaveDC(dc);
        IntersectClipRect(dc, l.list.left, l.list.top, l.list.right, l.list.bottom);
        SIGS.with(|g| {
            for (index, item) in g.items.borrow().iter().enumerate() {
                let (area, remove, caption) = row_rects(ui, &l, index);
                if caption.bottom < l.list.top || area.top > l.list.bottom {
                    continue;
                }
                blit_preview(dc, &item.image, &item.cache, area, s(CHECKER));
                let hovered = hover == SigHit::Item(index);
                let pen = if hovered { s(2) } else { 1 };
                with_pen(dc, PS_SOLID, pen, if hovered { colors.primary } else { colors.border }, || {
                    outline(dc, area.left, area.top, area.right - 1, area.bottom - 1)
                });
                draw_text(dc, &item.label, caption, fonts.small, colors.muted, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS);
                paint_remove(dc, ui, remove, hover == SigHit::Delete(index));
            }
        });
        RestoreDC(dc, saved);
    }
}

fn paint_draft(dc: HDC, ui: &Ui, l: &Layout, hover: SigHit) {
    let s = |v| scale(ui, v);
    let colors = ui.palette.get();
    let fonts = ui.fonts.get();
    draw_text(dc, "Anteprima", l.label, fonts.body_bold, colors.fg, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);

    let (ok, options, tool, strokes, zoom) = SIGS.with(|g| {
        let mut draft = g.draft.borrow_mut();
        let Some(draft) = draft.as_mut() else { return (false, Options::default(), None, 0, 0) };
        if draft.erasing() {
            draft.view_for(l.preview);
        }
        let ok = match &draft.cleaned {
            Ok(prepared) => {
                match draft.view {
                    Some((_, _, view)) if draft.erasing() => blit_view(dc, prepared, view, &draft.view_cache, l.preview, s(CHECKER)),
                    _ => blit_preview(dc, &prepared.image, &draft.cache, l.preview, s(CHECKER)),
                }
                paint_eraser(dc, ui, l, draft, hover == SigHit::Preview);
                with_pen(dc, PS_SOLID, 1, colors.border, || outline(dc, l.preview.left, l.preview.top, l.preview.right - 1, l.preview.bottom - 1));
                true
            }
            Err(error) => {
                fill_round_rect(dc, l.preview, s(6), colors.card);
                let inner = rect(l.preview.left + s(12), l.preview.top, l.preview.right - s(12), l.preview.bottom);
                let height = text_height(dc, error, inner.right - inner.left, fonts.small);
                let top = inner.top + ((inner.bottom - inner.top) - height) / 2;
                draw_text(dc, error, rect(inner.left, top, inner.right, top + height), fonts.small, colors.err, DT_WORDBREAK | DT_NOPREFIX | DT_CENTER);
                false
            }
        };
        (ok, draft.options, draft.tool, draft.strokes.len(), draft.zoom)
    });
    let erasing = tool.is_some();

    paint_button(dc, ui, l.magic, "Gomma magica", ok || erasing, tool == Some(true), hover == SigHit::Magic);
    paint_button(dc, ui, l.eraser, "Gomma", ok || erasing, tool == Some(false), hover == SigHit::Eraser);
    let undo = if strokes > 0 { format!("Annulla ({strokes})") } else { "Annulla".to_string() };
    paint_button(dc, ui, l.undo, &undo, strokes > 0, false, hover == SigHit::Undo);
    if erasing {
        if ok {
            let badge = format!("zoom {}x", ZOOMS[zoom.min(ZOOMS.len() - 1)]);
            let corner = rect(l.preview.left, l.preview.bottom - s(22), l.preview.right - s(8), l.preview.bottom - s(4));
            draw_text(dc, &badge, corner, fonts.small, colors.muted, DT_SINGLELINE | DT_VCENTER | DT_RIGHT | DT_NOPREFIX);
        }
        draw_text(dc, ERASE_HINT, l.erase_note, fonts.small, colors.muted, DT_WORDBREAK | DT_NOPREFIX);
        paint_button(dc, ui, l.done, "Fine", true, true, hover == SigHit::Done);
        return;
    }

    draw_text(dc, "più chiara", l.ends, fonts.small, colors.muted, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);
    draw_text(dc, "più scura", l.ends, fonts.small, colors.muted, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_RIGHT);
    let (a, b) = track(ui, l.slider);
    let cy = (l.slider.top + l.slider.bottom) / 2;
    fill_round_rect(dc, rect(a, cy - s(2), b, cy + s(2)), s(4), colors.track);
    let middle = (a + b) / 2;
    fill_rect(dc, rect(middle - s(1) / 2, cy - s(6), middle + s(1) / 2 + 1, cy + s(6)), colors.border);
    let x = a + (b - a) * (options.darkness + DARKNESS_RANGE) / (2 * DARKNESS_RANGE);
    let knob = if hover == SigHit::Slider || SIGS.with(|g| g.sliding.get()) { s(10) } else { s(8) };
    fill_round_rect(dc, rect(x - knob, cy - knob, x + knob, cy + knob), 2 * knob, colors.primary);

    let checked = [options.solid, options.for_print, options.remove_print];
    for (k, row) in l.toggles.iter().enumerate() {
        paint_check(dc, ui, *row, TOGGLES[k], checked[k], hover == SigHit::Toggle(k));
    }
    draw_text(dc, "Nota", l.note_label, fonts.small, colors.muted, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);
    paint_field(dc, ui, l.note_field);

    paint_button(dc, ui, l.save, "Salva", ok, true, hover == SigHit::Save);
    paint_button(dc, ui, l.discard, "Scarta", true, false, hover == SigHit::Discard);
    let text = if ok {
        "Nota facoltativa, per ricordare di chi è la firma: finisce nel nome del file. Salva mette la firma nell'elenco e la copia negli appunti."
    } else {
        "Prova con lo slider o con le spunte, oppure scarta e cattura di nuovo solo la firma, su fondo chiaro."
    };
    draw_text(dc, text, l.draft_note, fonts.small, colors.muted, DT_WORDBREAK | DT_NOPREFIX);
}

/// Gomma sull'anteprima: passata in corso (tratto pieno del colore principale) e cerchio sotto il mouse.
fn paint_eraser(dc: HDC, ui: &Ui, l: &Layout, draft: &Draft, hovered: bool) {
    if !draft.erasing() {
        return;
    }
    let colors = ui.palette.get();
    let r = scale(ui, ERASER);
    unsafe {
        let saved = SaveDC(dc);
        IntersectClipRect(dc, l.preview.left, l.preview.top, l.preview.right, l.preview.bottom);
        if !draft.live.is_empty() {
            let mut points = draft.live.clone();
            if points.len() == 1 {
                points.push(points[0]);
            }
            with_pen(dc, PS_SOLID, 2 * r, colors.primary, || {
                Polyline(dc, points.as_ptr(), points.len() as i32);
            });
        }
        if hovered || !draft.live.is_empty() {
            let p = SIGS.with(|g| g.pointer.get());
            let old = SelectObject(dc, GetStockObject(NULL_BRUSH));
            with_pen(dc, PS_SOLID, scale(ui, 1).max(1), colors.err, || {
                Ellipse(dc, p.x - r, p.y - r, p.x + r + 1, p.y + r + 1);
            });
            SelectObject(dc, old);
        }
        RestoreDC(dc, saved);
    }
}

// ---------- input ----------

/// Ricalcola l'anteprima con le scelte nuove. Vero se sono cambiate.
fn set_options(change: impl FnOnce(&mut Options)) -> bool {
    let changed = SIGS.with(|g| {
        let mut draft = g.draft.borrow_mut();
        let Some(draft) = draft.as_mut() else { return false };
        let mut options = draft.options;
        change(&mut options);
        if options == draft.options {
            return false;
        }
        draft.options = options;
        draft.refresh();
        g.defaults.set(Options { darkness: 0, ..options });
        true
    });
    changed
}

/// Tasto sinistro premuto: inizia un trascinamento della firma o dello slider. Vero se gestito.
pub(super) fn mouse_down(hwnd: HWND, ui: &Ui, x: i32, y: i32) -> bool {
    match hit_test(hwnd, ui, x, y) {
        SigHit::Item(index) => {
            SIGS.with(|g| g.press.set(Some((index, POINT { x, y }))));
            unsafe { SetCapture(hwnd) };
            true
        }
        SigHit::Preview => {
            SIGS.with(|g| {
                g.pointer.set(POINT { x, y });
                if let Some(draft) = g.draft.borrow_mut().as_mut() {
                    draft.live = vec![POINT { x, y }];
                }
            });
            unsafe { SetCapture(hwnd) };
            true
        }
        SigHit::Slider => {
            SIGS.with(|g| g.sliding.set(true));
            unsafe { SetCapture(hwnd) };
            let value = darkness_at(ui, &layout(hwnd, ui), x);
            set_options(|o| o.darkness = value);
            true
        }
        _ => false,
    }
}

/// Movimento col tasto premuto: slider o firma trascinata fuori. Vero se gestito.
pub(super) fn mouse_move(hwnd: HWND, ui: &Ui, x: i32, y: i32) -> bool {
    // Gomma: la passata in corso segue il mouse; senza tasto premuto solo il cerchio (e l'hover).
    let (erasing, stroking) = SIGS.with(|g| g.draft.borrow().as_ref().map_or((false, false), |d| (d.erasing(), !d.live.is_empty())));
    if erasing {
        SIGS.with(|g| {
            g.pointer.set(POINT { x, y });
            if stroking {
                if let Some(draft) = g.draft.borrow_mut().as_mut() {
                    draft.live.push(POINT { x, y });
                }
            }
        });
        unsafe { InvalidateRect(hwnd, null(), 0) };
        if stroking {
            return true;
        }
    }
    if SIGS.with(|g| g.sliding.get()) {
        let value = darkness_at(ui, &layout(hwnd, ui), x);
        if set_options(|o| o.darkness = value) {
            unsafe { InvalidateRect(hwnd, null(), 0) };
        }
        return true;
    }
    let Some((index, start)) = SIGS.with(|g| g.press.get()) else { return false };
    if (x - start.x).abs() <= DRAG_THRESHOLD && (y - start.y).abs() <= DRAG_THRESHOLD {
        return true;
    }
    SIGS.with(|g| g.press.set(None));
    unsafe { ReleaseCapture() };
    let Some(path) = SIGS.with(|g| g.items.borrow().get(index).map(|item| item.path.clone())) else { return true };
    SIGS.with(|g| g.dragging.set(true));
    let dropped = drag_source::drag_file(&path);
    SIGS.with(|g| g.dragging.set(false));
    if dropped {
        note("Firma inserita.", false);
    }
    unsafe { InvalidateRect(hwnd, null(), 0) };
    true
}

/// Tasto sinistro rilasciato: fine dello slider o clic su una firma (copia). Vero se gestito.
pub(super) fn mouse_up(hwnd: HWND, ui: &Ui) -> bool {
    if finish_stroke(hwnd, ui) {
        unsafe { ReleaseCapture() };
        return true;
    }
    if SIGS.with(|g| g.sliding.replace(false)) {
        unsafe { ReleaseCapture() };
        return true;
    }
    let Some((index, _)) = SIGS.with(|g| g.press.replace(None)) else { return false };
    unsafe { ReleaseCapture() };
    copy(hwnd, index);
    true
}

/// Fine della passata di gomma: riportata sulla sorgente e firma ripulita. Vero se c'era una passata.
fn finish_stroke(hwnd: HWND, ui: &Ui) -> bool {
    let area = layout(hwnd, ui).preview;
    let radius = scale(ui, ERASER) as f32;
    SIGS.with(|g| {
        let mut draft = g.draft.borrow_mut();
        let Some(draft) = draft.as_mut() else { return false };
        if draft.live.is_empty() {
            return false;
        }
        let live = std::mem::take(&mut draft.live);
        let mapped: Vec<(f32, f32, f32)> = live.iter().filter_map(|&p| draft.to_source(area, p)).collect();
        if let Some(&(_, _, ratio)) = mapped.first() {
            let points = mapped.iter().map(|&(x, y, _)| (x, y)).collect();
            draft.strokes.push(Stroke { points, radius: radius * ratio, magic: draft.tool == Some(true) });
            draft.refresh();
        }
        true
    })
}

fn copy(hwnd: HWND, index: usize) {
    let done = SIGS.with(|g| {
        let items = g.items.borrow();
        let item = items.get(index)?;
        let png = fs::read(&item.path).ok()?;
        Some(clipboard::copy_signature(hwnd, &png, &item.image))
    });
    match done {
        Some(true) => note("Firma copiata negli appunti: incollala con Ctrl+V.", false),
        Some(false) => note("Appunti occupati da un'altra app: riprova.", true),
        None => {
            note("Firma non più disponibile: elenco aggiornato.", true);
            reload();
        }
    }
}

pub(super) fn on_click(hwnd: HWND, hit: SigHit) {
    match hit {
        SigHit::Capture => {
            note("", false);
            if let Some(image) = capture::capture(&[hwnd]) {
                start_draft(image);
            }
        }
        SigHit::Toggle(0) => {
            set_options(|o| o.solid = !o.solid);
        }
        SigHit::Toggle(1) => {
            set_options(|o| o.for_print = !o.for_print);
        }
        SigHit::Toggle(_) => {
            set_options(|o| o.remove_print = !o.remove_print);
        }
        SigHit::Magic | SigHit::Eraser | SigHit::Done => SIGS.with(|g| {
            if let Some(draft) = g.draft.borrow_mut().as_mut() {
                let wanted = match hit {
                    SigHit::Magic => Some(true),
                    SigHit::Eraser => Some(false),
                    _ => None,
                };
                // Lo stesso tasto una seconda volta spegne la gomma.
                draft.set_tool(if draft.tool == wanted { None } else { wanted });
            }
        }),
        SigHit::Undo => SIGS.with(|g| {
            if let Some(draft) = g.draft.borrow_mut().as_mut() {
                if draft.strokes.pop().is_some() {
                    draft.refresh();
                }
            }
        }),
        SigHit::Delete(index) => delete(index),
        SigHit::Save => save(hwnd),
        SigHit::Discard => SIGS.with(|g| *g.draft.borrow_mut() = None),
        SigHit::Item(_) | SigHit::Slider | SigHit::Preview | SigHit::Nothing => {}
    }
}

fn save(hwnd: HWND) {
    let cleaned = SIGS.with(|g| g.draft.borrow().as_ref().and_then(|d| d.cleaned.as_ref().ok().map(|p| (p.image.clone(), p.dpi))));
    let Some((image, dpi)) = cleaned else { return };
    let text = get_text(SIGS.with(|g| g.edit.get()));
    let saved = signature::to_png(&image, dpi).and_then(|png| signature::save(&png, &text).map(|path| (png, path)));
    match saved {
        Ok((png, _)) => {
            SIGS.with(|g| *g.draft.borrow_mut() = None);
            reload();
            SIGS.with(|g| g.scroll.set(0));
            if clipboard::copy_signature(hwnd, &png, &image) {
                note("Firma salvata e copiata negli appunti.", false);
            } else {
                note("Firma salvata. Appunti occupati: clic sulla firma per copiarla.", false);
            }
        }
        Err(error) => note(&format!("Firma non salvata: {}", error_text(error)), true),
    }
}

fn delete(index: usize) {
    let Some((path, label)) = SIGS.with(|g| g.items.borrow().get(index).map(|item| (item.path.clone(), item.label.clone()))) else { return };
    let text = format!("Eliminare questa firma?\n\n{label}");
    if win::message_box(main_hwnd(), &text, APP_TITLE, MB_YESNO | MB_ICONQUESTION | MB_TOPMOST | MB_DEFBUTTON2) != IDYES {
        return;
    }
    match signature::delete(&path) {
        Ok(()) => note("Firma eliminata.", false),
        Err(error) => note(&error_text(error), true),
    }
    reload();
}

/// Menu a comparsa nel punto indicato (coordinate dello schermo). Id 0 = separatore.
/// Restituisce l'id scelto, 0 se annullato.
fn popup(owner: HWND, items: &[(usize, String)], at: POINT) -> usize {
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return 0;
        }
        for (id, label) in items {
            if *id == 0 {
                AppendMenuW(menu, MF_SEPARATOR, 0, null());
            } else {
                // & nel testo dei menu sottolinea la lettera dopo: va raddoppiato.
                AppendMenuW(menu, MF_STRING, *id, wide(&label.replace('&', "&&")).as_ptr());
            }
        }
        // Senza primo piano il menu non si chiude cliccando altrove.
        SetForegroundWindow(owner);
        let choice = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY, at.x, at.y, 0, owner, null());
        PostMessageW(owner, WM_NULL, 0, 0);
        DestroyMenu(menu);
        choice.max(0) as usize
    }
}

/// Scelta della firma in Strumenti.
pub(crate) enum Pick {
    Saved(PathBuf),
    Other,
    Cancel,
}

/// Menu sotto il cursore con le firme salvate e "Altro file…". Senza firme salvate: Altro file.
pub(crate) fn pick_for_tools(owner: HWND) -> Pick {
    const OTHER: usize = 1;
    let saved = signature::list();
    if saved.is_empty() {
        return Pick::Other;
    }
    let mut items: Vec<(usize, String)> = saved.iter().enumerate().map(|(k, path)| (k + 2, signature::label(path))).collect();
    items.push((0, String::new()));
    items.push((OTHER, "Altro file…".into()));
    let mut at = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut at) };
    match popup(owner, &items, at) {
        0 => Pick::Cancel,
        OTHER => Pick::Other,
        id => saved.get(id - 2).cloned().map_or(Pick::Cancel, Pick::Saved),
    }
}

/// Tasto destro su una firma: Copia, Elimina, Apri cartella firme.
pub(super) fn context_menu(hwnd: HWND, ui: &Ui, x: i32, y: i32) {
    let SigHit::Item(index) = hit_test(hwnd, ui, x, y) else { return };
    let Some(path) = SIGS.with(|g| g.items.borrow().get(index).map(|item| item.path.clone())) else { return };
    let items = [(MENU_COPY, "Copia".to_string()), (MENU_DELETE, "Elimina…".into()), (0, String::new()), (MENU_FOLDER, "Apri cartella firme".into())];
    let mut point = POINT { x, y };
    unsafe { ClientToScreen(hwnd, &mut point) };
    match popup(hwnd, &items, point) {
        MENU_COPY => copy(hwnd, index),
        MENU_DELETE => delete(index),
        MENU_FOLDER => super::reveal(&path, false),
        _ => {}
    }
}

//! Pannello della zona Strumenti: miniature selezionabili, rotazioni, estrai ed elimina,
//! numeri, timbro, firma, password e JPG. Le modifiche restano in sospeso fino a Salva.
//! I campi di testo sono controlli EDIT di Windows; il resto e' disegnato in GDI come il pannello.

use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LRESULT, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_RETURN, VK_SHIFT};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use super::signatures_panel::{self, Pick};
use super::paint::{client_rect, draw_text, fill_rect, fill_round_rect, line, outline, paint_button, with_pen, MARGIN};
use super::panel::{draw_thumb, paint_placeholder, paint_remove, HEADER};
use super::win::{rect, wide, Cell2, RectExt};
use super::{dialogs, main_hwnd, scale, win, with_tools, ToolsTask, Ui, APP_TITLE};
use crate::settings::{self, ToolsPrefs, STAMP_MAX_CHARS};
use crate::tools::edit::{self, Plan, Signature};
use crate::tools::preview::Thumb;
use crate::tools::{file_stem, Source};

const FILE_ROW: i32 = 34;
const LABEL: i32 = 20;
const BUTTON: i32 = 28;
const CHECK: i32 = 26;
const FIELD: i32 = 26;
const SELECTION: i32 = 28;
const SUMMARY: i32 = 18;
const FOOTER_BUTTON: i32 = 34;
const INDENT: i32 = 26;
const COLUMNS: i32 = 4;
const CELL_GAP: i32 = 8;
const STAMP_BLOCK: i32 = BUTTON + 4 + FIELD + 6;
const SIGNATURE_BLOCK: i32 = BUTTON + 4 + 46 + 6;
const PASSWORD_BLOCK: i32 = 2 * (FIELD + 4) + 2;
const PRESETS: [&str; 3] = ["RISERVATO", "COPIA", "BOZZA"];
const CHECKS: [&str; 4] = ["Numeri \"Pagina 1 di N\"", "Timbro", "Firma", "Password"];
const SIZES: [&str; 3] = ["S", "M", "L"];

/// Messaggio EDIT per la lunghezza massima (in Win32_UI_Controls, feature non attivata).
const EM_LIMITTEXT: u32 = 0x00C5;

/// Campi di testo: timbro, nuova password, ripeti, password per sbloccare.
const E_STAMP: usize = 0;
const E_NEW: usize = 1;
const E_REPEAT: usize = 2;
const E_UNLOCK: usize = 3;

/// Zone del pannello per il trascinamento.
pub(crate) const DROP_FILE: usize = 0;
pub(crate) const DROP_SIGNATURE: usize = 1;

/// Documento aperto nella zona Strumenti; lo riempie il thread delle anteprime.
pub(crate) struct ToolsDoc {
    pub id: u64,
    pub name: String,
    pub path: Option<PathBuf>,
    /// Byte ricevuti: servono a sbloccare un PDF protetto.
    pub original: Vec<u8>,
    /// PDF in chiaro su cui si lavora.
    pub data: Vec<u8>,
    pub was_protected: bool,
    pub pages: usize,
    /// Miniature per pagina originale; possono essere meno delle pagine.
    pub thumbs: Vec<Thumb>,
    pub state: DocState,
    pub plan: Plan,
    pub selected: Vec<bool>,
}

pub(crate) enum DocState {
    Loading,
    /// PDF protetto: serve la password. Il testo e' l'errore dell'ultimo tentativo.
    Locked(Option<String>),
    Ready,
    Failed(String),
}

impl ToolsDoc {
    pub fn loading(id: u64, name: String) -> Self {
        ToolsDoc {
            id,
            name,
            path: None,
            original: Vec::new(),
            data: Vec::new(),
            was_protected: false,
            pages: 0,
            thumbs: Vec::new(),
            state: DocState::Loading,
            plan: Plan::default(),
            selected: Vec::new(),
        }
    }

    fn kept(&self) -> Vec<usize> {
        (0..self.pages).filter(|&p| self.plan.keep.get(p).copied().unwrap_or(false)).collect()
    }

    fn chosen(&self) -> Vec<usize> {
        self.kept().into_iter().filter(|&p| self.selected.get(p).copied().unwrap_or(false)).collect()
    }
}

/// Elemento del pannello Strumenti sotto il mouse.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum ToolsHit {
    Nothing,
    CloseDoc,
    Page(usize),
    All,
    NoPages,
    Rotate(u16),
    Extract,
    Delete,
    Check(usize),
    Preset(usize),
    SigPick,
    SigClear,
    SigCell(u8),
    SigPrev,
    SigNext,
    SigSize(u8),
    Unlock,
    Jpg,
    Reset,
    Save,
}

struct SignatureImage {
    name: String,
    path: Option<PathBuf>,
    bytes: Vec<u8>,
}

struct ToolsUi {
    scroll: Cell2<i32>,
    /// Aggiungi: numeri, timbro, firma, password.
    checks: Cell2<[bool; 4]>,
    signature: RefCell<Option<SignatureImage>>,
    /// Pagina della firma tra quelle rimaste; None = ultima.
    signature_page: Cell2<Option<usize>>,
    signature_position: Cell2<u8>,
    signature_size: Cell2<u8>,
    /// Ultima pagina cliccata: con Maiusc si seleziona l'intervallo.
    last_click: Cell2<Option<usize>>,
    note: RefCell<String>,
    edits: Cell2<[HWND; 4]>,
    edit_rects: Cell2<[Option<RECT>; 4]>,
    edit_font: Cell2<HFONT>,
    brush: Cell2<(HBRUSH, COLORREF)>,
}

thread_local! {
    static TOOLS: ToolsUi = ToolsUi {
        scroll: Cell2::new(0),
        checks: Cell2::new([false; 4]),
        signature: RefCell::new(None),
        signature_page: Cell2::new(None),
        signature_position: Cell2::new(8),
        signature_size: Cell2::new(1),
        last_click: Cell2::new(None),
        note: RefCell::new(String::new()),
        edits: Cell2::new([null_mut(); 4]),
        edit_rects: Cell2::new([None; 4]),
        edit_font: Cell2::new(null_mut()),
        brush: Cell2::new((null_mut(), 0)),
    };
}

fn checks() -> [bool; 4] {
    TOOLS.with(|t| t.checks.get())
}

fn edit_hwnd(index: usize) -> HWND {
    TOOLS.with(|t| t.edits.get()[index])
}

fn note(text: &str) {
    TOOLS.with(|t| *t.note.borrow_mut() = text.to_string());
}

pub(super) fn get_text(hwnd: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd).max(0) as usize;
        let mut buffer = vec![0u16; len + 1];
        let copied = GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32).max(0) as usize;
        String::from_utf16_lossy(&buffer[..copied])
    }
}

pub(super) fn set_text(hwnd: HWND, text: &str) {
    let text = wide(text);
    unsafe { SetWindowTextW(hwnd, text.as_ptr()) };
}

// ---------- campi di testo ----------

/// Crea i campi EDIT, nascosti. Le preferenze sono gia' caricate.
pub(super) fn create_children(panel: HWND, instance: HINSTANCE) {
    let class = wide("EDIT");
    let prefs = settings::tools_prefs();
    let mut edits = [null_mut(); 4];
    for (index, edit) in edits.iter_mut().enumerate() {
        let password = index != E_STAMP;
        let style = WS_CHILD | ES_AUTOHSCROLL as u32 | if password { ES_PASSWORD as u32 } else { 0 };
        unsafe {
            *edit = CreateWindowExW(0, class.as_ptr(), null(), style, 0, 0, 0, 0, panel, (100 + index) as HMENU, instance, null());
            let limit = if password { 128 } else { STAMP_MAX_CHARS };
            SendMessageW(*edit, EM_LIMITTEXT, limit, 0);
        }
    }
    set_text(edits[E_STAMP], &prefs.stamp);
    TOOLS.with(|t| {
        t.edits.set(edits);
        t.signature_position.set(prefs.signature_position.min(8));
        t.signature_size.set(prefs.signature_size.min(2));
    });
}

fn is_edit(hwnd: HWND) -> bool {
    !hwnd.is_null() && TOOLS.with(|t| t.edits.get().contains(&hwnd))
}

/// Il pannello non ruba lo stato attivo, salvo quando si clicca un campo di testo.
pub(super) fn edit_under_cursor() -> bool {
    let mut point = POINT { x: 0, y: 0 };
    unsafe {
        GetCursorPos(&mut point);
        is_edit(WindowFromPoint(point))
    }
}

/// Colori dei campi di testo, come le schede del tema.
pub(super) fn edit_colors(dc: HDC, ui: &Ui) -> LRESULT {
    let colors = ui.palette.get();
    unsafe {
        SetTextColor(dc, colors.fg);
        SetBkColor(dc, colors.card);
    }
    TOOLS.with(|t| {
        let (brush, color) = t.brush.get();
        if !brush.is_null() && color == colors.card {
            return brush as LRESULT;
        }
        unsafe {
            if !brush.is_null() {
                DeleteObject(brush);
            }
            let fresh = CreateSolidBrush(colors.card);
            t.brush.set((fresh, colors.card));
            fresh as LRESULT
        }
    })
}

/// Invio nel campo password di un PDF protetto sblocca; negli altri campi non fa nulla
/// (senza il bip del campo a riga singola).
pub(crate) fn handle_key(message: &MSG) -> bool {
    if message.message != WM_KEYDOWN || message.wParam != VK_RETURN as usize || !is_edit(message.hwnd) {
        return false;
    }
    if message.hwnd == edit_hwnd(E_UNLOCK) {
        unlock();
        super::panel::invalidate();
    }
    true
}

/// Mostra e posiziona i campi di testo secondo lo stato. Mai durante il disegno.
pub(super) fn sync_children(panel: HWND, ui: &Ui, visible: bool) {
    let facts = facts();
    let layout = layout(panel, ui, facts);
    let ready = visible && facts.view == View::Ready;
    let on = checks();
    let wanted: [Option<RECT>; 4] = [
        (ready && on[1]).then_some(layout.stamp_field),
        (ready && on[3]).then_some(layout.password_fields[0]),
        (ready && on[3]).then_some(layout.password_fields[1]),
        (visible && facts.view == View::Locked).then_some(layout.unlock_field),
    ];
    let font = ui.fonts.get().small;
    let s = |v| scale(ui, v);
    TOOLS.with(|t| {
        let edits = t.edits.get();
        if t.edit_font.replace(font) != font {
            for edit in edits {
                unsafe { SendMessageW(edit, WM_SETFONT, font as usize, 0) };
            }
        }
        let previous = t.edit_rects.get();
        for (index, edit) in edits.iter().enumerate() {
            if previous[index].map(rect_key) == wanted[index].map(rect_key) {
                continue;
            }
            unsafe {
                match wanted[index] {
                    Some(field) => {
                        let height = s(18);
                        let top = field.top + (field.bottom - field.top - height) / 2;
                        SetWindowPos(*edit, null_mut(), field.left + s(8), top, field.right - field.left - s(16), height, SWP_NOZORDER | SWP_NOACTIVATE);
                        ShowWindow(*edit, SW_SHOWNA);
                    }
                    None => {
                        ShowWindow(*edit, SW_HIDE);
                    }
                }
            }
        }
        t.edit_rects.set(wanted);
    });
}

fn rect_key(r: RECT) -> (i32, i32, i32, i32) {
    (r.left, r.top, r.right, r.bottom)
}

// ---------- stato del documento ----------

#[derive(Clone, Copy, PartialEq)]
enum View {
    Empty,
    Loading,
    Locked,
    Ready,
    Failed,
}

#[derive(Clone, Copy)]
struct Facts {
    view: View,
    was_protected: bool,
}

fn facts() -> Facts {
    with_tools(|doc| match doc {
        None => Facts { view: View::Empty, was_protected: false },
        Some(doc) => Facts {
            view: match doc.state {
                DocState::Loading => View::Loading,
                DocState::Locked(_) => View::Locked,
                DocState::Ready => View::Ready,
                DocState::Failed(_) => View::Failed,
            },
            was_protected: doc.was_protected,
        },
    })
    .unwrap_or(Facts { view: View::Empty, was_protected: false })
}

/// Nuovo documento o documento chiuso: si riparte da zero, tranne testo del timbro e firma.
pub(crate) fn reset_state() {
    TOOLS.with(|t| {
        t.scroll.set(0);
        t.checks.set([false; 4]);
        t.signature_page.set(None);
        t.last_click.set(None);
        t.note.borrow_mut().clear();
    });
    for index in [E_NEW, E_REPEAT, E_UNLOCK] {
        set_text(edit_hwnd(index), "");
    }
}

// ---------- geometria ----------

struct Layout {
    file: RECT,
    close: RECT,
    grid: RECT,
    selection: RECT,
    all: RECT,
    none: RECT,
    pages_label: RECT,
    page_buttons: [RECT; 5],
    add_label: RECT,
    checks: [RECT; 4],
    presets: [RECT; 3],
    stamp_field: RECT,
    sig_zone: RECT,
    sig_name: RECT,
    sig_pick: RECT,
    sig_clear: RECT,
    sig_cells: [RECT; 9],
    sig_page_label: RECT,
    sig_prev: RECT,
    sig_page: RECT,
    sig_next: RECT,
    sig_size_label: RECT,
    sig_sizes: [RECT; 3],
    password_labels: [RECT; 2],
    password_fields: [RECT; 2],
    export_label: RECT,
    jpg: RECT,
    footer_top: i32,
    summary: RECT,
    reset: RECT,
    save: RECT,
    message: RECT,
    unlock_field: RECT,
    unlock: RECT,
    unlock_note: RECT,
}

impl Layout {
    fn empty() -> Self {
        let z = rect(0, 0, 0, 0);
        Layout {
            file: z,
            close: z,
            grid: z,
            selection: z,
            all: z,
            none: z,
            pages_label: z,
            page_buttons: [z; 5],
            add_label: z,
            checks: [z; 4],
            presets: [z; 3],
            stamp_field: z,
            sig_zone: z,
            sig_name: z,
            sig_pick: z,
            sig_clear: z,
            sig_cells: [z; 9],
            sig_page_label: z,
            sig_prev: z,
            sig_page: z,
            sig_next: z,
            sig_size_label: z,
            sig_sizes: [z; 3],
            password_labels: [z; 2],
            password_fields: [z; 2],
            export_label: z,
            jpg: z,
            footer_top: 0,
            summary: z,
            reset: z,
            save: z,
            message: z,
            unlock_field: z,
            unlock: z,
            unlock_note: z,
        }
    }
}

/// Divide r in n tasti uguali separati da gap.
fn split<const N: usize>(r: RECT, gap: i32) -> [RECT; N] {
    let n = N as i32;
    let width = (r.right - r.left - (n - 1) * gap) / n.max(1);
    std::array::from_fn(|i| {
        let x = r.left + i as i32 * (width + gap);
        let right = if i + 1 == N { r.right } else { x + width };
        rect(x, r.top, right, r.bottom)
    })
}

/// I comandi stanno in basso con altezza fissa; le miniature prendono lo spazio che resta.
fn layout(hwnd: HWND, ui: &Ui, facts: Facts) -> Layout {
    let s = |v| scale(ui, v);
    let client = client_rect(hwnd);
    let (left, right) = (s(MARGIN), client.right - s(MARGIN));
    let mut l = Layout::empty();
    let top = s(HEADER);
    l.file = rect(left, top, right, top + s(FILE_ROW));
    let close = s(26);
    l.close = rect(right - close, top + (s(FILE_ROW) - close) / 2, right, top + (s(FILE_ROW) + close) / 2);
    let below_file = l.file.bottom + s(4);
    match facts.view {
        View::Ready => {}
        View::Locked => {
            l.message = rect(left, below_file + s(8), right, below_file + s(52));
            let y = l.message.bottom + s(6);
            l.unlock = rect(right - s(90), y, right, y + s(FIELD));
            l.unlock_field = rect(left, y, l.unlock.left - s(8), y + s(FIELD));
            l.unlock_note = rect(left, y + s(FIELD) + s(8), right, y + s(FIELD) + s(48));
            return l;
        }
        _ => {
            l.message = rect(left, below_file + s(8), right, client.bottom - s(MARGIN));
            return l;
        }
    }

    // Piede: riepilogo, Annulla e Salva.
    let mut y = client.bottom - s(MARGIN);
    [l.reset, l.save] = split(rect(left, y - s(FOOTER_BUTTON), right, y), s(8));
    y -= s(FOOTER_BUTTON) + s(4);
    l.summary = rect(left, y - s(SUMMARY), right, y);
    y -= s(SUMMARY) + s(8);
    l.footer_top = y;
    y -= s(8);

    // Esporta.
    l.jpg = rect(left, y - s(BUTTON), right, y);
    y -= s(BUTTON);
    l.export_label = rect(left, y - s(LABEL), right, y);
    y -= s(LABEL) + s(8);

    // Aggiungi, con i sottopannelli delle voci spuntate.
    let on = checks();
    let block = s(LABEL)
        + 4 * s(CHECK)
        + if on[1] { s(STAMP_BLOCK) } else { 0 }
        + if on[2] { s(SIGNATURE_BLOCK) } else { 0 }
        + if on[3] { s(PASSWORD_BLOCK) } else { 0 };
    let mut row = y - block;
    y = row - s(8);
    l.add_label = rect(left, row, right, row + s(LABEL));
    row += s(LABEL);
    let inner = left + s(INDENT);
    for index in 0..4 {
        l.checks[index] = rect(left, row, right, row + s(CHECK));
        row += s(CHECK);
        match index {
            1 if on[1] => {
                l.presets = split(rect(inner, row, right, row + s(BUTTON)), s(4));
                row += s(BUTTON) + s(4);
                l.stamp_field = rect(inner, row, right, row + s(FIELD));
                row += s(FIELD) + s(6);
            }
            2 => {
                let zone_top = l.checks[2].top;
                if on[2] {
                    l.sig_clear = rect(right - s(26), row + s(1), right, row + s(BUTTON) - s(1));
                    l.sig_pick = rect(l.sig_clear.left - s(4) - s(78), row, l.sig_clear.left - s(4), row + s(BUTTON));
                    l.sig_name = rect(inner, row, l.sig_pick.left - s(6), row + s(BUTTON));
                    row += s(BUTTON) + s(4);
                    let (cell_w, cell_h) = (s(18), s(14));
                    for k in 0..9 {
                        let (c, r) = (k as i32 % 3, k as i32 / 3);
                        let x = inner + c * (cell_w + s(2));
                        let cy = row + r * (cell_h + s(2));
                        l.sig_cells[k] = rect(x, cy, x + cell_w, cy + cell_h);
                    }
                    let gx = inner + 3 * (cell_w + s(2)) + s(10);
                    let label_w = s(62);
                    l.sig_page_label = rect(gx, row, gx + label_w, row + s(22));
                    l.sig_prev = rect(gx + label_w, row, gx + label_w + s(22), row + s(22));
                    l.sig_page = rect(l.sig_prev.right, row, l.sig_prev.right + s(50), row + s(22));
                    l.sig_next = rect(l.sig_page.right, row, l.sig_page.right + s(22), row + s(22));
                    let y3 = row + s(24);
                    l.sig_size_label = rect(gx, y3, gx + label_w, y3 + s(22));
                    for k in 0..3 {
                        let x = gx + label_w + k as i32 * s(32);
                        l.sig_sizes[k] = rect(x, y3, x + s(30), y3 + s(22));
                    }
                    row += s(46) + s(6);
                }
                l.sig_zone = rect(0, zone_top, client.right, row);
            }
            3 if on[3] => {
                for k in 0..2 {
                    l.password_labels[k] = rect(inner, row, inner + s(58), row + s(FIELD));
                    l.password_fields[k] = rect(inner + s(60), row, right, row + s(FIELD));
                    row += s(FIELD) + s(4);
                }
                row += s(2);
            }
            _ => {}
        }
    }

    // Pagine.
    l.page_buttons = split(rect(left, y - s(BUTTON), right, y), s(4));
    y -= s(BUTTON);
    l.pages_label = rect(left, y - s(LABEL), right, y);
    y -= s(LABEL) + s(4);

    // Selezione.
    l.selection = rect(left, y - s(SELECTION), right, y);
    l.none = rect(right - s(72), l.selection.top + s(2), right, l.selection.bottom - s(2));
    l.all = rect(l.none.left - s(4) - s(60), l.none.top, l.none.left - s(4), l.none.bottom);
    y -= s(SELECTION) + s(4);

    l.grid = rect(0, below_file, client.right, y.max(below_file + s(40)));
    l
}

/// Larghezza della cella, altezza della miniatura e della cella con il numero.
fn grid_metrics(ui: &Ui, area: RECT) -> (i32, i32, i32) {
    let s = |v| scale(ui, v);
    let cell_w = ((area.right - area.left - 2 * s(MARGIN) - (COLUMNS - 1) * s(CELL_GAP)) / COLUMNS).max(1);
    let thumb_h = cell_w * 141 / 100;
    (cell_w, thumb_h, thumb_h + s(16))
}

fn cell_origin(ui: &Ui, area: RECT, view_index: usize, scroll: i32) -> (i32, i32) {
    let (cell_w, _, cell_h) = grid_metrics(ui, area);
    let s = |v| scale(ui, v);
    let (column, row) = (view_index as i32 % COLUMNS, view_index as i32 / COLUMNS);
    (area.left + s(MARGIN) + column * (cell_w + s(CELL_GAP)), area.top + s(4) + row * (cell_h + s(CELL_GAP)) - scroll)
}

pub(super) fn hit_test(hwnd: HWND, ui: &Ui, x: i32, y: i32) -> ToolsHit {
    let facts = facts();
    let l = layout(hwnd, ui, facts);
    if facts.view != View::Empty && l.close.contains(x, y) {
        return ToolsHit::CloseDoc;
    }
    if facts.view == View::Locked {
        return if l.unlock.contains(x, y) { ToolsHit::Unlock } else { ToolsHit::Nothing };
    }
    if facts.view != View::Ready {
        return ToolsHit::Nothing;
    }
    if l.grid.contains(x, y) {
        let scroll = TOOLS.with(|t| t.scroll.get());
        let (cell_w, _, cell_h) = grid_metrics(ui, l.grid);
        let kept = with_tools(|doc| doc.as_ref().map(ToolsDoc::kept).unwrap_or_default()).unwrap_or_default();
        for (view_index, page) in kept.into_iter().enumerate() {
            let (cx, cy) = cell_origin(ui, l.grid, view_index, scroll);
            if rect(cx, cy, cx + cell_w, cy + cell_h).contains(x, y) {
                return ToolsHit::Page(page);
            }
        }
        return ToolsHit::Nothing;
    }
    let on = checks();
    let simple = [
        (l.all, ToolsHit::All),
        (l.none, ToolsHit::NoPages),
        (l.page_buttons[0], ToolsHit::Rotate(270)),
        (l.page_buttons[1], ToolsHit::Rotate(90)),
        (l.page_buttons[2], ToolsHit::Rotate(180)),
        (l.page_buttons[3], ToolsHit::Extract),
        (l.page_buttons[4], ToolsHit::Delete),
        (l.jpg, ToolsHit::Jpg),
        (l.reset, ToolsHit::Reset),
        (l.save, ToolsHit::Save),
    ];
    if let Some((_, hit)) = simple.iter().find(|(r, _)| r.contains(x, y)) {
        return *hit;
    }
    if on[1] {
        if let Some(i) = l.presets.iter().position(|r| r.contains(x, y)) {
            return ToolsHit::Preset(i);
        }
    }
    if on[2] {
        let mut hits = vec![(l.sig_pick, ToolsHit::SigPick), (l.sig_prev, ToolsHit::SigPrev), (l.sig_next, ToolsHit::SigNext)];
        if TOOLS.with(|t| t.signature.borrow().is_some()) {
            hits.push((l.sig_clear, ToolsHit::SigClear));
        }
        if let Some((_, hit)) = hits.iter().find(|(r, _)| r.contains(x, y)) {
            return *hit;
        }
        if let Some(k) = l.sig_cells.iter().position(|r| r.contains(x, y)) {
            return ToolsHit::SigCell(k as u8);
        }
        if let Some(k) = l.sig_sizes.iter().position(|r| r.contains(x, y)) {
            return ToolsHit::SigSize(k as u8);
        }
    }
    l.checks.iter().position(|r| r.contains(x, y)).map_or(ToolsHit::Nothing, ToolsHit::Check)
}

/// Zona di rilascio nel pannello: la riga Firma riceve l'immagine, il resto apre il file.
pub(super) fn drop_zone(hwnd: HWND, ui: &Ui, x: i32, y: i32) -> usize {
    let facts = facts();
    if facts.view == View::Ready && layout(hwnd, ui, facts).sig_zone.contains(x, y) { DROP_SIGNATURE } else { DROP_FILE }
}

pub(super) fn scroll(delta: i32) {
    TOOLS.with(|t| t.scroll.set(t.scroll.get() - delta));
}

// ---------- disegno ----------

pub(super) fn paint(dc: HDC, hwnd: HWND, ui: &Ui, client: RECT, hover: ToolsHit, drop: Option<usize>) {
    let s = |v| scale(ui, v);
    let colors = ui.palette.get();
    let fonts = ui.fonts.get();
    let facts = facts();
    let l = layout(hwnd, ui, facts);

    if facts.view == View::Empty {
        let area = rect(s(MARGIN), s(HEADER) + s(MARGIN), client.right - s(MARGIN), client.bottom - s(MARGIN));
        fill_round_rect(dc, area, s(8), if drop.is_some() { colors.tint } else { colors.card });
        with_pen(dc, if drop.is_some() { PS_SOLID } else { PS_DASH }, if drop.is_some() { s(2) } else { 1 }, colors.primary, || unsafe {
            let old = SelectObject(dc, GetStockObject(NULL_BRUSH));
            RoundRect(dc, area.left, area.top, area.right, area.bottom, s(8), s(8));
            SelectObject(dc, old);
        });
        let text = "Trascina qui un PDF (o un'immagine).\n\nRuota, estrai o elimina pagine, aggiungi numeri, timbro, firma o password, salva le pagine in JPG.";
        let inner = rect(area.left + s(16), area.top + (area.bottom - area.top) / 3, area.right - s(16), area.bottom);
        draw_text(dc, text, inner, fonts.small, colors.muted, DT_WORDBREAK | DT_CENTER | DT_NOPREFIX);
        return;
    }

    let (name, pages, message) = with_tools(|doc| {
        doc.as_ref().map(|doc| {
            let message = match &doc.state {
                DocState::Loading => "Lettura del documento e preparazione delle anteprime…".to_string(),
                DocState::Locked(_) => "PDF protetto da password. Scrivi la password per aprirlo: il file salvato sarà senza password, salvo nuova scelta.".into(),
                DocState::Failed(text) => text.clone(),
                DocState::Ready => String::new(),
            };
            let error = if let DocState::Locked(Some(e)) = &doc.state { e.clone() } else { String::new() };
            (doc.name.clone(), doc.kept().len(), (message, error))
        })
    })
    .flatten()
    .unwrap_or_default();

    // Riga del file.
    let pages_text = if facts.view == View::Ready { if pages == 1 { "1 pag".to_string() } else { format!("{pages} pag") } } else { String::new() };
    let pages_w = if pages_text.is_empty() { 0 } else { s(56) };
    draw_text(dc, &name, rect(l.file.left, l.file.top, l.close.left - pages_w - s(4), l.file.bottom), fonts.body_bold, colors.fg,
        DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS);
    draw_text(dc, &pages_text, rect(l.close.left - pages_w, l.file.top, l.close.left - s(4), l.file.bottom), fonts.small, colors.muted,
        DT_SINGLELINE | DT_VCENTER | DT_RIGHT | DT_NOPREFIX);
    paint_remove(dc, ui, l.close, hover == ToolsHit::CloseDoc);
    fill_rect(dc, rect(0, l.file.bottom, client.right, l.file.bottom + 1), colors.track);
    if drop == Some(DROP_FILE) {
        with_pen(dc, PS_SOLID, s(2), colors.primary, || outline(dc, s(1), s(HEADER), client.right - s(2), client.bottom - s(2)));
    }

    let (message, error) = message;
    match facts.view {
        View::Ready => {}
        View::Locked => {
            draw_text(dc, &message, l.message, fonts.small, colors.fg, DT_WORDBREAK | DT_NOPREFIX);
            paint_field(dc, ui, l.unlock_field);
            paint_button(dc, ui, l.unlock, "Sblocca", true, true, hover == ToolsHit::Unlock);
            let note = TOOLS.with(|t| t.note.borrow().clone());
            let text = if note.is_empty() { error } else { note };
            draw_text(dc, &text, l.unlock_note, fonts.small, colors.err, DT_WORDBREAK | DT_NOPREFIX);
            return;
        }
        View::Failed => {
            draw_text(dc, &message, l.message, fonts.small, colors.err, DT_WORDBREAK | DT_NOPREFIX);
            return;
        }
        _ => {
            draw_text(dc, &message, l.message, fonts.small, colors.muted, DT_WORDBREAK | DT_NOPREFIX);
            return;
        }
    }

    paint_grid(dc, ui, l.grid, hover);

    // Selezione.
    let chosen = with_tools(|doc| doc.as_ref().map_or(0, |d| d.chosen().len())).unwrap_or(0);
    let selection_text = match chosen {
        0 => "Clic sulle pagine per sceglierle".to_string(),
        1 => "1 selezionata".to_string(),
        n => format!("{n} selezionate"),
    };
    draw_text(dc, &selection_text, rect(l.selection.left, l.selection.top, l.all.left - s(4), l.selection.bottom), fonts.small, colors.muted,
        DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS);
    paint_button(dc, ui, l.all, "Tutte", true, false, hover == ToolsHit::All);
    paint_button(dc, ui, l.none, "Nessuna", chosen > 0, false, hover == ToolsHit::NoPages);

    // Pagine.
    let target = if chosen > 0 { "selezionate" } else { "tutte" };
    section(dc, ui, l.pages_label, &format!("Pagine ({target})"));
    let labels = ["↺ 90°", "↻ 90°", "180°", "Estrai", "Elimina"];
    let hits = [ToolsHit::Rotate(270), ToolsHit::Rotate(90), ToolsHit::Rotate(180), ToolsHit::Extract, ToolsHit::Delete];
    for i in 0..5 {
        let enabled = i < 3 || chosen > 0;
        paint_button(dc, ui, l.page_buttons[i], labels[i], enabled, false, hover == hits[i]);
    }

    // Aggiungi.
    section(dc, ui, l.add_label, "Aggiungi al salvataggio");
    let on = checks();
    let signature_drop = drop == Some(DROP_SIGNATURE);
    if signature_drop {
        fill_rect(dc, l.sig_zone, colors.tint);
    }
    for index in 0..4 {
        let mut label = CHECKS[index].to_string();
        if index == 2 && !on[2] {
            label.push_str(" (immagine, anche trascinata qui)");
        }
        if index == 3 && facts.was_protected && !on[3] {
            label.push_str(" (ora si salva senza)");
        }
        paint_check(dc, ui, l.checks[index], &label, on[index], hover == ToolsHit::Check(index));
    }
    if on[1] {
        let current = get_text(edit_hwnd(E_STAMP));
        for (i, preset) in PRESETS.iter().enumerate() {
            paint_chip(dc, ui, l.presets[i], preset, current.trim().eq_ignore_ascii_case(preset), hover == ToolsHit::Preset(i));
        }
        paint_field(dc, ui, l.stamp_field);
    }
    if on[2] {
        paint_signature(dc, ui, &l, hover);
    }
    if on[3] {
        for (k, text) in ["Nuova", "Ripeti"].iter().enumerate() {
            draw_text(dc, text, l.password_labels[k], fonts.small, colors.muted, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);
            paint_field(dc, ui, l.password_fields[k]);
        }
    }

    // Esporta.
    section(dc, ui, l.export_label, "Esporta subito");
    paint_button(dc, ui, l.jpg, "Pagine in JPG", true, false, hover == ToolsHit::Jpg);

    // Piede.
    fill_rect(dc, rect(0, l.footer_top, client.right, l.footer_top + 1), colors.border);
    let pending = TOOLS.with(|t| t.note.borrow().clone());
    let (summary, can_save) = summary(facts);
    let (text, color) = if pending.is_empty() { (summary, colors.muted) } else { (pending, colors.err) };
    draw_text(dc, &text, l.summary, fonts.small, color, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS);
    paint_button(dc, ui, l.reset, "Annulla", can_save, false, hover == ToolsHit::Reset);
    paint_button(dc, ui, l.save, "Salva", can_save, true, hover == ToolsHit::Save);
}

fn section(dc: HDC, ui: &Ui, r: RECT, text: &str) {
    let colors = ui.palette.get();
    draw_text(dc, text, r, ui.fonts.get().body_bold, colors.fg, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS);
}

fn paint_grid(dc: HDC, ui: &Ui, area: RECT, hover: ToolsHit) {
    let s = |v| scale(ui, v);
    let colors = ui.palette.get();
    let fonts = ui.fonts.get();
    with_tools(|doc| {
        let Some(doc) = doc.as_ref() else { return };
        let kept = doc.kept();
        let (cell_w, thumb_h, cell_h) = grid_metrics(ui, area);
        let rows = (kept.len() as i32 + COLUMNS - 1) / COLUMNS;
        let max_scroll = (rows * (cell_h + s(CELL_GAP)) + s(4) - (area.bottom - area.top)).max(0);
        let scroll = TOOLS.with(|t| {
            t.scroll.set(t.scroll.get().clamp(0, max_scroll));
            t.scroll.get()
        });
        unsafe {
            let saved = SaveDC(dc);
            IntersectClipRect(dc, area.left, area.top, area.right, area.bottom);
            for (view_index, &page) in kept.iter().enumerate() {
                let (x, y) = cell_origin(ui, area, view_index, scroll);
                if y + cell_h < area.top || y > area.bottom {
                    continue;
                }
                let selected = doc.selected.get(page).copied().unwrap_or(false);
                let cell = rect(x - s(3), y - s(3), x + cell_w + s(3), y + cell_h);
                if selected {
                    fill_round_rect(dc, cell, s(6), colors.tint);
                } else if hover == ToolsHit::Page(page) {
                    fill_round_rect(dc, cell, s(6), colors.card);
                }
                let thumb_box = rect(x, y, x + cell_w, y + thumb_h);
                match doc.thumbs.get(page) {
                    Some(thumb) => draw_thumb(dc, ui, thumb, thumb_box),
                    None => paint_placeholder(dc, ui, thumb_box, &format!("{}", view_index + 1)),
                }
                if selected {
                    with_pen(dc, PS_SOLID, s(2), colors.primary, || outline(dc, cell.left + s(1), cell.top + s(1), cell.right - s(2), cell.bottom - s(2)));
                }
                let label = if selected { format!("✓ {}", view_index + 1) } else { format!("{}", view_index + 1) };
                draw_text(dc, &label, rect(x, y + thumb_h, x + cell_w, y + cell_h), fonts.small, if selected { colors.primary } else { colors.muted },
                    DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX);
            }
            RestoreDC(dc, saved);
        }
    });
}

pub(super) fn paint_field(dc: HDC, ui: &Ui, r: RECT) {
    let colors = ui.palette.get();
    let s = |v| scale(ui, v);
    fill_round_rect(dc, r, s(6), colors.border);
    fill_round_rect(dc, rect(r.left + 1, r.top + 1, r.right - 1, r.bottom - 1), s(6), colors.card);
}

fn paint_chip(dc: HDC, ui: &Ui, r: RECT, text: &str, selected: bool, hover: bool) {
    let colors = ui.palette.get();
    let fill = if selected { colors.primary } else if hover { colors.tint } else { colors.track };
    let inner = rect(r.left + scale(ui, 1), r.top, r.right - scale(ui, 1), r.bottom);
    fill_round_rect(dc, inner, scale(ui, 6), fill);
    draw_text(dc, text, inner, ui.fonts.get().small, if selected { colors.on_primary } else { colors.fg },
        DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX | DT_END_ELLIPSIS);
}

pub(super) fn paint_check(dc: HDC, ui: &Ui, row: RECT, label: &str, checked: bool, hover: bool) {
    let s = |v| scale(ui, v);
    let colors = ui.palette.get();
    if hover {
        fill_round_rect(dc, row, s(6), colors.card);
    }
    let cy = (row.top + row.bottom) / 2;
    let size = s(14);
    let square = rect(row.left + s(4), cy - size / 2, row.left + s(4) + size, cy - size / 2 + size);
    if checked {
        fill_round_rect(dc, square, s(3), colors.primary);
        with_pen(dc, PS_SOLID, s(1) + 1, colors.on_primary, || {
            line(dc, square.left + s(3), cy, square.left + s(6), cy + s(3));
            line(dc, square.left + s(6), cy + s(3), square.right - s(3), cy - s(3));
        });
    } else {
        fill_round_rect(dc, square, s(3), colors.border);
        fill_round_rect(dc, rect(square.left + 1, square.top + 1, square.right - 1, square.bottom - 1), s(3), colors.card);
    }
    draw_text(dc, label, rect(square.right + s(8), row.top, row.right, row.bottom), ui.fonts.get().small, colors.fg,
        DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS);
}

fn paint_signature(dc: HDC, ui: &Ui, l: &Layout, hover: ToolsHit) {
    let s = |v| scale(ui, v);
    let colors = ui.palette.get();
    let fonts = ui.fonts.get();
    let name = TOOLS.with(|t| t.signature.borrow().as_ref().map(|sig| sig.name.clone()));
    let (text, color) = match &name {
        Some(name) => (name.clone(), colors.fg),
        None => ("Nessuna immagine".to_string(), colors.muted),
    };
    draw_text(dc, &text, l.sig_name, fonts.small, color, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS);
    paint_button(dc, ui, l.sig_pick, "Scegli…", true, false, hover == ToolsHit::SigPick);
    if name.is_some() {
        paint_remove(dc, ui, l.sig_clear, hover == ToolsHit::SigClear);
    }
    let position = TOOLS.with(|t| t.signature_position.get());
    for (k, cell) in l.sig_cells.iter().enumerate() {
        let fill = if k as u8 == position { colors.primary } else if hover == ToolsHit::SigCell(k as u8) { colors.tint } else { colors.track };
        fill_round_rect(dc, *cell, s(3), fill);
    }
    draw_text(dc, "Pagina", l.sig_page_label, fonts.small, colors.muted, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);
    let page = match TOOLS.with(|t| t.signature_page.get()) {
        None => "ultima".to_string(),
        Some(p) => format!("{}", p + 1),
    };
    paint_chip(dc, ui, l.sig_prev, "‹", false, hover == ToolsHit::SigPrev);
    draw_text(dc, &page, l.sig_page, fonts.small, colors.fg, DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX);
    paint_chip(dc, ui, l.sig_next, "›", false, hover == ToolsHit::SigNext);
    draw_text(dc, "Larghezza", l.sig_size_label, fonts.small, colors.muted, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);
    let size = TOOLS.with(|t| t.signature_size.get());
    for (k, r) in l.sig_sizes.iter().enumerate() {
        paint_chip(dc, ui, *r, SIZES[k], k as u8 == size, hover == ToolsHit::SigSize(k as u8));
    }
}

/// Riepilogo delle modifiche in sospeso e se Salva ha qualcosa da fare.
fn summary(facts: Facts) -> (String, bool) {
    let mut parts: Vec<String> = Vec::new();
    with_tools(|doc| {
        if let Some(doc) = doc.as_ref() {
            let rotated = doc.kept().iter().filter(|&&p| doc.plan.rotation.get(p).is_some_and(|r| r % 360 != 0)).count();
            if rotated > 0 {
                parts.push(format!("ruota {rotated}"));
            }
            let removed = doc.plan.keep.iter().filter(|k| !**k).count();
            if removed > 0 {
                parts.push(format!("-{removed} p."));
            }
        }
    });
    let on = checks();
    for (index, label) in ["numeri", "timbro", "firma", "password"].iter().enumerate() {
        if on[index] {
            parts.push(label.to_string());
        }
    }
    if facts.was_protected && !on[3] {
        parts.push("senza password".into());
    }
    if parts.is_empty() { ("Nessuna modifica in sospeso".into(), false) } else { (format!("Modifiche: {}", parts.join(", ")), true) }
}

// ---------- azioni ----------

pub(super) fn on_click(hit: ToolsHit) {
    if hit != ToolsHit::Nothing {
        note("");
    }
    match hit {
        ToolsHit::Nothing => {}
        ToolsHit::CloseDoc => {
            with_tools(|doc| *doc = None);
            reset_state();
        }
        ToolsHit::Page(page) => click_page(page),
        ToolsHit::All => select_all(true),
        ToolsHit::NoPages => select_all(false),
        ToolsHit::Rotate(degrees) => rotate(degrees),
        ToolsHit::Extract => extract(),
        ToolsHit::Delete => delete(),
        ToolsHit::Check(index) => toggle_check(index),
        ToolsHit::Preset(index) => set_text(edit_hwnd(E_STAMP), PRESETS[index]),
        ToolsHit::SigPick => match signatures_panel::pick_for_tools(main_hwnd()) {
            Pick::Saved(path) => load_signature(Source::from_path(path)),
            Pick::Other => {
                if let Some(path) = dialogs::pick_image(main_hwnd()) {
                    load_signature(Source::from_path(path));
                }
            }
            Pick::Cancel => {}
        },
        ToolsHit::SigClear => {
            TOOLS.with(|t| *t.signature.borrow_mut() = None);
        }
        ToolsHit::SigCell(k) => TOOLS.with(|t| t.signature_position.set(k)),
        ToolsHit::SigSize(k) => TOOLS.with(|t| t.signature_size.set(k)),
        ToolsHit::SigPrev => step_signature_page(false),
        ToolsHit::SigNext => step_signature_page(true),
        ToolsHit::Unlock => unlock(),
        ToolsHit::Jpg => export_jpg(),
        ToolsHit::Reset => reset_changes(),
        ToolsHit::Save => save(),
    }
}

fn click_page(page: usize) {
    let shift = unsafe { GetKeyState(VK_SHIFT as i32) } < 0;
    let last = TOOLS.with(|t| t.last_click.replace(Some(page)));
    with_tools(|doc| {
        let Some(doc) = doc.as_mut() else { return };
        match last.filter(|_| shift) {
            Some(from) => {
                let (a, b) = (from.min(page), from.max(page));
                for p in doc.kept().into_iter().filter(|p| (a..=b).contains(p)) {
                    doc.selected[p] = true;
                }
            }
            None => {
                if let Some(flag) = doc.selected.get_mut(page) {
                    *flag = !*flag;
                }
            }
        }
    });
}

fn select_all(on: bool) {
    with_tools(|doc| {
        if let Some(doc) = doc.as_mut() {
            for p in doc.kept() {
                doc.selected[p] = on;
            }
        }
    });
}

/// Ruota le pagine scelte, o tutte se nessuna e' scelta. Le miniature girano subito.
fn rotate(degrees: u16) {
    with_tools(|doc| {
        let Some(doc) = doc.as_mut() else { return };
        let chosen = doc.chosen();
        let targets = if chosen.is_empty() { doc.kept() } else { chosen };
        for p in targets {
            doc.plan.rotation[p] = (doc.plan.rotation[p] + degrees) % 360;
            if let Some(thumb) = doc.thumbs.get_mut(p) {
                *thumb = rotate_thumb(thumb, degrees);
            }
        }
    });
}

/// Ruota i pixel della miniatura in senso orario di 90, 180 o 270 gradi.
fn rotate_thumb(thumb: &Thumb, degrees: u16) -> Thumb {
    let (w, h) = (thumb.width as usize, thumb.height as usize);
    let turns = (degrees / 90) % 4;
    if turns == 0 || w == 0 || h == 0 {
        return Thumb { width: thumb.width, height: thumb.height, bgra: thumb.bgra.clone() };
    }
    let (nw, nh) = if turns == 2 { (w, h) } else { (h, w) };
    let mut bgra = vec![0u8; nw * nh * 4];
    for y in 0..h {
        for x in 0..w {
            let (nx, ny) = match turns {
                1 => (h - 1 - y, x),
                2 => (w - 1 - x, h - 1 - y),
                _ => (y, w - 1 - x),
            };
            let from = (y * w + x) * 4;
            let to = (ny * nw + nx) * 4;
            bgra[to..to + 4].copy_from_slice(&thumb.bgra[from..from + 4]);
        }
    }
    Thumb { width: nw as u32, height: nh as u32, bgra }
}

fn delete() {
    let refused = with_tools(|doc| {
        let doc = doc.as_mut()?;
        let chosen = doc.chosen();
        if chosen.is_empty() {
            return Some("Seleziona le pagine da eliminare");
        }
        if chosen.len() == doc.kept().len() {
            return Some("Deve restare almeno una pagina");
        }
        for p in chosen {
            doc.plan.keep[p] = false;
            doc.selected[p] = false;
        }
        None
    })
    .flatten();
    if let Some(text) = refused {
        note(text);
    }
    clamp_signature_page();
}

fn clamp_signature_page() {
    let kept = with_tools(|doc| doc.as_ref().map_or(0, |d| d.kept().len())).unwrap_or(0);
    TOOLS.with(|t| {
        if t.signature_page.get().is_some_and(|p| p + 1 >= kept) {
            t.signature_page.set(None);
        }
    });
}

/// Ordine del menu della pagina: 1, 2, ... penultima, poi "ultima".
fn step_signature_page(forward: bool) {
    let kept = with_tools(|doc| doc.as_ref().map_or(0, |d| d.kept().len())).unwrap_or(0);
    if kept < 2 {
        return;
    }
    let choices = kept; // 0..kept-1 come Some, l'ultima e' None
    TOOLS.with(|t| {
        let current = t.signature_page.get().unwrap_or(choices - 1);
        let next = if forward { (current + 1) % choices } else { (current + choices - 1) % choices };
        t.signature_page.set(if next == choices - 1 { None } else { Some(next) });
    });
}

fn toggle_check(index: usize) {
    let mut on = checks();
    on[index] = !on[index];
    TOOLS.with(|t| t.checks.set(on));
    if index == 2 && on[2] && TOOLS.with(|t| t.signature.borrow().is_none()) {
        // Si riprende la firma usata l'ultima volta, se c'e' ancora.
        if let Some(path) = settings::tools_prefs().signature.filter(|p| p.is_file()) {
            load_signature(Source::from_path(path));
        }
    }
    if index == 3 && !on[3] {
        set_text(edit_hwnd(E_NEW), "");
        set_text(edit_hwnd(E_REPEAT), "");
    }
}

/// Immagine della firma da un file scelto o trascinato (anche da Outlook).
pub(super) fn load_signature(source: Source) {
    let Source { name, path, bytes } = source;
    let data = match (bytes, &path) {
        (Some(bytes), _) => Ok(bytes),
        (None, Some(path)) => fs::read(path).map_err(|e| e.to_string()),
        (None, None) => Err("file vuoto".into()),
    };
    match data {
        Ok(bytes) if edit::is_image(&bytes) => {
            TOOLS.with(|t| {
                *t.signature.borrow_mut() = Some(SignatureImage { name, path, bytes });
                let mut on = t.checks.get();
                on[2] = true;
                t.checks.set(on);
            });
        }
        Ok(_) => note("La firma deve essere un'immagine (PNG o JPG)"),
        Err(error) => note(&format!("Firma non leggibile: {error}")),
    }
}

fn unlock() {
    let password = get_text(edit_hwnd(E_UNLOCK));
    if password.is_empty() {
        note("Scrivi la password");
        return;
    }
    set_text(edit_hwnd(E_UNLOCK), "");
    let id = with_tools(|doc| {
        let doc = doc.as_mut()?;
        doc.state = DocState::Loading;
        Some(doc.id)
    })
    .flatten();
    if let Some(id) = id {
        super::request_unlock(id, password);
    }
}

/// Copia del documento per un lavoro in coda: nome, percorso, PDF e piano di pagine.
fn snapshot() -> Option<(String, Option<PathBuf>, Vec<u8>, Plan, Vec<usize>)> {
    with_tools(|doc| {
        let doc = doc.as_ref().filter(|d| matches!(d.state, DocState::Ready))?;
        Some((doc.name.clone(), doc.path.clone(), doc.data.clone(), doc.plan.clone(), doc.chosen()))
    })
    .flatten()
}

fn extract() {
    let Some((name, path, data, plan, chosen)) = snapshot() else { return };
    if chosen.is_empty() {
        note("Seleziona le pagine da estrarre");
        return;
    }
    super::start_tools(ToolsTask::Extract(chosen), name, path, data, plan);
}

fn export_jpg() {
    let Some((name, path, data, plan, _)) = snapshot() else { return };
    let count = plan.keep.iter().filter(|k| **k).count();
    let pages = if count == 1 { "la pagina".to_string() } else { format!("le {count} pagine") };
    let text = format!("Salvare {pages} come immagini JPG nella cartella {}_immagini?", file_stem(&name));
    if win::message_box(main_hwnd(), &text, APP_TITLE, MB_YESNO | MB_ICONQUESTION | MB_TOPMOST) == IDYES {
        super::start_tools(ToolsTask::Jpg, name, path, data, plan);
    }
}

/// Annulla: pagine, rotazioni e voci di Aggiungi tornano come all'apertura.
fn reset_changes() {
    with_tools(|doc| {
        let Some(doc) = doc.as_mut() else { return };
        for (p, thumb) in doc.thumbs.iter_mut().enumerate() {
            let rotation = doc.plan.rotation.get(p).copied().unwrap_or(0) % 360;
            if rotation != 0 {
                *thumb = rotate_thumb(thumb, 360 - rotation);
            }
        }
        doc.plan = Plan::new(doc.pages);
        doc.selected = vec![false; doc.pages];
    });
    TOOLS.with(|t| {
        t.checks.set([false; 4]);
        t.signature_page.set(None);
    });
    set_text(edit_hwnd(E_NEW), "");
    set_text(edit_hwnd(E_REPEAT), "");
}

/// Salva: tutte le modifiche in un solo nome_modificato.pdf.
fn save() {
    let facts = facts();
    if !summary(facts).1 {
        note("Nessuna modifica da salvare");
        return;
    }
    let Some((name, path, data, mut plan, _)) = snapshot() else { return };
    let on = checks();
    plan.numbers = on[0];
    let stamp = get_text(edit_hwnd(E_STAMP)).trim().to_string();
    if on[1] {
        if stamp.is_empty() {
            note("Scrivi il testo del timbro");
            return;
        }
        plan.stamp = Some(stamp.clone());
    }
    if on[2] {
        let image = TOOLS.with(|t| t.signature.borrow().as_ref().map(|sig| sig.bytes.clone()));
        let Some(image) = image else {
            note("Scegli l'immagine della firma");
            return;
        };
        let (page, position, size) = TOOLS.with(|t| (t.signature_page.get(), t.signature_position.get(), t.signature_size.get()));
        plan.signature = Some(Signature { image, page, position, size });
    }
    if on[3] {
        let (first, second) = (get_text(edit_hwnd(E_NEW)), get_text(edit_hwnd(E_REPEAT)));
        if first.is_empty() {
            note("Scrivi la nuova password");
            return;
        }
        if first != second {
            note("Le due password non coincidono");
            return;
        }
        plan.password = Some(first);
    }
    remember_prefs(&stamp);
    super::start_tools(ToolsTask::Save, name, path, data, plan);
}

/// Testo del timbro, firma, posizione e larghezza restano per la volta dopo. La password no.
fn remember_prefs(stamp: &str) {
    let previous = settings::tools_prefs();
    let prefs = TOOLS.with(|t| ToolsPrefs {
        stamp: if stamp.is_empty() { previous.stamp.clone() } else { stamp.to_string() },
        signature: match t.signature.borrow().as_ref() {
            None => None,
            Some(sig) => sig.path.clone().or_else(|| previous.signature.clone()),
        },
        signature_position: t.signature_position.get(),
        signature_size: t.signature_size.get(),
    });
    if prefs != previous {
        let _ = settings::save_tools(prefs);
    }
}

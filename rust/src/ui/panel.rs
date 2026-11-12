//! Pannello accanto alla barra: lista di Unisci con anteprime, Strumenti, Info, Impostazioni.
//! Stesso thread e stessi colori della barra; si apre sul lato libero dello schermo.
//! La parte Strumenti sta in tools_panel.rs, la parte Firme in signatures_panel.rs.

use std::cell::RefCell;
use std::mem::size_of;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use super::drop_target::{self, Target};
use super::paint::{bgra_info, client_rect, draw_text, fill_rect, fill_round_rect, line, outline, paint_button, text_height, with_pen, MARGIN};
use super::signatures_panel::{self, SigHit, DROP_IMAGE};
use super::tools_panel::{self, ToolsHit, DROP_FILE, DROP_SIGNATURE};
use super::win::{rect, wide, Cell2, RectExt};
use super::{high_word, low_word, main_hwnd, monitors, scale, with_tray, Ui, APP_TITLE, AUTHOR, BUILD_DATE, UI, VERSION, ZONE_JOIN, ZONE_TOOLS};
use std::path::PathBuf;

use crate::settings::{self, Conversion, Save, Settings, Side, Theme, LIMITS_MB};
use crate::tools::preview::Thumb;
use crate::tools::Source;

const WIDTH: i32 = 340;
pub(super) const HEADER: i32 = 48;
/// Zona di rilascio del pannello Unisci: i file vanno nel vassoio.
const DROP_JOIN: usize = 2;
const ROW: i32 = 62;
const FOOTER: i32 = 96;
const SMALL_BUTTON: i32 = 26;
const THUMB_W: i32 = 36;
const THUMB_H: i32 = 48;
const COLUMNS: i32 = 3;
const GAP: i32 = 8;
const PAGES_LABEL: i32 = 30;
const GROUP_STEP: i32 = 66;
/// Riga della cartella fissa sotto "Dove salvare": sposta in giu' i gruppi successivi.
const FOLDER_ROW: i32 = 34;
const SAVE_GROUP: usize = 1;
const DRAG_THRESHOLD: i32 = 5;

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Mode {
    Hidden,
    Join,
    Tools,
    Signatures,
    Info,
    Settings,
}

#[derive(Clone, Copy, PartialEq)]
enum Hit {
    Tools(ToolsHit),
    Signatures(SigHit),
    Nothing,
    Close,
    Row(usize),
    Up(usize),
    Down(usize),
    Remove(usize),
    Join,
    Clear,
    Choice(usize, usize),
    /// Tasto Cambia… della cartella fissa.
    Folder,
    Save,
    /// Indirizzo del progetto nel pannello Info.
    Link,
}

fn web_address() -> String {
    format!("https://github.com/{}", crate::update::REPO)
}

/// Riga del link in Info: sotto titolo, versione e autore, che hanno altezza fissa.
fn link_rect(hwnd: HWND, ui: &Ui) -> RECT {
    let s = |v| scale(ui, v);
    let client = client_rect(hwnd);
    let top = s(HEADER) + s(14) + s(20) + 2 * s(18);
    rect(s(MARGIN), top, client.right - s(MARGIN), top + s(18))
}

struct Panel {
    hwnd: Cell2<HWND>,
    mode: Cell2<Mode>,
    selected: Cell2<Option<u64>>,
    list_scroll: Cell2<i32>,
    pages_scroll: Cell2<i32>,
    hover: Cell2<Hit>,
    tracking: Cell2<bool>,
    /// Riga premuta e y del clic: diventa trascinamento oltre una soglia.
    press: Cell2<Option<(usize, i32)>>,
    drop_at: Cell2<Option<usize>>,
    /// Zona sotto un file trascinato sul pannello.
    drop_zone: Cell2<Option<usize>>,
    draft: Cell2<Settings>,
    /// Cartella fissa in modifica, salvata con Salva.
    draft_folder: RefCell<Option<PathBuf>>,
    note: RefCell<String>,
}

thread_local! {
    static PANEL: Panel = Panel {
        hwnd: Cell2::new(null_mut()),
        mode: Cell2::new(Mode::Hidden),
        selected: Cell2::new(None),
        list_scroll: Cell2::new(0),
        pages_scroll: Cell2::new(0),
        hover: Cell2::new(Hit::Nothing),
        tracking: Cell2::new(false),
        press: Cell2::new(None),
        drop_at: Cell2::new(None),
        drop_zone: Cell2::new(None),
        draft: Cell2::new(Settings::default()),
        draft_folder: RefCell::new(None),
        note: RefCell::new(String::new()),
    };
}

/// Voci fisse delle impostazioni; l'ultima, Schermo, dipende dagli schermi collegati.
const FIXED_GROUPS: [(&str, &[&str]); 6] = [
    ("Limite di peso", &["1 MB", "2 MB", "5 MB", "10 MB"]),
    ("Dove salvare", &["Accanto all'orig.", "Cartella fissa", "Chiedi sempre"]),
    ("Conversione in immagini", &["Chiedi", "Sempre", "Mai"]),
    ("OCR", &["In tutte le zone", "Solo zona Testo"]),
    ("Lato della barra", &["← Sinistra", "Destra →"]),
    ("Tema", &["Come Windows", "Chiaro", "Scuro"]),
];
const SCREEN_GROUP: usize = FIXED_GROUPS.len();

fn groups() -> Vec<(&'static str, Vec<String>)> {
    let mut all: Vec<(&'static str, Vec<String>)> =
        FIXED_GROUPS.iter().map(|(label, choices)| (*label, choices.iter().map(|c| c.to_string()).collect())).collect();
    let mut screens = vec!["Automatico".to_string()];
    screens.extend(monitors::list().iter().map(|m| m.label.clone()));
    all.push(("Schermo", screens));
    all
}

fn choice_of(settings: &Settings, group: usize) -> usize {
    if group == SCREEN_GROUP {
        let list = monitors::list();
        return settings.monitor.and_then(|n| list.iter().position(|m| m.number == n)).map_or(0, |i| i + 1);
    }
    match group {
        0 => LIMITS_MB.iter().position(|&mb| mb == settings.limit_mb).unwrap_or(0),
        1 => match settings.save {
            Save::Beside => 0,
            Save::Folder => 1,
            Save::Ask => 2,
        },
        2 => match settings.conversion {
            Conversion::Ask => 0,
            Conversion::Always => 1,
            Conversion::Never => 2,
        },
        3 => usize::from(!settings.always_ocr),
        4 => usize::from(settings.side == Side::Right),
        _ => match settings.theme {
            Theme::System => 0,
            Theme::Light => 1,
            Theme::Dark => 2,
        },
    }
}

fn with_choice(mut settings: Settings, group: usize, choice: usize) -> Settings {
    if group == SCREEN_GROUP {
        settings.monitor = choice.checked_sub(1).and_then(|i| monitors::list().get(i).map(|m| m.number));
        return settings;
    }
    match group {
        0 => settings.limit_mb = LIMITS_MB[choice.min(LIMITS_MB.len() - 1)],
        1 => settings.save = [Save::Beside, Save::Folder, Save::Ask][choice.min(2)],
        2 => settings.conversion = [Conversion::Ask, Conversion::Always, Conversion::Never][choice.min(2)],
        3 => settings.always_ocr = choice == 0,
        4 => settings.side = if choice == 0 { Side::Left } else { Side::Right },
        _ => settings.theme = [Theme::System, Theme::Light, Theme::Dark][choice.min(2)],
    }
    settings
}

pub(crate) fn mode() -> Mode {
    PANEL.with(|p| p.mode.get())
}

pub(crate) fn create(instance: HINSTANCE) -> Result<(), String> {
    unsafe {
        let class_name = wide("ToolsPdfPanel");
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: null_mut(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            hbrBackground: null_mut(),
            lpszMenuName: null(),
            lpszClassName: class_name.as_ptr(),
            hIconSm: null_mut(),
        };
        if RegisterClassExW(&class) == 0 {
            return Err("registrazione del pannello non riuscita".into());
        }
        let title = wide(APP_TITLE);
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            0, 0, 0, 0,
            null_mut(), null_mut(), instance, null(),
        );
        if hwnd.is_null() {
            return Err("creazione del pannello non riuscita".into());
        }
        PANEL.with(|p| p.hwnd.set(hwnd));
        tools_panel::create_children(hwnd, instance);
        signatures_panel::create_children(hwnd, instance);
        drop_target::register(hwnd, Target::Panel)?;
    }
    Ok(())
}

pub(crate) fn destroy() {
    let hwnd = PANEL.with(|p| p.hwnd.replace(null_mut()));
    if !hwnd.is_null() {
        drop_target::revoke(hwnd);
        unsafe { DestroyWindow(hwnd) };
    }
}

pub(crate) fn invalidate() {
    let hwnd = PANEL.with(|p| p.hwnd.get());
    if !hwnd.is_null() && mode() != Mode::Hidden {
        sync_children(hwnd);
        unsafe { InvalidateRect(hwnd, null(), 0) };
    }
}

/// Campi di testo: in Strumenti e nella nota delle Firme.
fn sync_children(hwnd: HWND) {
    UI.with(|ui| {
        tools_panel::sync_children(hwnd, ui, mode() == Mode::Tools);
        signatures_panel::sync_children(hwnd, ui, mode() == Mode::Signatures);
    });
}

/// Zona del pannello sotto un punto dello schermo, durante un trascinamento.
pub(crate) fn drop_zone_at(x: i32, y: i32) -> Option<usize> {
    let hwnd = PANEL.with(|p| p.hwnd.get());
    let mut point = POINT { x, y };
    unsafe { ScreenToClient(hwnd, &mut point) };
    match mode() {
        Mode::Tools => Some(UI.with(|ui| tools_panel::drop_zone(hwnd, ui, point.x, point.y))),
        Mode::Join => Some(DROP_JOIN),
        Mode::Signatures => Some(DROP_IMAGE),
        _ => None,
    }
}

pub(crate) fn set_drop_zone(zone: Option<usize>) {
    if PANEL.with(|p| p.drop_zone.replace(zone)) != zone {
        invalidate();
    }
}

/// File rilasciati sul pannello: aprono Strumenti, vanno nel vassoio o diventano la firma.
pub(crate) fn on_drop(zone: usize, mut sources: Vec<Source>, errors: Vec<(String, String)>) {
    match zone {
        DROP_SIGNATURE if !sources.is_empty() => {
            tools_panel::load_signature(sources.remove(0));
            invalidate();
        }
        DROP_IMAGE if !sources.is_empty() => {
            signatures_panel::load_dropped(sources.remove(0));
            invalidate();
        }
        DROP_JOIN => super::on_drop(ZONE_JOIN, sources, errors),
        DROP_FILE => super::on_drop(ZONE_TOOLS, sources, errors),
        _ => {}
    }
}

fn show(mode: Mode) {
    PANEL.with(|p| {
        p.mode.set(mode);
        p.hover.set(Hit::Nothing);
        if mode == Mode::Settings {
            p.draft.set(settings::current());
            *p.draft_folder.borrow_mut() = settings::output_folder();
            p.note.borrow_mut().clear();
        }
    });
    if mode == Mode::Signatures {
        signatures_panel::reload();
    }
    reposition();
    let hwnd = PANEL.with(|p| p.hwnd.get());
    sync_children(hwnd);
    unsafe {
        ShowWindow(hwnd, SW_SHOWNA);
        InvalidateRect(hwnd, null(), 0);
        InvalidateRect(main_hwnd(), null(), 0);
    }
}

pub(crate) fn hide() {
    let hwnd = PANEL.with(|p| {
        p.mode.set(Mode::Hidden);
        p.hwnd.get()
    });
    unsafe {
        ShowWindow(hwnd, SW_HIDE);
        InvalidateRect(main_hwnd(), null(), 0);
    }
}

/// Info e Impostazioni si aprono e si chiudono con lo stesso tasto.
pub(crate) fn toggle(target: Mode) {
    if mode() == target {
        hide();
    } else {
        show(target);
    }
}

/// Mostra la lista di Unisci; se indicato, seleziona il file appena aggiunto.
pub(crate) fn show_join(select: Option<u64>) {
    PANEL.with(|p| {
        if select.is_some() {
            p.selected.set(select);
            p.pages_scroll.set(0);
        }
    });
    show(Mode::Join);
}

/// Mostra il pannello Strumenti, vuoto o con il documento aperto.
pub(crate) fn show_tools() {
    show(Mode::Tools);
}

/// Mostra il pannello Firme: elenco o anteprima della firma appena arrivata.
pub(crate) fn show_signatures() {
    show(Mode::Signatures);
}

/// Il pannello sta sul lato libero della barra, alto quanto lei.
pub(crate) fn reposition() {
    let hwnd = PANEL.with(|p| p.hwnd.get());
    let bar = main_hwnd();
    if hwnd.is_null() || bar.is_null() || mode() == Mode::Hidden {
        return;
    }
    let mut bar_rect = rect(0, 0, 0, 0);
    unsafe { GetWindowRect(bar, &mut bar_rect) };
    let width = UI.with(|ui| scale(ui, WIDTH));
    let x = if settings::current().side == Side::Left { bar_rect.right } else { bar_rect.left - width };
    let screen = monitors::containing(&bar_rect);
    let x = screen.map_or(x, |m| x.clamp(m.rect.left, (m.rect.right - width).max(m.rect.left)));
    unsafe {
        SetWindowPos(hwnd, HWND_TOPMOST, x, bar_rect.top, width, bar_rect.bottom - bar_rect.top, SWP_NOACTIVATE);
    }
}

// ---------- geometria ----------

fn list_rect(hwnd: HWND, ui: &Ui, count: usize) -> RECT {
    let s = |v| scale(ui, v);
    let client = client_rect(hwnd);
    let available = client.bottom - s(HEADER) - s(FOOTER);
    let height = (count as i32 * s(ROW)).min(available * 45 / 100);
    rect(0, s(HEADER), client.right, s(HEADER) + height)
}

fn pages_rect(hwnd: HWND, ui: &Ui, count: usize) -> RECT {
    let client = client_rect(hwnd);
    let list = list_rect(hwnd, ui, count);
    rect(0, list.bottom + scale(ui, PAGES_LABEL), client.right, client.bottom - scale(ui, FOOTER))
}

fn row_top(hwnd: HWND, ui: &Ui, count: usize, index: usize) -> i32 {
    list_rect(hwnd, ui, count).top + index as i32 * scale(ui, ROW) - PANEL.with(|p| p.list_scroll.get())
}

/// Tasti di riga, da destra: elimina, giu', su.
fn row_button(hwnd: HWND, ui: &Ui, count: usize, index: usize, position: i32) -> RECT {
    let s = |v| scale(ui, v);
    let client = client_rect(hwnd);
    let size = s(SMALL_BUTTON);
    let right = client.right - s(MARGIN) - position * (size + s(4));
    let top = row_top(hwnd, ui, count, index) + (s(ROW) - size) / 2;
    rect(right - size, top, right, top + size)
}

fn footer_buttons(hwnd: HWND, ui: &Ui) -> (RECT, RECT) {
    let s = |v| scale(ui, v);
    let client = client_rect(hwnd);
    let top = client.bottom - s(MARGIN) - s(36);
    let middle = client.right / 2;
    (
        rect(s(MARGIN), top, middle - s(4), top + s(36)),
        rect(middle + s(4), top, client.right - s(MARGIN), top + s(36)),
    )
}

fn close_rect(hwnd: HWND, ui: &Ui) -> RECT {
    let client = client_rect(hwnd);
    let size = scale(ui, 32);
    let gap = (scale(ui, HEADER) - size) / 2;
    rect(client.right - size - gap, gap, client.right - gap, gap + size)
}

/// Cima del gruppo di impostazioni: dopo "Dove salvare" c'e' la riga della cartella fissa.
fn group_top(ui: &Ui, group: usize) -> i32 {
    let s = |v| scale(ui, v);
    s(HEADER) + s(12) + group as i32 * s(GROUP_STEP) + if group > SAVE_GROUP { s(FOLDER_ROW) } else { 0 }
}

/// Riga della cartella fissa: percorso e tasto Cambia….
fn folder_rects(hwnd: HWND, ui: &Ui) -> (RECT, RECT) {
    let s = |v| scale(ui, v);
    let client = client_rect(hwnd);
    let top = group_top(ui, SAVE_GROUP) + s(24) + s(32) + s(4);
    let button = rect(client.right - s(MARGIN) - s(84), top, client.right - s(MARGIN), top + s(SMALL_BUTTON));
    (rect(s(MARGIN) + s(2), top, button.left - s(6), top + s(SMALL_BUTTON)), button)
}

fn choice_rect(hwnd: HWND, ui: &Ui, group: usize, choice: usize) -> RECT {
    let s = |v| scale(ui, v);
    let client = client_rect(hwnd);
    let count = groups().get(group).map_or(1, |g| g.1.len()).max(1) as i32;
    let left = s(MARGIN);
    let width = (client.right - 2 * s(MARGIN)) / count;
    let top = group_top(ui, group) + s(24);
    let x = left + choice as i32 * width;
    rect(x, top, x + width, top + s(32))
}

fn save_rect(hwnd: HWND, ui: &Ui) -> RECT {
    let s = |v| scale(ui, v);
    let client = client_rect(hwnd);
    let top = group_top(ui, SCREEN_GROUP + 1) + s(6);
    rect(s(MARGIN), top, client.right - s(MARGIN), top + s(36))
}

fn hit_test(hwnd: HWND, ui: &Ui, x: i32, y: i32) -> Hit {
    if close_rect(hwnd, ui).contains(x, y) {
        return Hit::Close;
    }
    match mode() {
        Mode::Tools => Hit::Tools(tools_panel::hit_test(hwnd, ui, x, y)),
        Mode::Signatures => Hit::Signatures(signatures_panel::hit_test(hwnd, ui, x, y)),
        Mode::Join => {
            let count = with_tray(|tray| tray.len()).unwrap_or(0);
            let (join, clear) = footer_buttons(hwnd, ui);
            if join.contains(x, y) {
                return if count >= 2 { Hit::Join } else { Hit::Nothing };
            }
            if clear.contains(x, y) {
                return Hit::Clear;
            }
            let list = list_rect(hwnd, ui, count);
            if !list.contains(x, y) {
                return Hit::Nothing;
            }
            for index in 0..count {
                if row_button(hwnd, ui, count, index, 0).contains(x, y) {
                    return Hit::Remove(index);
                }
                if row_button(hwnd, ui, count, index, 1).contains(x, y) {
                    return if index + 1 < count { Hit::Down(index) } else { Hit::Nothing };
                }
                if row_button(hwnd, ui, count, index, 2).contains(x, y) {
                    return if index > 0 { Hit::Up(index) } else { Hit::Nothing };
                }
                let top = row_top(hwnd, ui, count, index);
                if y >= top && y < top + scale(ui, ROW) {
                    return Hit::Row(index);
                }
            }
            Hit::Nothing
        }
        Mode::Settings => {
            for (group, (_, choices)) in groups().iter().enumerate() {
                for choice in 0..choices.len() {
                    if choice_rect(hwnd, ui, group, choice).contains(x, y) {
                        return Hit::Choice(group, choice);
                    }
                }
            }
            if folder_rects(hwnd, ui).1.contains(x, y) {
                return Hit::Folder;
            }
            if save_rect(hwnd, ui).contains(x, y) {
                return Hit::Save;
            }
            Hit::Nothing
        }
        Mode::Info if link_rect(hwnd, ui).contains(x, y) => Hit::Link,
        _ => Hit::Nothing,
    }
}

/// Posizione d'inserimento sotto il cursore durante il trascinamento di una riga.
fn insertion_at(hwnd: HWND, ui: &Ui, count: usize, y: i32) -> usize {
    let list = list_rect(hwnd, ui, count);
    let offset = y - list.top + PANEL.with(|p| p.list_scroll.get());
    let row = scale(ui, ROW).max(1);
    ((offset + row / 2) / row).clamp(0, count as i32) as usize
}

// ---------- disegno ----------

fn paint(hwnd: HWND, ui: &Ui) {
    super::paint::buffered(hwnd, |dc, client| {
        let colors = ui.palette.get();
        let fonts = ui.fonts.get();
        let s = |v| scale(ui, v);
        fill_rect(dc, client, colors.bg);
        let edge = if settings::current().side == Side::Left { 0 } else { client.right - s(1) };
        fill_rect(dc, rect(edge, 0, edge + s(1), client.bottom), colors.border);

        let title = match mode() {
            Mode::Join => match with_tray(|tray| tray.len()).unwrap_or(0) {
                1 => "Unisci 1 file".to_string(),
                n => format!("Unisci {n} file"),
            },
            Mode::Tools => "Strumenti".into(),
            Mode::Signatures => "Firme".into(),
            Mode::Info => "Informazioni".into(),
            Mode::Settings => "Impostazioni".into(),
            Mode::Hidden => String::new(),
        };
        let close = close_rect(hwnd, ui);
        draw_text(dc, &title, rect(s(MARGIN), 0, close.left - s(4), s(HEADER)), fonts.title, colors.fg, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS);
        if PANEL.with(|p| p.hover.get()) == Hit::Close {
            fill_round_rect(dc, close, s(6), colors.track);
        }
        let inset = s(11);
        with_pen(dc, PS_SOLID, s(1) + 1, colors.fg, || {
            line(dc, close.left + inset, close.top + inset, close.right - inset, close.bottom - inset);
            line(dc, close.right - inset, close.top + inset, close.left + inset, close.bottom - inset);
        });
        fill_rect(dc, rect(0, s(HEADER) - 1, client.right, s(HEADER)), colors.border);

        match mode() {
            Mode::Tools => {
                let (hover, drop) = PANEL.with(|p| (p.hover.get(), p.drop_zone.get()));
                let hover = if let Hit::Tools(hit) = hover { hit } else { ToolsHit::Nothing };
                tools_panel::paint(dc, hwnd, ui, client, hover, drop);
            }
            Mode::Signatures => {
                let (hover, drop) = PANEL.with(|p| (p.hover.get(), p.drop_zone.get()));
                let hover = if let Hit::Signatures(hit) = hover { hit } else { SigHit::Nothing };
                signatures_panel::paint(dc, hwnd, ui, client, hover, drop == Some(DROP_IMAGE));
            }
            Mode::Join => paint_join(dc, hwnd, ui, client),
            Mode::Info => paint_info(dc, hwnd, ui, client),
            Mode::Settings => paint_settings(dc, hwnd, ui, client),
            Mode::Hidden => {}
        }
    });
}

fn paint_join(dc: HDC, hwnd: HWND, ui: &Ui, client: RECT) {
    let s = |v| scale(ui, v);
    let colors = ui.palette.get();
    let fonts = ui.fonts.get();
    let hover = PANEL.with(|p| p.hover.get());

    with_tray(|tray| {
        let count = tray.len();
        let list = list_rect(hwnd, ui, count);
        let max_scroll = (count as i32 * s(ROW) - (list.bottom - list.top)).max(0);
        PANEL.with(|p| p.list_scroll.set(p.list_scroll.get().clamp(0, max_scroll)));

        // La selezione resta valida anche se il file e' stato tolto.
        let selected = PANEL.with(|p| p.selected.get()).filter(|id| tray.iter().any(|i| i.id == *id)).or(tray.first().map(|i| i.id));
        PANEL.with(|p| p.selected.set(selected));

        unsafe {
            let saved = SaveDC(dc);
            IntersectClipRect(dc, list.left, list.top, list.right, list.bottom);
            for (index, item) in tray.iter().enumerate() {
                let top = row_top(hwnd, ui, count, index);
                if top + s(ROW) < list.top || top > list.bottom {
                    continue;
                }
                let row = rect(0, top, client.right, top + s(ROW));
                if Some(item.id) == selected {
                    fill_rect(dc, row, colors.tint);
                } else if hover == Hit::Row(index) {
                    fill_rect(dc, row, colors.card);
                }
                draw_text(dc, &format!("{}", index + 1), rect(s(MARGIN) - s(4), top, s(MARGIN) + s(14), top + s(ROW)), fonts.body_bold, colors.muted,
                    DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX);

                let thumb_box = rect(s(34), top + (s(ROW) - s(THUMB_H)) / 2, s(34) + s(THUMB_W), top + (s(ROW) + s(THUMB_H)) / 2);
                match item.thumbs.first() {
                    Some(thumb) => draw_thumb(dc, ui, thumb, thumb_box),
                    None => paint_placeholder(dc, ui, thumb_box, &kind_badge(&item.label, &item.name)),
                }

                let text_left = thumb_box.right + s(10);
                let text_right = row_button(hwnd, ui, count, index, 2).left - s(6);
                draw_text(dc, &item.name, rect(text_left, top + s(10), text_right, top + s(30)), fonts.body_bold, colors.fg,
                    DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS);
                let (detail, color) = match (&item.error, item.ready) {
                    (Some(error), _) => (error.clone(), colors.err),
                    (None, false) => ("Anteprima in preparazione…".to_string(), colors.muted),
                    (None, true) => (item.label.clone(), colors.muted),
                };
                draw_text(dc, &detail, rect(text_left, top + s(30), text_right, top + s(50)), fonts.small, color,
                    DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS);

                paint_arrow(dc, ui, row_button(hwnd, ui, count, index, 2), true, index > 0, hover == Hit::Up(index));
                paint_arrow(dc, ui, row_button(hwnd, ui, count, index, 1), false, index + 1 < count, hover == Hit::Down(index));
                paint_remove(dc, ui, row_button(hwnd, ui, count, index, 0), hover == Hit::Remove(index));
                fill_rect(dc, rect(s(MARGIN), top + s(ROW) - 1, client.right - s(MARGIN), top + s(ROW)), colors.track);
            }
            if let Some(at) = PANEL.with(|p| p.drop_at.get()) {
                let y = list.top + at as i32 * s(ROW) - PANEL.with(|p| p.list_scroll.get());
                fill_rect(dc, rect(s(MARGIN), y - s(1), client.right - s(MARGIN), y + s(2)), colors.primary);
            }
            RestoreDC(dc, saved);
        }

        let pages = pages_rect(hwnd, ui, count);
        let chosen = tray.iter().find(|i| Some(i.id) == selected);
        let label = match chosen {
            None => "Trascina i documenti sulla zona Unisci della barra.".to_string(),
            Some(item) if item.pages > item.thumbs.len() && !item.thumbs.is_empty() => {
                format!("Pagine di {}: prime {} di {}", item.name, item.thumbs.len(), item.pages)
            }
            Some(item) => format!("Pagine di {}", item.name),
        };
        draw_text(dc, &label, rect(s(MARGIN), list.bottom, client.right - s(MARGIN), pages.top), fonts.small, colors.muted,
            DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS);

        if let Some(item) = chosen {
            paint_pages(dc, ui, item, pages);
        }
    });

    let footer_top = client.bottom - s(FOOTER);
    fill_rect(dc, rect(0, footer_top, client.right, client.bottom), colors.bg);
    fill_rect(dc, rect(0, footer_top, client.right, footer_top + 1), colors.border);
    draw_text(dc, "Trascina le righe o usa le frecce per cambiare l'ordine.", rect(s(MARGIN), footer_top + s(6), client.right - s(MARGIN), footer_top + s(40)),
        fonts.small, colors.muted, DT_WORDBREAK | DT_NOPREFIX | DT_CENTER);
    let count = with_tray(|tray| tray.len()).unwrap_or(0);
    let (join, clear) = footer_buttons(hwnd, ui);
    paint_button(dc, ui, join, "Unisci", count >= 2, true, hover == Hit::Join);
    paint_button(dc, ui, clear, "Svuota", count >= 1, false, hover == Hit::Clear);
}

fn kind_badge(label: &str, name: &str) -> String {
    match label {
        "Word" => "DOC".into(),
        "Excel" => "XLS".into(),
        _ => std::path::Path::new(name).extension().map_or_else(|| "?".into(), |e| e.to_string_lossy().to_uppercase().chars().take(4).collect()),
    }
}

fn paint_pages(dc: HDC, ui: &Ui, item: &super::TrayItem, area: RECT) {
    let s = |v| scale(ui, v);
    let colors = ui.palette.get();
    let fonts = ui.fonts.get();
    if item.thumbs.is_empty() {
        let text = if item.error.is_some() {
            "Anteprima non disponibile per questo file."
        } else if !item.ready {
            "Anteprima in preparazione…"
        } else {
            "Word ed Excel si vedono solo dopo l'unione: per mostrarli andrebbero prima convertiti con Office."
        };
        draw_text(dc, text, rect(area.left + s(MARGIN), area.top + s(8), area.right - s(MARGIN), area.bottom), fonts.small, colors.muted,
            DT_WORDBREAK | DT_NOPREFIX);
        return;
    }
    let cell_w = (area.right - area.left - 2 * s(MARGIN) - (COLUMNS - 1) * s(GAP)) / COLUMNS;
    let thumb_h = cell_w * 141 / 100;
    let cell_h = thumb_h + s(20);
    let rows = (item.thumbs.len() as i32 + COLUMNS - 1) / COLUMNS;
    let max_scroll = (rows * (cell_h + s(GAP)) - (area.bottom - area.top)).max(0);
    let scroll = PANEL.with(|p| {
        p.pages_scroll.set(p.pages_scroll.get().clamp(0, max_scroll));
        p.pages_scroll.get()
    });
    unsafe {
        let saved = SaveDC(dc);
        IntersectClipRect(dc, area.left, area.top, area.right, area.bottom);
        for (index, thumb) in item.thumbs.iter().enumerate() {
            let column = index as i32 % COLUMNS;
            let row = index as i32 / COLUMNS;
            let x = area.left + s(MARGIN) + column * (cell_w + s(GAP));
            let y = area.top + row * (cell_h + s(GAP)) - scroll;
            if y + cell_h < area.top || y > area.bottom {
                continue;
            }
            draw_thumb(dc, ui, thumb, rect(x, y, x + cell_w, y + thumb_h));
            draw_text(dc, &format!("{}", index + 1), rect(x, y + thumb_h, x + cell_w, y + cell_h), fonts.small, colors.muted,
                DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX);
        }
        RestoreDC(dc, saved);
    }
}

/// Miniatura adattata al riquadro senza deformarla, con bordo sottile.
pub(super) fn draw_thumb(dc: HDC, ui: &Ui, thumb: &Thumb, area: RECT) {
    let colors = ui.palette.get();
    let (box_w, box_h) = (area.right - area.left, area.bottom - area.top);
    if box_w <= 0 || box_h <= 0 || thumb.width == 0 || thumb.height == 0 {
        return;
    }
    let ratio = (box_w as f64 / thumb.width as f64).min(box_h as f64 / thumb.height as f64);
    let (w, h) = ((thumb.width as f64 * ratio) as i32, (thumb.height as f64 * ratio) as i32);
    let (x, y) = (area.left + (box_w - w) / 2, area.top + (box_h - h) / 2);
    let info = bgra_info(thumb.width as i32, thumb.height as i32);
    unsafe {
        SetStretchBltMode(dc, HALFTONE);
        SetBrushOrgEx(dc, 0, 0, null_mut());
        StretchDIBits(dc, x, y, w, h, 0, 0, thumb.width as i32, thumb.height as i32, thumb.bgra.as_ptr().cast(), &info, DIB_RGB_COLORS, SRCCOPY);
    }
    with_pen(dc, PS_SOLID, 1, colors.border, || outline(dc, x - 1, y - 1, x + w, y + h));
}

pub(super) fn paint_placeholder(dc: HDC, ui: &Ui, area: RECT, badge: &str) {
    let colors = ui.palette.get();
    let fonts = ui.fonts.get();
    fill_rect(dc, area, colors.card);
    with_pen(dc, PS_SOLID, 1, colors.border, || outline(dc, area.left, area.top, area.right - 1, area.bottom - 1));
    draw_text(dc, badge, area, fonts.small, colors.primary, DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX);
}

fn paint_arrow(dc: HDC, ui: &Ui, r: RECT, up: bool, enabled: bool, hover: bool) {
    let s = |v| scale(ui, v);
    let colors = ui.palette.get();
    if hover && enabled {
        fill_round_rect(dc, r, s(5), colors.track);
    }
    let (cx, cy) = ((r.left + r.right) / 2, (r.top + r.bottom) / 2);
    let d = if up { -1 } else { 1 };
    let color = if enabled { colors.fg } else { colors.border };
    with_pen(dc, PS_SOLID, s(1) + 1, color, || {
        line(dc, cx - s(5), cy - d * s(3), cx, cy + d * s(2));
        line(dc, cx, cy + d * s(2), cx + s(5), cy - d * s(3));
    });
}

pub(super) fn paint_remove(dc: HDC, ui: &Ui, r: RECT, hover: bool) {
    let s = |v| scale(ui, v);
    let colors = ui.palette.get();
    if hover {
        fill_round_rect(dc, r, s(5), colors.track);
    }
    let inset = s(8);
    with_pen(dc, PS_SOLID, s(1) + 1, if hover { colors.err } else { colors.muted }, || {
        line(dc, r.left + inset, r.top + inset, r.right - inset, r.bottom - inset);
        line(dc, r.right - inset, r.top + inset, r.left + inset, r.bottom - inset);
    });
}

fn paint_info(dc: HDC, hwnd: HWND, ui: &Ui, client: RECT) {
    let s = |v| scale(ui, v);
    let colors = ui.palette.get();
    let fonts = ui.fonts.get();
    let width = client.right - 2 * s(MARGIN);
    let settings_file = settings::file_path().map_or_else(|| "non disponibile".to_string(), |p| p.display().to_string());

    // Intestazione ad altezza fissa: link_rect conta su queste righe.
    let line_flags = DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_END_ELLIPSIS;
    let mut y = s(HEADER) + s(14);
    draw_text(dc, "Tools PDF", rect(s(MARGIN), y, client.right - s(MARGIN), y + s(20)), fonts.body_bold, colors.fg, DT_SINGLELINE | DT_NOPREFIX);
    y += s(20);
    for text in [format!("Versione {VERSION}, compilata il {BUILD_DATE}."), format!("di {AUTHOR}")] {
        draw_text(dc, &text, rect(s(MARGIN), y, client.right - s(MARGIN), y + s(18)), fonts.small, colors.muted, line_flags);
        y += s(18);
    }
    let link = link_rect(hwnd, ui);
    let hover = PANEL.with(|p| p.hover.get()) == Hit::Link;
    if hover {
        fill_round_rect(dc, rect(link.left - s(4), link.top, link.right, link.bottom), s(4), colors.tint);
    }
    draw_text(dc, &web_address(), link, fonts.small, colors.primary, line_flags);
    y = link.bottom + s(12);

    let paragraphs: [(&str, String); 10] = [
        ("Comprimi", format!("Porta PDF, immagini, Word ed Excel sotto {}.", crate::tools::limit_label())),
        ("Dividi", "Crea la cartella nome_pagine con un PDF per pagina, ognuno nel limite.".into()),
        (
            "Testo",
            "Riconosce il testo delle scansioni con l'OCR di Windows e lo rende selezionabile. Comprimi, Dividi e Unisci fanno lo stesso, salvo OCR \"Solo zona Testo\" nelle impostazioni.".into(),
        ),
        (
            "Word",
            "PDF con testo: Word lo ricostruisce modificabile. Scansioni e immagini: pagine identiche all'originale, con il testo OCR da cercare e copiare.".into(),
        ),
        (
            "Strumenti",
            "Ruota, estrai o elimina pagine; aggiunge numeri, timbro, firma o password e salva tutto in nome_modificato.pdf. Pagine anche in JPG.".into(),
        ),
        (
            "Firme",
            "Cattura una firma dallo schermo o da una foto: diventa nera su sfondo trasparente. Clic per copiarla, trascinala nel documento o usala in Strumenti.".into(),
        ),
        ("Unisci", "Raccogli i file anche da email diverse, ordinali qui e premi Unisci.".into()),
        (
            "Dove salva",
            "Accanto al file originale, oppure sempre in una cartella fissa scelta nelle Impostazioni. Senza cartella fissa, per gli allegati di Outlook e i file nelle cartelle temporanee chiede dove salvare: niente finisce in una cartella temporanea.".into(),
        ),
        (
            "Riservatezza",
            format!(
                "I documenti restano su questo PC: nessun file viene inviato in rete. Unico accesso a internet: il controllo delle versioni nuove su github.com/{} (si spegne con aggiornamenti=mai). Impostazioni in {settings_file}.",
                crate::update::REPO
            ),
        ),
        (
            "Crediti",
            "Scritte dei moduli lette con ocrs (MIT/Apache-2.0). Modelli: Ocrs models (github.com/robertknight/ocrs-models), addestrati sul dataset HierText di Google Research, licenza CC BY-SA 4.0.".into(),
        ),
    ];
    for (heading, body) in paragraphs {
        draw_text(dc, heading, rect(s(MARGIN), y, client.right - s(MARGIN), y + s(20)), fonts.body_bold, colors.fg, DT_SINGLELINE | DT_NOPREFIX);
        y += s(20);
        let height = text_height(dc, &body, width, fonts.small);
        draw_text(dc, &body, rect(s(MARGIN), y, client.right - s(MARGIN), y + height), fonts.small, colors.muted, DT_WORDBREAK | DT_NOPREFIX);
        y += height + s(12);
    }
}

fn paint_settings(dc: HDC, hwnd: HWND, ui: &Ui, client: RECT) {
    let s = |v| scale(ui, v);
    let colors = ui.palette.get();
    let fonts = ui.fonts.get();
    let draft = PANEL.with(|p| p.draft.get());
    let hover = PANEL.with(|p| p.hover.get());
    for (group, (label, choices)) in groups().iter().enumerate() {
        let top = group_top(ui, group);
        draw_text(dc, label, rect(s(MARGIN), top, client.right - s(MARGIN), top + s(22)), fonts.body_bold, colors.fg, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);
        let chosen = choice_of(&draft, group);
        for (choice, text) in choices.iter().enumerate() {
            let r = choice_rect(hwnd, ui, group, choice);
            let inner = rect(r.left + s(2), r.top, r.right - s(2), r.bottom);
            let selected = choice == chosen;
            let fill = if selected { colors.primary } else if hover == Hit::Choice(group, choice) { colors.tint } else { colors.track };
            fill_round_rect(dc, inner, s(6), fill);
            let color = if selected { colors.on_primary } else { colors.fg };
            draw_text(dc, text, inner, fonts.small, color, DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX | DT_END_ELLIPSIS);
        }
    }
    let folder = PANEL.with(|p| p.draft_folder.borrow().clone());
    let (path_box, change) = folder_rects(hwnd, ui);
    let (path_text, path_color) = match &folder {
        Some(path) => (format!("📁 {}", path.display()), if draft.save == Save::Folder { colors.fg } else { colors.muted }),
        None => ("📁 Nessuna cartella fissa scelta".to_string(), colors.muted),
    };
    draw_text(dc, &path_text, path_box, fonts.small, path_color, DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX | DT_PATH_ELLIPSIS);
    paint_button(dc, ui, change, "Cambia…", true, false, hover == Hit::Folder);

    let save = save_rect(hwnd, ui);
    let changed = draft != settings::current() || folder != settings::output_folder();
    paint_button(dc, ui, save, "Salva", changed, true, hover == Hit::Save);
    let note = PANEL.with(|p| p.note.borrow().clone());
    let hint = "Cartella fissa: tutti i file prodotti, anche dagli allegati di Outlook, finiscono lì senza domande. Altrimenti da Outlook e dalle cartelle temporanee chiede sempre dove salvare. Schermo automatico: il bordo esterno del desktop sul lato scelto.";
    // Firme e impostazioni stanno accanto all'exe: su OneDrive seguono l'utente su un altro PC.
    let (cloud, cloud_color) = if settings::on_onedrive() {
        ("✓ Firme e impostazioni sono su OneDrive: ti seguono se cambi computer.", colors.ok)
    } else {
        ("Consiglio: metti ToolsPDF.exe in una cartella tutta sua su OneDrive (es. OneDrive\\ToolsPDF). Firme e impostazioni si salvano accanto all'exe: cambiando computer ritrovi tutto.", colors.muted)
    };
    let mut y = save.bottom + s(10);
    for (text, color) in [(note.as_str(), colors.ok), (hint, colors.muted), (cloud, cloud_color)] {
        if text.is_empty() {
            continue;
        }
        let height = text_height(dc, text, client.right - 2 * s(MARGIN), fonts.small);
        draw_text(dc, text, rect(s(MARGIN), y, client.right - s(MARGIN), y + height), fonts.small, color, DT_WORDBREAK | DT_NOPREFIX);
        y += height + s(8);
    }
}

// ---------- input ----------

fn on_click(hwnd: HWND, hit: Hit) {
    match hit {
        Hit::Tools(tool) => {
            tools_panel::on_click(tool);
            // Il clic puo' aprire sottopannelli o chiudere il documento: si ricalcolano hover e campi.
            PANEL.with(|p| p.hover.set(Hit::Nothing));
            sync_children(hwnd);
        }
        Hit::Signatures(hit) => {
            signatures_panel::on_click(hwnd, hit);
            PANEL.with(|p| p.hover.set(Hit::Nothing));
            sync_children(hwnd);
        }
        Hit::Close => hide(),
        Hit::Up(index) => super::tray_move(index, index - 1),
        Hit::Down(index) => super::tray_move(index, index + 2),
        Hit::Remove(index) => super::tray_remove(index),
        Hit::Join => super::start_join(),
        Hit::Clear => super::clear_tray(),
        Hit::Choice(group, choice) => {
            // Cartella fissa senza cartella: la si sceglie subito; annullando non cambia nulla.
            let missing = PANEL.with(|p| p.draft_folder.borrow().is_none());
            if group == SAVE_GROUP && choice == 1 && missing && !pick_output(hwnd) {
                unsafe { InvalidateRect(hwnd, null(), 0) };
                return;
            }
            PANEL.with(|p| {
                p.draft.set(with_choice(p.draft.get(), group, choice));
                p.note.borrow_mut().clear();
            });
        }
        Hit::Folder => {
            if pick_output(hwnd) {
                PANEL.with(|p| p.draft.set(with_choice(p.draft.get(), SAVE_GROUP, 1)));
            }
        }
        Hit::Save => save_settings(),
        Hit::Link => super::open_web(&web_address()),
        Hit::Row(_) | Hit::Nothing => {}
    }
    unsafe { InvalidateRect(hwnd, null(), 0) };
}

/// Dialogo della cartella fissa; vero se scelta (resta in bozza fino a Salva).
fn pick_output(hwnd: HWND) -> bool {
    let Some(folder) = super::dialogs::pick_output_folder(hwnd) else { return false };
    PANEL.with(|p| {
        *p.draft_folder.borrow_mut() = Some(folder);
        p.note.borrow_mut().clear();
    });
    true
}

fn save_settings() {
    let draft = PANEL.with(|p| p.draft.get());
    let folder = PANEL.with(|p| p.draft_folder.borrow().clone());
    let previous = settings::current();
    if draft == previous && folder == settings::output_folder() {
        return;
    }
    let note = match settings::save(draft, folder) {
        Ok(path) => format!("Impostazioni salvate in {}.", path.display()),
        Err(error) => format!("Attive fino alla chiusura, ma non salvate: {error}"),
    };
    PANEL.with(|p| *p.note.borrow_mut() = note);
    super::apply_settings(previous, draft);
}

fn select_row(index: usize) {
    let id = with_tray(|tray| tray.get(index).map(|item| item.id)).flatten();
    PANEL.with(|p| {
        if p.selected.replace(id) != id {
            p.pages_scroll.set(0);
        }
    });
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match message {
            WM_PAINT => {
                UI.with(|ui| paint(hwnd, ui));
                return 0;
            }
            WM_ERASEBKGND => return 1,
            // Il pannello non ruba lo stato attivo alla finestra su cui si sta lavorando,
            // salvo quando si clicca un campo di testo: per scrivere serve.
            WM_MOUSEACTIVATE => {
                let activate = if tools_panel::edit_under_cursor() || signatures_panel::edit_under_cursor() { MA_ACTIVATE } else { MA_NOACTIVATE };
                return activate as LRESULT;
            }
            WM_CTLCOLOREDIT => return UI.with(|ui| tools_panel::edit_colors(wparam as HDC, ui)),
            WM_COMMAND => {
                if high_word(wparam as isize) as u32 == EN_CHANGE {
                    InvalidateRect(hwnd, null(), 0);
                }
                return 0;
            }
            WM_SIZE => {
                sync_children(hwnd);
            }
            WM_MOUSEMOVE => {
                let (x, y) = (low_word(lparam), high_word(lparam));
                if !PANEL.with(|p| p.tracking.replace(true)) {
                    let mut track = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                    TrackMouseEvent(&mut track);
                }
                UI.with(|ui| {
                    if mode() == Mode::Signatures && signatures_panel::mouse_move(hwnd, ui, x, y) {
                        return;
                    }
                    if let Some((_, start_y)) = PANEL.with(|p| p.press.get()) {
                        if (y - start_y).abs() > DRAG_THRESHOLD || PANEL.with(|p| p.drop_at.get()).is_some() {
                            let count = with_tray(|tray| tray.len()).unwrap_or(0);
                            PANEL.with(|p| p.drop_at.set(Some(insertion_at(hwnd, ui, count, y))));
                            InvalidateRect(hwnd, null(), 0);
                        }
                        return;
                    }
                    let hit = hit_test(hwnd, ui, x, y);
                    if PANEL.with(|p| p.hover.replace(hit)) != hit {
                        InvalidateRect(hwnd, null(), 0);
                    }
                });
                return 0;
            }
            0x02A3 => {
                // WM_MOUSELEAVE
                PANEL.with(|p| {
                    p.tracking.set(false);
                    p.hover.set(Hit::Nothing);
                });
                InvalidateRect(hwnd, null(), 0);
                return 0;
            }
            WM_LBUTTONDOWN => {
                if mode() == Mode::Signatures && UI.with(|ui| signatures_panel::mouse_down(hwnd, ui, low_word(lparam), high_word(lparam))) {
                    InvalidateRect(hwnd, null(), 0);
                    return 0;
                }
                let hit = UI.with(|ui| hit_test(hwnd, ui, low_word(lparam), high_word(lparam)));
                if let Hit::Row(index) = hit {
                    PANEL.with(|p| p.press.set(Some((index, high_word(lparam)))));
                    SetCapture(hwnd);
                }
                return 0;
            }
            WM_LBUTTONUP => {
                if mode() == Mode::Signatures && UI.with(|ui| signatures_panel::mouse_up(hwnd, ui)) {
                    InvalidateRect(hwnd, null(), 0);
                    return 0;
                }
                let pressed = PANEL.with(|p| p.press.replace(None));
                let dropped = PANEL.with(|p| p.drop_at.replace(None));
                if pressed.is_some() {
                    ReleaseCapture();
                }
                match (pressed, dropped) {
                    (Some((from, _)), Some(to)) => {
                        super::tray_move(from, to);
                        select_row(if to > from { to - 1 } else { to });
                    }
                    (Some((index, _)), None) => select_row(index),
                    _ => {
                        let hit = UI.with(|ui| hit_test(hwnd, ui, low_word(lparam), high_word(lparam)));
                        on_click(hwnd, hit);
                    }
                }
                InvalidateRect(hwnd, null(), 0);
                return 0;
            }
            WM_RBUTTONUP => {
                if mode() == Mode::Signatures {
                    UI.with(|ui| signatures_panel::context_menu(hwnd, ui, low_word(lparam), high_word(lparam)));
                    InvalidateRect(hwnd, null(), 0);
                }
                return 0;
            }
            WM_MOUSEWHEEL => {
                let delta = high_word(wparam as isize);
                let mut point = POINT { x: low_word(lparam), y: high_word(lparam) };
                ScreenToClient(hwnd, &mut point);
                UI.with(|ui| {
                    let count = with_tray(|tray| tray.len()).unwrap_or(0);
                    let step = delta * scale(ui, ROW) / 120;
                    PANEL.with(|p| {
                        if p.mode.get() == Mode::Tools {
                            tools_panel::scroll(step);
                        } else if p.mode.get() == Mode::Signatures {
                            if !signatures_panel::zoom(hwnd, ui, delta, point) {
                                signatures_panel::scroll(step);
                            }
                        } else if list_rect(hwnd, ui, count).contains(point.x, point.y) {
                            p.list_scroll.set(p.list_scroll.get() - step);
                        } else {
                            p.pages_scroll.set(p.pages_scroll.get() - step);
                        }
                    });
                });
                InvalidateRect(hwnd, null(), 0);
                return 0;
            }
            WM_CLOSE => {
                hide();
                return 0;
            }
            _ => {}
        }
        DefWindowProcW(hwnd, message, wparam, lparam)
    }
}

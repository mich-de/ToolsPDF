//! Barra laterale in Win32 e GDI: appbar, zone di rilascio, coda di lavoro.
//! Accanto alla barra si apre il pannello (panel.rs) per Unisci, Info e Impostazioni.

mod capture;
mod clipboard;
mod drag_source;
mod dialogs;
mod drop_target;
mod monitors;
mod paint;
mod panel;
mod photo_panel;
mod scanner_panel;
mod signatures_panel;
mod tools_panel;
mod win;

use std::cell::Cell;
use std::ffi::OsString;
use std::mem::size_of;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread;

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{InvalidateRect, ScreenToClient};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows_sys::Win32::UI::Shell::{
    SHAppBarMessage, ABE_LEFT, ABE_RIGHT, ABM_ACTIVATE, ABM_NEW, ABM_QUERYPOS, ABM_REMOVE, ABM_SETPOS, ABM_WINDOWPOSCHANGED, ABN_FULLSCREENAPP, ABN_POSCHANGED,
    APPBARDATA,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::settings::{self, Conversion, Settings, Side};
use crate::update;
use crate::tools::edit::{self, Plan};
use crate::tools::preview::{self, Thumb};
use crate::tools::scanner::{self, ScannerInfo};
use crate::tools::{self, location, Callbacks, Destination, Error as ToolError, Kind, Loaded, Operation, Outcome, Source};
use paint::{FontSet, Palette, ROW_HEIGHT};
use tools_panel::{DocState, ToolsDoc};
use win::{rect, wide, Cell2, RectExt};

const LOGICAL_WIDTH: i32 = 220;
pub(crate) const APP_TITLE: &str = "Tools PDF";
pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");
pub(crate) const BUILD_DATE: &str = env!("TOOLSPDF_BUILD_DATE");
pub(crate) const AUTHOR: &str = "Michele De Angelis";
const WM_APPBAR_NOTIFY: u32 = WM_APP + 1;
const WM_REPAINT: u32 = WM_APP + 2;
/// wparam 0: c'e' una versione nuova da proporre; 1: e' installata, si propone il riavvio.
const WM_UPDATE: u32 = WM_APP + 3;
/// Primo controllo poco dopo l'avvio, poi una volta al giorno.
const UPDATE_FIRST_CHECK: std::time::Duration = std::time::Duration::from_secs(15);
const UPDATE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(24 * 3600);
const WM_DPICHANGED_MSG: u32 = 0x02E0;
const WM_MOUSELEAVE_MSG: u32 = 0x02A3;
const CANCELLED_TEXT: &str = "Annullato: nessun file creato";
/// Miniature per file: oltre, il pannello dice quante pagine non mostra.
const MAX_PREVIEW_PAGES: usize = 40;
const PREVIEW_WIDTH_PX: u32 = 160;
/// Strumenti mostra tutte le pagine da scegliere: miniature piu' piccole, fino a 200.
const MAX_TOOLS_THUMBS: usize = 200;
const TOOLS_THUMB_PX: u32 = 100;

/// Indice delle zone di rilascio, dall'alto.
pub(crate) const ZONE_COMPRESS: usize = 0;
pub(crate) const ZONE_SPLIT: usize = 1;
pub(crate) const ZONE_TEXT: usize = 2;
pub(crate) const ZONE_WORD: usize = 3;
pub(crate) const ZONE_TOOLS: usize = 4;
pub(crate) const ZONE_SIGNATURES: usize = 5;
pub(crate) const ZONE_PHOTO: usize = 6;
pub(crate) const ZONE_JOIN: usize = 7;
pub(crate) const ZONE_COUNT: usize = 8;

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum RowState {
    Queued,
    Working,
    Done,
    Failed,
    Cancelled,
}

/// File nell'elenco; condiviso tra thread UI e worker dietro il Mutex.
pub(crate) struct Row {
    pub name: String,
    pub pct: f64,
    pub status: String,
    pub state: RowState,
    pub output: Option<(PathBuf, bool)>,
}

/// File nel vassoio di Unisci, con le miniature preparate in sottofondo.
pub(crate) struct TrayItem {
    pub id: u64,
    pub name: String,
    pub source: Source,
    pub label: String,
    pub pages: usize,
    pub thumbs: Vec<Thumb>,
    pub ready: bool,
    pub error: Option<String>,
}

/// Lavori della zona Strumenti che scrivono un file.
pub(crate) enum ToolsTask {
    /// Tutte le modifiche in nome_modificato.pdf.
    Save,
    /// Le pagine indicate (indici originali) in nome_estratto.pdf.
    Extract(Vec<usize>),
    /// Cartella nome_immagini con un JPG per pagina.
    Jpg,
}

enum Job {
    One { row: usize, op: Operation, source: Source },
    Join { row: usize, sources: Vec<Source> },
    Tools { row: usize, task: ToolsTask, name: String, path: Option<PathBuf>, data: Vec<u8>, plan: Plan },
}

/// Lavori del thread delle anteprime.
enum PreviewJob {
    Tray(u64, Source),
    /// Apre un documento nella zona Strumenti.
    Tools(u64, Source),
    /// Sblocca il PDF protetto aperto in Strumenti.
    Unlock(u64, String),
}

struct Shared {
    rows: Mutex<Vec<Row>>,
    tray: Mutex<Vec<TrayItem>>,
    tools: Mutex<Option<ToolsDoc>>,
    hwnd: AtomicIsize,
    jobs: Sender<Job>,
    previews: Sender<PreviewJob>,
}

/// Stato dell'aggiornamento mostrato nel banner della barra e in Info.
#[derive(Clone)]
pub(crate) enum UpdateStatus {
    Idle,
    Checking,
    UpToDate,
    Available(update::Release),
    Downloading(String),
    Ready(String),
    Error(String),
}

static SHARED: OnceLock<Shared> = OnceLock::new();
static NEXT_TRAY_ID: AtomicU64 = AtomicU64::new(1);
/// Versione trovata su GitHub, in attesa della risposta dell'utente.
static UPDATE_OFFER: Mutex<Option<update::Release>> = Mutex::new(None);
/// Stato persistente dell'aggiornamento (per il banner nella barra e il tasto in Info).
static UPDATE_STATUS: Mutex<UpdateStatus> = Mutex::new(UpdateStatus::Idle);
/// Versioni gia' proposte con popup in questa sessione: non si richiede ogni giorno la stessa.
static UPDATE_ASKED: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// Dopo la chiusura si riavvia l'exe aggiornato.
static RESTART: AtomicBool = AtomicBool::new(false);
/// Scanner WIA rilevati sul PC in sottofondo.
static SCANNERS: Mutex<Vec<ScannerInfo>> = Mutex::new(Vec::new());
static HAS_SCANNER: AtomicBool = AtomicBool::new(false);

/// Elemento sotto il mouse nella barra.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Hover {
    Nothing,
    Close,
    Info,
    Settings,
    Scanner,
    UpdateBanner,
    Join,
    Clear,
    JoinZone,
    ToolsZone,
    SignaturesZone,
    PhotoZone,
    Folder(usize),
}

/// Stato usato solo dal thread UI. Sono Cell, non RefCell: le API Win32 possono
/// rientrare in wnd_proc mentre una chiamata e' in corso.
pub(crate) struct Ui {
    pub dpi: Cell2<u32>,
    pub taskbar_msg: Cell2<u32>,
    pub registered: Cell2<bool>,
    pub hover: Cell2<Hover>,
    pub tracking: Cell2<bool>,
    pub scroll: Cell2<i32>,
    pub palette: Cell2<Palette>,
    pub fonts: Cell2<FontSet>,
    /// Zona sotto il cursore durante un trascinamento.
    pub drag_zone: Cell2<Option<usize>>,
}

thread_local! {
    pub(crate) static UI: Ui = Ui {
        dpi: Cell2::new(96),
        taskbar_msg: Cell2::new(0),
        registered: Cell2::new(false),
        hover: Cell2::new(Hover::Nothing),
        tracking: Cell2::new(false),
        scroll: Cell2::new(0),
        palette: Cell2::new(paint::palette_for(settings::Theme::System)),
        fonts: Cell2::new(FontSet::default()),
        drag_zone: Cell2::new(None),
    };
}

pub(crate) fn scale(ui: &Ui, value: i32) -> i32 {
    value * ui.dpi.get() as i32 / 96
}

/// Mostra la barra e resta in esecuzione fino alla chiusura.
pub fn run(files: Vec<OsString>) {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let mutex_name = wide("Local\\ToolsPDF-SingleInstance");
        let mutex = CreateMutexW(null(), 0, mutex_name.as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            win::message_box(null_mut(), "La barra è già aperta sul bordo dello schermo.", APP_TITLE, MB_ICONWARNING | MB_TOPMOST);
            return;
        }

        let updated = update::cleanup();
        let settings = settings::load();
        let show_news = crate::news::should_show(updated, settings::had_file(), settings::seen_version().as_deref(), VERSION);
        if !show_news && !settings::had_file() {
            // Al primo uso segna la versione in memoria: se poi l'utente salva le impostazioni,
            // al secondo avvio non comparira' il popup novita' senza alcun aggiornamento.
            settings::mark_seen_in_memory(VERSION);
        }
        tools::set_limit_mb(settings.limit_mb);
        tools::set_always_ocr(settings.always_ocr);
        UI.with(|ui| ui.palette.set(paint::palette_for(settings.theme)));

        let (jobs, receiver) = channel::<Job>();
        let (previews, preview_receiver) = channel::<PreviewJob>();
        let _ = SHARED.set(Shared {
            rows: Mutex::new(Vec::new()),
            tray: Mutex::new(Vec::new()),
            tools: Mutex::new(None),
            hwnd: AtomicIsize::new(0),
            jobs,
            previews,
        });
        thread::spawn(move || {
            dialogs::init_worker_thread();
            for job in receiver {
                match job {
                    Job::One { row, op, source } => process_one(row, op, source),
                    Job::Join { row, sources } => process_join(row, sources),
                    Job::Tools { row, task, name, path, data, plan } => process_tools(row, task, &name, path, &data, &plan),
                }
            }
        });
        thread::spawn(move || {
            for job in preview_receiver {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || match job {
                    PreviewJob::Tray(id, source) => make_preview(id, source),
                    PreviewJob::Tools(id, source) => load_tools_doc(id, source),
                    PreviewJob::Unlock(id, password) => unlock_tools_doc(id, &password),
                }));
            }
        });

        let instance = GetModuleHandleW(null());
        let class_name = wide("ToolsPdfSidebar");
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
            fatal("RegisterClassEx non riuscita");
            return;
        }

        let taskbar_name = wide("TaskbarCreated");
        let taskbar_msg = RegisterWindowMessageW(taskbar_name.as_ptr());
        UI.with(|ui| ui.taskbar_msg.set(taskbar_msg));

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
            fatal("CreateWindowEx non riuscita");
            return;
        }
        if let Some(shared) = SHARED.get() {
            shared.hwnd.store(hwnd as isize, Ordering::SeqCst);
        }
        update_dpi(hwnd);
        if settings.reserve {
            register_appbar(hwnd);
        } else {
            position_appbar(hwnd);
        }
        if let Err(text) = drop_target::register(hwnd, drop_target::Target::Bar) {
            fatal(&text);
            remove_appbar(hwnd);
            return;
        }
        if let Err(text) = panel::create(instance) {
            fatal(&text);
            remove_appbar(hwnd);
            return;
        }
        ShowWindow(hwnd, SW_SHOWNA);
        refresh_scanners();
        // Da riga di comando o trascinando sull'exe si comprime.
        let sources = files.into_iter().map(|f| Source::from_path(PathBuf::from(f))).collect();
        on_drop(ZONE_COMPRESS, sources, Vec::new());
        if settings.check_updates {
            start_update_checks();
        }
        // Novita': dopo che la barra e' comparsa, dal ciclo dei messaggi.
        if show_news {
            PostMessageW(hwnd, WM_UPDATE, WPARAM_NEWS, 0);
        }

        let mut message: MSG = std::mem::zeroed();
        while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
            if tools_panel::handle_key(&message) || signatures_panel::handle_key(&message) {
                continue;
            }
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        remove_appbar(hwnd);
        CloseHandle(mutex);
        // Il mutex e' gia' chiuso: la nuova istanza non trova la barra "gia' aperta".
        if RESTART.load(Ordering::SeqCst) {
            if let Ok(exe) = std::env::current_exe() {
                let _ = Command::new(exe).spawn();
            }
        }
    }
}

/// Vero se sul PC e' stato rilevato almeno uno scanner WIA.
pub(crate) fn has_scanner() -> bool {
    HAS_SCANNER.load(Ordering::Relaxed)
}

pub(crate) fn scanners_list() -> Vec<ScannerInfo> {
    SCANNERS.lock().map(|s| s.clone()).unwrap_or_default()
}

/// Cerca gli scanner di sistema in sottofondo: non blocca la barra e mostra l'icona solo se esistono.
pub(crate) fn refresh_scanners() {
    thread::spawn(|| {
        let found = scanner::list_scanners();
        let count = found.len();
        HAS_SCANNER.store(count > 0, Ordering::Relaxed);
        if let Ok(mut list) = SCANNERS.lock() {
            *list = found;
        }
        scanner_panel::on_scanners_refreshed(count);
        request_repaint();
    });
}

/// Invia un PDF appena scansionato in memoria a Testo (OCR) o Comprimi.
pub(crate) fn send_scanned_pdf(op: Operation, pdf: Vec<u8>) {
    let Some(shared) = SHARED.get() else { return };
    let name = scanner::default_name();
    let source = Source::from_memory(&name, pdf);
    if let Some(row) = new_row(name) {
        let _ = shared.jobs.send(Job::One { row, op, source });
    }
    request_repaint();
}

pub(crate) fn update_status() -> UpdateStatus {
    UPDATE_STATUS.lock().map(|s| s.clone()).unwrap_or(UpdateStatus::Idle)
}

fn set_update_status(status: UpdateStatus) {
    if let Ok(mut current) = UPDATE_STATUS.lock() {
        *current = status;
    }
    request_repaint();
}

/// Controlla GitHub in sottofondo; se trova una versione nuova mostra il banner e propone il download.
fn start_update_checks() {
    thread::spawn(|| {
        thread::sleep(UPDATE_FIRST_CHECK);
        loop {
            if let Ok(release) = update::latest() {
                if update::is_newer(&release.version, VERSION) {
                    let already_ready = matches!(update_status(), UpdateStatus::Ready(_) | UpdateStatus::Downloading(_));
                    if !already_ready {
                        set_update_status(UpdateStatus::Available(release.clone()));
                    }
                    let fresh = UPDATE_ASKED.lock().is_ok_and(|asked| !asked.contains(&release.version));
                    if fresh && !already_ready {
                        if let Ok(mut offer) = UPDATE_OFFER.lock() {
                            *offer = Some(release);
                        }
                        let hwnd = main_hwnd();
                        if !hwnd.is_null() {
                            unsafe { PostMessageW(hwnd, WM_UPDATE, 0, 0) };
                        }
                    }
                }
            }
            thread::sleep(UPDATE_INTERVAL);
        }
    });
}

/// Controllo manuale avviato dal pulsante "Cerca aggiornamenti" in Info.
pub(crate) fn check_updates_now() {
    if matches!(update_status(), UpdateStatus::Checking | UpdateStatus::Downloading(_)) {
        return;
    }
    set_update_status(UpdateStatus::Checking);
    thread::spawn(|| match update::latest() {
        Ok(release) if update::is_newer(&release.version, VERSION) => {
            set_update_status(UpdateStatus::Available(release));
        }
        Ok(_) => set_update_status(UpdateStatus::UpToDate),
        Err(err) => set_update_status(UpdateStatus::Error(err)),
    });
}

/// Avvia il download della release gia' rilevata (dal banner, da Info o dal popup).
pub(crate) fn start_update_download(release: update::Release) {
    if matches!(update_status(), UpdateStatus::Downloading(_)) {
        return;
    }
    let version = release.version.clone();
    set_update_status(UpdateStatus::Downloading(version.clone()));
    let Some(row) = new_row(format!("Aggiornamento a Tools PDF {version}")) else { return };
    UI.with(|ui| ui.scroll.set(0));
    thread::spawn(move || {
        update_row(row, |r| {
            r.state = RowState::Working;
            r.status = "Download da GitHub…".into();
        });
        let progress = |pct: f64| update_row(row, |r| r.pct = pct * 0.95);
        let result = update::install(&release, &progress);
        update_row(row, |r| {
            r.pct = 1.0;
            match &result {
                Ok(()) => {
                    r.state = RowState::Done;
                    r.status = "Pronta: parte al prossimo avvio della barra".into();
                }
                Err(text) => {
                    r.state = RowState::Failed;
                    r.status = text.clone();
                }
            }
        });
        match &result {
            Ok(()) => {
                set_update_status(UpdateStatus::Ready(version));
                let hwnd = main_hwnd();
                if !hwnd.is_null() {
                    unsafe { PostMessageW(hwnd, WM_UPDATE, 1, 0) };
                }
            }
            Err(text) => {
                set_update_status(UpdateStatus::Error(text.clone()));
            }
        }
    });
}

/// Azione sul banner della barra o sul pulsante aggiornamento in Info.
pub(crate) fn handle_update_action(hwnd: HWND) {
    match update_status() {
        UpdateStatus::Available(release) => start_update_download(release),
        UpdateStatus::Ready(_) => offer_restart(hwnd),
        UpdateStatus::Checking | UpdateStatus::Downloading(_) => {}
        UpdateStatus::Idle | UpdateStatus::UpToDate | UpdateStatus::Error(_) => check_updates_now(),
    }
}

/// Chiede se scaricare la versione nuova; il download si vede come una riga della barra.
/// WM_UPDATE con questo wparam: novita' della versione appena installata.
const WPARAM_NEWS: usize = 2;

/// Popup delle novita', una volta sola per versione.
fn show_news(hwnd: HWND) {
    let Some(text) = crate::news::of(VERSION) else { return };
    let title = format!("Novità di Tools PDF {VERSION}");
    let body = format!("{text}\n\nTutto è spiegato anche in Info (tasto i in alto nella barra).");
    win::message_box(hwnd, &body, &title, MB_OK | MB_ICONINFORMATION | MB_TOPMOST);
    let _ = settings::save_seen_version(VERSION);
}

fn offer_update(hwnd: HWND) {
    let Some(release) = UPDATE_OFFER.lock().ok().and_then(|mut offer| offer.take()) else { return };
    if let Ok(mut asked) = UPDATE_ASKED.lock() {
        asked.push(release.version.clone());
    }
    let text = format!(
        "È disponibile Tools PDF {} (in uso: {VERSION}).\n\nScaricarla adesso da GitHub? La barra in uso continua a funzionare; la nuova versione parte al prossimo avvio.",
        release.version
    );
    if win::message_box(hwnd, &text, APP_TITLE, MB_YESNO | MB_ICONQUESTION | MB_TOPMOST) != IDYES {
        return;
    }
    start_update_download(release);
}

fn offer_restart(hwnd: HWND) {
    let text = "Aggiornamento installato. Riavviare adesso la barra con la nuova versione?";
    if win::message_box(hwnd, text, APP_TITLE, MB_YESNO | MB_ICONQUESTION | MB_TOPMOST) == IDYES {
        RESTART.store(true, Ordering::SeqCst);
        unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
    }
}

fn fatal(text: &str) {
    win::message_box(null_mut(), &format!("Avvio non riuscito: {text}"), APP_TITLE, MB_ICONERROR | MB_TOPMOST);
}

fn update_dpi(hwnd: HWND) {
    UI.with(|ui| {
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        ui.dpi.set(if dpi == 0 { 96 } else { dpi });
        let mut fonts = ui.fonts.get();
        fonts.rebuild(ui);
        ui.fonts.set(fonts);
    });
}

fn appbar_data(hwnd: HWND) -> APPBARDATA {
    APPBARDATA {
        cbSize: size_of::<APPBARDATA>() as u32,
        hWnd: hwnd,
        uCallbackMessage: 0,
        uEdge: if settings::current().side == Side::Left { ABE_LEFT } else { ABE_RIGHT },
        rc: rect(0, 0, 0, 0),
        lParam: 0,
    }
}

fn register_appbar(hwnd: HWND) {
    if !settings::current().reserve {
        position_appbar(hwnd);
        return;
    }
    let mut data = appbar_data(hwnd);
    data.uCallbackMessage = WM_APPBAR_NOTIFY;
    let ok = unsafe { SHAppBarMessage(ABM_NEW, &mut data) } != 0;
    UI.with(|ui| ui.registered.set(ok));
    position_appbar(hwnd);
}

fn remove_appbar(hwnd: HWND) {
    let registered = UI.with(|ui| ui.registered.replace(false));
    if registered {
        let mut data = appbar_data(hwnd);
        unsafe { SHAppBarMessage(ABM_REMOVE, &mut data) };
    }
}

/// Posiziona la barra sul lato scelto dello schermo scelto: se `reserve` e' attivo riserva
/// la striscia come appbar; altrimenti sta "sopra le finestre" sull'area di lavoro senza
/// mai spostare le icone del desktop ne' coprire la taskbar.
fn position_appbar(hwnd: HWND) {
    let current = settings::current();
    let left_side = current.side == Side::Left;
    let Some(screen) = monitors::target(current.side, current.monitor) else { return };
    // La larghezza segue i DPI dello schermo di destinazione, non di quello attuale.
    let width = LOGICAL_WIDTH * screen.dpi as i32 / 96;
    let registered = UI.with(|ui| ui.registered.get());
    unsafe {
        if registered {
            let m = screen.rect;
            let mut data = appbar_data(hwnd);
            data.rc = if left_side { rect(m.left, m.top, m.left + width, m.bottom) } else { rect(m.right - width, m.top, m.right, m.bottom) };
            SHAppBarMessage(ABM_QUERYPOS, &mut data);
            if left_side {
                data.rc.right = data.rc.left + width;
            } else {
                data.rc.left = data.rc.right - width;
            }
            SHAppBarMessage(ABM_SETPOS, &mut data);
            SetWindowPos(
                hwnd,
                null_mut(),
                data.rc.left,
                data.rc.top,
                data.rc.right - data.rc.left,
                data.rc.bottom - data.rc.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        } else {
            let w = screen.work;
            let x = if left_side { w.left } else { w.right - width };
            SetWindowPos(hwnd, HWND_TOPMOST, x, w.top, width, w.bottom - w.top, SWP_NOACTIVATE);
        }
    }
    panel::reposition();
}

/// Rende attive le impostazioni appena salvate. Gira sul thread UI.
pub(crate) fn apply_settings(previous: Settings, current: Settings) {
    tools::set_limit_mb(current.limit_mb);
    tools::set_always_ocr(current.always_ocr);
    UI.with(|ui| ui.palette.set(paint::palette_for(current.theme)));
    let hwnd = main_hwnd();
    if !hwnd.is_null() {
        if previous.reserve != current.reserve {
            if current.reserve {
                register_appbar(hwnd);
            } else {
                remove_appbar(hwnd);
                position_appbar(hwnd);
            }
        } else if previous.side != current.side || previous.monitor != current.monitor {
            position_appbar(hwnd);
        }
    }
    request_repaint();
}

pub(crate) fn main_hwnd() -> HWND {
    SHARED.get().map_or(null_mut(), |s| s.hwnd.load(Ordering::SeqCst) as HWND)
}

fn request_repaint() {
    let hwnd = main_hwnd();
    if !hwnd.is_null() {
        unsafe { PostMessageW(hwnd, WM_REPAINT, 0, 0) };
    }
}

pub(crate) fn with_rows<T>(action: impl FnOnce(&mut Vec<Row>) -> T) -> Option<T> {
    let shared = SHARED.get()?;
    let mut rows = shared.rows.lock().unwrap_or_else(|e| e.into_inner());
    Some(action(&mut rows))
}

pub(crate) fn with_tray<T>(action: impl FnOnce(&mut Vec<TrayItem>) -> T) -> Option<T> {
    let shared = SHARED.get()?;
    let mut tray = shared.tray.lock().unwrap_or_else(|e| e.into_inner());
    Some(action(&mut tray))
}

pub(crate) fn with_tools<T>(action: impl FnOnce(&mut Option<ToolsDoc>) -> T) -> Option<T> {
    let shared = SHARED.get()?;
    let mut doc = shared.tools.lock().unwrap_or_else(|e| e.into_inner());
    Some(action(&mut doc))
}

pub(crate) fn tray_len() -> usize {
    with_tray(|tray| tray.len()).unwrap_or(0)
}

fn new_row(name: String) -> Option<usize> {
    with_rows(|rows| {
        rows.push(Row { name, pct: 0.0, status: "In coda".into(), state: RowState::Queued, output: None });
        rows.len() - 1
    })
}

/// Copia della sorgente per il thread delle anteprime.
fn source_copy(source: &Source) -> Source {
    match (&source.path, &source.bytes) {
        (Some(path), _) => Source::from_path(path.clone()),
        (None, Some(bytes)) => Source::from_memory(&source.name, bytes.clone()),
        (None, None) => Source::from_memory(&source.name, Vec::new()),
    }
}

/// Riceve i file rilasciati su una zona. Gira sul thread UI.
pub(crate) fn on_drop(zone: usize, sources: Vec<Source>, errors: Vec<(String, String)>) {
    let Some(shared) = SHARED.get() else { return };
    for (name, text) in errors {
        if let Some(row) = new_row(name) {
            update_row(row, |r| {
                r.state = RowState::Failed;
                r.status = text;
            });
        }
    }
    if zone == ZONE_JOIN {
        let mut first_new = None;
        for source in sources {
            let id = NEXT_TRAY_ID.fetch_add(1, Ordering::Relaxed);
            first_new.get_or_insert(id);
            let copy = source_copy(&source);
            let item = TrayItem {
                id,
                name: source.name.clone(),
                source,
                label: String::new(),
                pages: 0,
                thumbs: Vec::new(),
                ready: false,
                error: None,
            };
            with_tray(|tray| tray.push(item));
            let _ = shared.previews.send(PreviewJob::Tray(id, copy));
        }
        if let Some(id) = first_new {
            panel::show_join(Some(id));
        }
    } else if zone == ZONE_TOOLS {
        let mut sources = sources.into_iter();
        if let Some(source) = sources.next() {
            let id = NEXT_TRAY_ID.fetch_add(1, Ordering::Relaxed);
            tools_panel::reset_state();
            with_tools(|doc| *doc = Some(ToolsDoc::loading(id, source.name.clone())));
            let _ = shared.previews.send(PreviewJob::Tools(id, source));
        }
        for extra in sources {
            if let Some(row) = new_row(extra.name) {
                update_row(row, |r| {
                    r.state = RowState::Failed;
                    r.status = "Strumenti apre un file alla volta: trascinalo dopo".into();
                });
            }
        }
        panel::show_tools();
    } else if zone == ZONE_SIGNATURES {
        let mut sources = sources.into_iter();
        if let Some(source) = sources.next() {
            signatures_panel::load_dropped(source);
        }
        for extra in sources {
            if let Some(row) = new_row(extra.name) {
                update_row(row, |r| {
                    r.state = RowState::Failed;
                    r.status = "Firme prende un'immagine alla volta: trascinala dopo".into();
                });
            }
        }
        panel::show_signatures();
    } else if zone == ZONE_PHOTO {
        let mut sources = sources.into_iter();
        if let Some(source) = sources.next() {
            photo_panel::load_dropped(source);
        }
        for extra in sources {
            if let Some(row) = new_row(extra.name) {
                update_row(row, |r| {
                    r.state = RowState::Failed;
                    r.status = "Foto tessera prende una foto alla volta: trascinala dopo".into();
                });
            }
        }
        panel::show_photo();
    } else {
        let op = match zone {
            ZONE_SPLIT => Operation::Split,
            ZONE_TEXT => Operation::Text,
            ZONE_WORD => Operation::Word,
            _ => Operation::Compress,
        };
        for source in sources {
            if let Some(row) = new_row(source.name.clone()) {
                let _ = shared.jobs.send(Job::One { row, op, source });
            }
        }
    }
    UI.with(|ui| ui.scroll.set(0));
    request_repaint();
}

/// Prepara le miniature di un file del vassoio. Gira nel thread delle anteprime.
fn make_preview(id: u64, source: Source) {
    let result = tools::load(source).and_then(|file| preview::preview(&file, MAX_PREVIEW_PAGES, PREVIEW_WIDTH_PX));
    with_tray(|tray| {
        if let Some(item) = tray.iter_mut().find(|item| item.id == id) {
            item.ready = true;
            match result {
                Ok(shown) => {
                    item.label = shown.label;
                    item.pages = shown.pages;
                    item.thumbs = shown.thumbs;
                }
                Err(ToolError::Message(text)) => item.error = Some(text),
                Err(ToolError::Cancelled) => item.error = Some("anteprima non disponibile".into()),
            }
        }
    });
    request_repaint();
}

fn error_text(error: ToolError) -> String {
    match error {
        ToolError::Message(text) => text,
        ToolError::Cancelled => "operazione annullata".into(),
    }
}

/// Apre un documento in Strumenti. Gira nel thread delle anteprime.
fn load_tools_doc(id: u64, source: Source) {
    let file = match tools::load(source) {
        Ok(file) => file,
        Err(error) => return set_tools_state(id, DocState::Failed(error_text(error))),
    };
    let protected = file.kind == Kind::Pdf && edit::is_protected(&file.data);
    let path = file.path.clone();
    with_tools(|doc| {
        if let Some(doc) = doc.as_mut().filter(|d| d.id == id) {
            doc.path = path;
            doc.was_protected = protected;
        }
    });
    if protected {
        with_tools(|doc| {
            if let Some(doc) = doc.as_mut().filter(|d| d.id == id) {
                doc.original = file.data;
                doc.state = DocState::Locked(None);
            }
        });
        request_repaint();
        return;
    }
    finish_tools_doc(id, edit::prepare(&file, &|_, _| {}));
}

fn unlock_tools_doc(id: u64, password: &str) {
    let Some(original) = with_tools(|doc| doc.as_ref().filter(|d| d.id == id).map(|d| d.original.clone())).flatten() else { return };
    match edit::unlock(&original, password) {
        Ok(data) => finish_tools_doc(id, Ok(data)),
        Err(error) => set_tools_state(id, DocState::Locked(Some(error_text(error)))),
    }
}

/// PDF pronto: pagine e miniature, poi il pannello mostra i comandi.
fn finish_tools_doc(id: u64, data: Result<Vec<u8>, ToolError>) {
    let ready = data.and_then(|data| {
        let pages = edit::page_count(&data)?;
        let file = Loaded { name: String::new(), path: None, data, kind: Kind::Pdf };
        let shown = preview::preview(&file, MAX_TOOLS_THUMBS, TOOLS_THUMB_PX)?;
        Ok((file.data, pages, shown.thumbs))
    });
    match ready {
        Ok((data, pages, thumbs)) => {
            with_tools(|doc| {
                if let Some(doc) = doc.as_mut().filter(|d| d.id == id) {
                    doc.data = data;
                    doc.pages = pages;
                    doc.thumbs = thumbs;
                    doc.plan = Plan::new(pages);
                    doc.selected = vec![false; pages];
                    doc.state = DocState::Ready;
                }
            });
            request_repaint();
        }
        Err(error) => set_tools_state(id, DocState::Failed(error_text(error))),
    }
}

fn set_tools_state(id: u64, state: DocState) {
    with_tools(|doc| {
        if let Some(doc) = doc.as_mut().filter(|d| d.id == id) {
            doc.state = state;
        }
    });
    request_repaint();
}

pub(crate) fn request_unlock(id: u64, password: String) {
    if let Some(shared) = SHARED.get() {
        let _ = shared.previews.send(PreviewJob::Unlock(id, password));
    }
    request_repaint();
}

/// Mette in coda un salvataggio della zona Strumenti; l'avanzamento si vede nella barra.
pub(crate) fn start_tools(task: ToolsTask, name: String, path: Option<PathBuf>, data: Vec<u8>, plan: Plan) {
    let Some(shared) = SHARED.get() else { return };
    let label = match &task {
        ToolsTask::Save => "modifiche",
        ToolsTask::Extract(_) => "pagine estratte",
        ToolsTask::Jpg => "pagine in JPG",
    };
    if let Some(row) = new_row(format!("{name} · {label}")) {
        let _ = shared.jobs.send(Job::Tools { row, task, name, path, data, plan });
    }
    UI.with(|ui| ui.scroll.set(0));
    request_repaint();
}

/// Sposta un elemento: to e' la posizione d'inserimento, contata prima dello spostamento.
pub(crate) fn move_item<T>(items: &mut Vec<T>, from: usize, to: usize) {
    if from >= items.len() {
        return;
    }
    let target = if to > from { to - 1 } else { to }.min(items.len() - 1);
    let item = items.remove(from);
    items.insert(target, item);
}

pub(crate) fn tray_move(from: usize, to: usize) {
    with_tray(|tray| move_item(tray, from, to));
    request_repaint();
}

pub(crate) fn tray_remove(index: usize) {
    with_tray(|tray| {
        if index < tray.len() {
            tray.remove(index);
        }
    });
    if tray_len() == 0 {
        panel::hide();
    }
    request_repaint();
}

pub(crate) fn start_join() {
    let Some(shared) = SHARED.get() else { return };
    let sources: Vec<Source> = with_tray(|tray| {
        if tray.len() < 2 {
            return Vec::new();
        }
        tray.drain(..).map(|item| item.source).collect()
    })
    .unwrap_or_default();
    if sources.len() < 2 {
        return;
    }
    panel::hide();
    if let Some(row) = new_row(format!("Unione di {} file", sources.len())) {
        let _ = shared.jobs.send(Job::Join { row, sources });
    }
    UI.with(|ui| ui.scroll.set(0));
    request_repaint();
}

pub(crate) fn clear_tray() {
    with_tray(|tray| tray.clear());
    panel::hide();
    request_repaint();
}

/// explorer.exe con percorso completo: l'exe portable puo' stare ovunque
/// e non deve mai avviare un explorer.exe trovato nella sua cartella.
fn explorer() -> Command {
    let windows_dir = std::env::var_os("WINDIR").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    Command::new(windows_dir.join("explorer.exe"))
}

/// Apre un indirizzo web nel browser predefinito.
pub(crate) fn open_web(url: &str) {
    let _ = explorer().arg(url).spawn();
}

/// Apre Esplora file sul risultato.
fn reveal(path: &Path, is_folder: bool) {
    let mut command = explorer();
    if is_folder {
        command.arg(path);
    } else {
        command.raw_arg(format!("/select,\"{}\"", path.display()));
    }
    let _ = command.spawn();
}

fn update_row(index: usize, change: impl FnOnce(&mut Row)) {
    with_rows(|rows| {
        if let Some(row) = rows.get_mut(index) {
            change(row);
        }
    });
    request_repaint();
}

fn callbacks_for<'a>(progress: &'a dyn Fn(f64, &str), confirm: &'a dyn Fn(u64) -> bool) -> Callbacks<'a> {
    Callbacks { progress, confirm }
}

/// Il worker elabora un lavoro alla volta: i passaggi sono pesanti su CPU e RAM.
fn process_one(row: usize, op: Operation, source: Source) {
    update_row(row, |r| {
        r.state = RowState::Working;
        r.status = "Avvio…".into();
    });
    let name = source.name.clone();
    let refused = Cell::new(0u64);
    let result = tools::load(source).and_then(|file| {
        let dest = destination_for(row, op, &file)?;
        let progress = progress_for(row);
        let confirm = confirm_for(row, &name, &refused);
        tools::run(op, &file, &dest, callbacks_for(&progress, &confirm))
    });
    finish_row(row, result, refused.get());
}

fn process_join(row: usize, sources: Vec<Source>) {
    update_row(row, |r| {
        r.state = RowState::Working;
        r.status = "Lettura dei file…".into();
    });
    let refused = Cell::new(0u64);
    let result = (|| {
        let mut files: Vec<Loaded> = Vec::with_capacity(sources.len());
        for source in sources {
            let name = source.name.clone();
            let file = tools::load(source).map_err(|error| match error {
                ToolError::Message(text) => ToolError::Message(format!("{name}: {text}")),
                other => other,
            })?;
            files.push(file);
        }
        let first_stem = Path::new(&files[0].name).file_stem().map_or_else(|| "documento".to_string(), |s| s.to_string_lossy().into_owned());
        let fixed = zone_folder(SLOT_JOIN, &files[0].name).or_else(|| fixed_folder(&files[0].name));
        let keep_beside = settings::current().save == settings::Save::Beside && files.iter().all(saved_outside_temp);
        let dest = match (&files[0].path, fixed) {
            (_, Some(dest)) => dest,
            (Some(path), None) if keep_beside => Destination::NextTo(path.clone()),
            _ => {
                update_row(row, |r| r.status = "Scegli dove salvare".into());
                let suggested = format!("{first_stem}_unito{}.pdf", tools::limit_suffix());
                let target = dialogs::pick_file(main_hwnd(), &suggested);
                Destination::File(target.ok_or(ToolError::Cancelled)?)
            }
        };
        let progress = progress_for(row);
        let confirm = confirm_for(row, "Documento unito", &refused);
        tools::run_join(&files, &dest, callbacks_for(&progress, &confirm))
    })();
    finish_row(row, result, refused.get());
}

fn saved_outside_temp(file: &Loaded) -> bool {
    file.path.as_deref().is_some_and(|path| !location::is_temporary(path))
}

/// Posti in settings::ZONE_FOLDERS delle zone che salvano file.
pub(crate) const SLOT_COMPRESS: usize = 0;
pub(crate) const SLOT_SPLIT: usize = 1;
pub(crate) const SLOT_TEXT: usize = 2;
pub(crate) const SLOT_WORD: usize = 3;
pub(crate) const SLOT_TOOLS: usize = 4;
pub(crate) const SLOT_PHOTO: usize = 5;
pub(crate) const SLOT_JOIN: usize = 6;

fn slot_of(op: Operation) -> usize {
    match op {
        Operation::Compress => SLOT_COMPRESS,
        Operation::Split => SLOT_SPLIT,
        Operation::Text => SLOT_TEXT,
        Operation::Word => SLOT_WORD,
    }
}

/// Cartella propria della zona, se scelta ed esistente: vince sulla regola generale. Se manca
/// (altro PC, OneDrive non sincronizzato) vale la regola generale.
fn zone_folder(slot: usize, name: &str) -> Option<Destination> {
    let folder = settings::zone_folder(slot).filter(|f| f.is_dir() && !location::is_temporary(f))?;
    let file = Path::new(name).file_name().map_or_else(|| "documento.pdf".into(), |n| n.to_os_string());
    Some(Destination::NextTo(folder.join(file)))
}

/// Dove salvare senza chiedere: nella cartella della zona, se scelta; poi nella cartella fissa,
/// se scelta, oppure accanto all'originale se sta in una cartella normale. None = si chiede.
fn beside(slot: usize, path: Option<&Path>, name: &str) -> Option<Destination> {
    if let Some(dest) = zone_folder(slot, name) {
        return Some(dest);
    }
    match settings::current().save {
        settings::Save::Folder => fixed_folder(name),
        settings::Save::Beside => path.filter(|p| !location::is_temporary(p)).map(|p| Destination::NextTo(p.to_path_buf())),
        settings::Save::Ask => None,
    }
}

/// Cartella fissa delle impostazioni, se scelta ed esistente: il file prodotto ci finisce con
/// lo stesso nome dell'originale piu' il suffisso, senza mai sovrascrivere. Se la cartella
/// manca (altro PC, OneDrive non sincronizzato) si chiede.
fn fixed_folder(name: &str) -> Option<Destination> {
    if settings::current().save != settings::Save::Folder {
        return None;
    }
    let folder = settings::output_folder().filter(|f| f.is_dir() && !location::is_temporary(f))?;
    let file = Path::new(name).file_name().map_or_else(|| "documento.pdf".into(), |n| n.to_os_string());
    Some(Destination::NextTo(folder.join(file)))
}

/// Un file in cartella normale si salva accanto, salvo diversa impostazione (cartella fissa o
/// chiedi sempre); da Outlook o da una temporanea si chiede, salvo cartella fissa.
fn destination_for(row: usize, op: Operation, file: &Loaded) -> Result<Destination, ToolError> {
    if let Some(dest) = beside(slot_of(op), file.path.as_deref(), &file.name) {
        return Ok(dest);
    }
    update_row(row, |r| r.status = "Scegli dove salvare".into());
    let stem = tools::file_stem(&file.name);
    if op == Operation::Split {
        dialogs::pick_folder(main_hwnd(), &format!("{stem}_pagine")).map(Destination::Folder).ok_or(ToolError::Cancelled)
    } else if op == Operation::Word {
        dialogs::pick_file(main_hwnd(), &format!("{stem}.docx")).map(Destination::File).ok_or(ToolError::Cancelled)
    } else {
        dialogs::pick_file(main_hwnd(), &format!("{stem}{}.pdf", tools::limit_suffix())).map(Destination::File).ok_or(ToolError::Cancelled)
    }
}

/// Salvataggi della zona Strumenti: stessa regola su dove salvare delle altre zone.
fn process_tools(row: usize, task: ToolsTask, name: &str, path: Option<PathBuf>, data: &[u8], plan: &Plan) {
    update_row(row, |r| {
        r.state = RowState::Working;
        r.status = "Avvio…".into();
    });
    let result = (|| {
        let dest = match beside(SLOT_TOOLS, path.as_deref(), name) {
            Some(dest) => dest,
            None => {
                update_row(row, |r| r.status = "Scegli dove salvare".into());
                let stem = tools::file_stem(name);
                let picked = match &task {
                    ToolsTask::Save => dialogs::pick_file(main_hwnd(), &format!("{stem}_modificato.pdf")).map(Destination::File),
                    ToolsTask::Extract(_) => dialogs::pick_file(main_hwnd(), &format!("{stem}_estratto.pdf")).map(Destination::File),
                    ToolsTask::Jpg => dialogs::pick_folder(main_hwnd(), &format!("{stem}_immagini")).map(Destination::Folder),
                };
                picked.ok_or(ToolError::Cancelled)?
            }
        };
        let progress = progress_for(row);
        match &task {
            ToolsTask::Save => edit::save_modified(&dest, data, plan, &progress),
            ToolsTask::Extract(selected) => edit::save_extract(&dest, data, plan, selected, &progress),
            ToolsTask::Jpg => edit::save_jpegs(&dest, name, data, plan, &progress),
        }
    })();
    finish_row(row, result, 0);
}

fn progress_for(row: usize) -> impl Fn(f64, &str) {
    move |pct: f64, text: &str| {
        update_row(row, |r| {
            r.pct = pct;
            r.status = text.to_string();
        });
    }
}

/// Applica l'impostazione sulla conversione in immagini. Se e' "mai", ricorda il minimo
/// raggiunto per spiegarlo nella riga.
fn confirm_for<'a>(row: usize, name: &'a str, refused: &'a Cell<u64>) -> impl Fn(u64) -> bool + 'a {
    move |kept: u64| match settings::current().conversion {
        Conversion::Always => true,
        Conversion::Never => {
            refused.set(kept.max(1));
            false
        }
        Conversion::Ask => {
            update_row(row, |r| r.status = "In attesa di conferma".into());
            ask_raster(name, kept)
        }
    }
}

fn finish_row(row: usize, result: Result<Outcome, ToolError>, refused: u64) {
    update_row(row, |r| {
        r.pct = 1.0;
        match result {
            Ok(outcome) => {
                r.state = RowState::Done;
                r.status = outcome.summary;
                r.output = Some((outcome.output, outcome.is_folder));
            }
            Err(ToolError::Cancelled) if refused > 0 => {
                r.state = RowState::Failed;
                r.status = if refused > 1 {
                    format!("conversione in immagini disattivata nelle impostazioni: minimo {}", tools::format_mb(refused))
                } else {
                    "conversione in immagini disattivata nelle impostazioni".into()
                };
            }
            Err(ToolError::Cancelled) => {
                r.state = RowState::Cancelled;
                r.status = CANCELLED_TEXT.into();
            }
            Err(ToolError::Message(text)) => {
                r.state = RowState::Failed;
                r.status = text;
            }
        }
    });
}

fn ask_raster(name: &str, kept: u64) -> bool {
    let reason = if kept > 0 {
        format!("Senza convertire le pagine, questo PDF non scende sotto {}.", tools::format_mb(kept))
    } else {
        "Questo PDF non si può ridurre senza convertire le pagine.".to_string()
    };
    let text = format!(
        "{name}\n\n{reason}\n\nConvertire le pagine in immagini per stare sotto {}?\nIl testo originale viene sostituito da quello riconosciuto con OCR, che può contenere errori. L'originale resta intatto.",
        tools::limit_label()
    );
    win::message_box(main_hwnd(), &text, APP_TITLE, MB_YESNO | MB_ICONQUESTION | MB_TOPMOST) == IDYES
}

fn is_busy() -> bool {
    with_rows(|rows| rows.iter().any(|r| matches!(r.state, RowState::Queued | RowState::Working))).unwrap_or(false)
}

/// Zona sotto un punto dello schermo: la usa il trascinamento.
pub(crate) fn zone_at_screen(hwnd: HWND, x: i32, y: i32) -> Option<usize> {
    let mut point = POINT { x, y };
    unsafe { ScreenToClient(hwnd, &mut point) };
    UI.with(|ui| (0..ZONE_COUNT).find(|&i| paint::zone_rect(hwnd, ui, i).contains(point.x, point.y)))
}

pub(crate) fn set_drag_zone(hwnd: HWND, zone: Option<usize>) {
    UI.with(|ui| {
        if ui.drag_zone.replace(zone) != zone {
            unsafe { InvalidateRect(hwnd, null(), 0) };
        }
    });
}

pub(crate) fn low_word(value: isize) -> i32 {
    (value & 0xFFFF) as i16 as i32
}

pub(crate) fn high_word(value: isize) -> i32 {
    ((value >> 16) & 0xFFFF) as i16 as i32
}

fn hit_test(hwnd: HWND, ui: &Ui, x: i32, y: i32) -> Hover {
    if paint::close_rect(hwnd, ui).contains(x, y) {
        return Hover::Close;
    }
    if paint::info_rect(hwnd, ui).contains(x, y) {
        return Hover::Info;
    }
    if paint::settings_rect(hwnd, ui).contains(x, y) {
        return Hover::Settings;
    }
    if has_scanner() && paint::scanner_rect(hwnd, ui).contains(x, y) {
        return Hover::Scanner;
    }
    if paint::update_banner_rect(hwnd, ui).is_some_and(|r| r.contains(x, y)) {
        return Hover::UpdateBanner;
    }
    let tray = tray_len();
    if paint::join_button(hwnd, ui).contains(x, y) {
        return if tray >= 2 { Hover::Join } else { Hover::Nothing };
    }
    if paint::clear_button(hwnd, ui).contains(x, y) {
        return if tray >= 1 { Hover::Clear } else { Hover::Nothing };
    }
    if paint::zone_rect(hwnd, ui, ZONE_JOIN).contains(x, y) && tray >= 1 {
        return Hover::JoinZone;
    }
    if paint::zone_rect(hwnd, ui, ZONE_TOOLS).contains(x, y) {
        return Hover::ToolsZone;
    }
    if paint::zone_rect(hwnd, ui, ZONE_SIGNATURES).contains(x, y) {
        return Hover::SignaturesZone;
    }
    if paint::zone_rect(hwnd, ui, ZONE_PHOTO).contains(x, y) {
        return Hover::PhotoZone;
    }
    paint::folder_at(hwnd, ui, x, y).map_or(Hover::Nothing, Hover::Folder)
}

fn set_hover(hwnd: HWND, ui: &Ui, hover: Hover) {
    if ui.hover.replace(hover) != hover {
        unsafe { InvalidateRect(hwnd, null(), 0) };
    }
}

fn on_click(hwnd: HWND, x: i32, y: i32) {
    match UI.with(|ui| hit_test(hwnd, ui, x, y)) {
        Hover::Close => {
            unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
        }
        Hover::Info => panel::toggle(panel::Mode::Info),
        Hover::Settings => panel::toggle(panel::Mode::Settings),
        Hover::Scanner => panel::toggle(panel::Mode::Scanner),
        Hover::UpdateBanner => handle_update_action(hwnd),
        Hover::Join => start_join(),
        Hover::Clear => clear_tray(),
        Hover::JoinZone => panel::show_join(None),
        Hover::ToolsZone => panel::toggle(panel::Mode::Tools),
        Hover::SignaturesZone => panel::toggle(panel::Mode::Signatures),
        Hover::PhotoZone => panel::toggle(panel::Mode::Photo),
        Hover::Folder(index) => {
            let output = with_rows(|rows| rows.get(index).and_then(|r| r.output.clone())).flatten();
            if let Some((path, is_folder)) = output {
                reveal(&path, is_folder);
            }
        }
        Hover::Nothing => {}
    }
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match message {
            WM_PAINT => {
                UI.with(|ui| paint::paint(hwnd, ui));
                return 0;
            }
            WM_ERASEBKGND => return 1,
            WM_REPAINT => {
                InvalidateRect(hwnd, null(), 0);
                panel::invalidate();
                return 0;
            }
            WM_UPDATE => {
                match wparam {
                    0 => offer_update(hwnd),
                    WPARAM_NEWS => show_news(hwnd),
                    _ => offer_restart(hwnd),
                }
                return 0;
            }
            WM_MOUSEMOVE => {
                UI.with(|ui| {
                    if !ui.tracking.replace(true) {
                        let mut track = TRACKMOUSEEVENT {
                            cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                            dwFlags: TME_LEAVE,
                            hwndTrack: hwnd,
                            dwHoverTime: 0,
                        };
                        TrackMouseEvent(&mut track);
                    }
                    let hover = hit_test(hwnd, ui, low_word(lparam), high_word(lparam));
                    set_hover(hwnd, ui, hover);
                });
                return 0;
            }
            WM_MOUSELEAVE_MSG => {
                UI.with(|ui| {
                    ui.tracking.set(false);
                    set_hover(hwnd, ui, Hover::Nothing);
                });
                return 0;
            }
            WM_LBUTTONUP => {
                on_click(hwnd, low_word(lparam), high_word(lparam));
                return 0;
            }
            WM_MOUSEWHEEL => {
                let delta = high_word(wparam as isize);
                UI.with(|ui| ui.scroll.set(ui.scroll.get() - delta * scale(ui, ROW_HEIGHT) / 120));
                InvalidateRect(hwnd, null(), 0);
                return 0;
            }
            WM_CLOSE => {
                if is_busy()
                    && win::message_box(hwnd, "Ci sono file in lavorazione. Chiudere comunque?", APP_TITLE, MB_YESNO | MB_ICONWARNING | MB_TOPMOST) != IDYES
                {
                    RESTART.store(false, Ordering::SeqCst);
                    return 0;
                }
                panel::destroy();
                DestroyWindow(hwnd);
                return 0;
            }
            WM_DESTROY => {
                drop_target::revoke(hwnd);
                remove_appbar(hwnd);
                PostQuitMessage(0);
                return 0;
            }
            WM_QUERYENDSESSION => return 1,
            WM_ENDSESSION => {
                if wparam != 0 {
                    remove_appbar(hwnd);
                }
                return 0;
            }
            WM_APPBAR_NOTIFY => {
                match wparam as u32 {
                    ABN_POSCHANGED => position_appbar(hwnd),
                    // Un'app a schermo intero deve poter coprire barra e pannello.
                    ABN_FULLSCREENAPP => {
                        let order = if lparam != 0 { HWND_NOTOPMOST } else { HWND_TOPMOST };
                        SetWindowPos(hwnd, order, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                        panel::set_topmost(lparam == 0);
                    }
                    _ => {}
                }
                return 0;
            }
            WM_ACTIVATE => {
                if UI.with(|ui| ui.registered.get()) {
                    let mut data = appbar_data(hwnd);
                    SHAppBarMessage(ABM_ACTIVATE, &mut data);
                }
            }
            WM_WINDOWPOSCHANGED => {
                if UI.with(|ui| ui.registered.get()) {
                    let mut data = appbar_data(hwnd);
                    SHAppBarMessage(ABM_WINDOWPOSCHANGED, &mut data);
                }
            }
            WM_DISPLAYCHANGE => position_appbar(hwnd),
            WM_DPICHANGED_MSG => {
                update_dpi(hwnd);
                position_appbar(hwnd);
                return 0;
            }
            WM_SETTINGCHANGE => {
                UI.with(|ui| ui.palette.set(paint::palette_for(settings::current().theme)));
                InvalidateRect(hwnd, null(), 0);
                panel::invalidate();
            }
            other => {
                // Se Explorer riparte perde le appbar: ci si registra di nuovo solo se reserve e' attivo.
                let taskbar = UI.with(|ui| ui.taskbar_msg.get());
                if taskbar != 0 && other == taskbar {
                    if settings::current().reserve {
                        register_appbar(hwnd);
                    } else {
                        position_appbar(hwnd);
                    }
                    return 0;
                }
            }
        }
        DefWindowProcW(hwnd, message, wparam, lparam)
    }
}

#[cfg(test)]
mod tests {
    use super::move_item;

    #[test]
    fn move_item_follows_the_insertion_point() {
        let mut items = vec!['a', 'b', 'c', 'd'];
        move_item(&mut items, 0, 3);
        assert_eq!(items, ['b', 'c', 'a', 'd']);
        move_item(&mut items, 3, 0);
        assert_eq!(items, ['d', 'b', 'c', 'a']);
        move_item(&mut items, 1, 4);
        assert_eq!(items, ['d', 'c', 'a', 'b']);
        move_item(&mut items, 2, 2);
        assert_eq!(items, ['d', 'c', 'a', 'b']);
        move_item(&mut items, 9, 0);
        assert_eq!(items, ['d', 'c', 'a', 'b']);
    }
}

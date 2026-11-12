//! Impostazioni: file ToolsPDF.ini accanto all'exe, cosi' l'app resta portable.
//! Se la cartella dell'exe non e' scrivibile si usa %APPDATA%\ToolsPDF.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const FILE_NAME: &str = "ToolsPDF.ini";
pub const LIMITS_MB: [u32; 4] = [1, 2, 5, 10];

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Conversion {
    Ask,
    Always,
    Never,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Side {
    Right,
    Left,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Theme {
    System,
    Light,
    Dark,
}

/// Dove salvare i file prodotti.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Save {
    /// Accanto all'originale; da Outlook e dalle cartelle temporanee chiede.
    Beside,
    /// Sempre nella cartella scelta (output_folder), anche per gli allegati.
    Folder,
    /// Chiede ogni volta.
    Ask,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Settings {
    pub limit_mb: u32,
    /// Dove finiscono i file prodotti.
    pub save: Save,
    pub conversion: Conversion,
    /// OCR anche in Comprimi, Dividi e Unisci; la zona Testo lo fa sempre.
    pub always_ocr: bool,
    pub side: Side,
    pub theme: Theme,
    /// Numero dello schermo (\\.\DISPLAYn); None sceglie il bordo esterno del desktop.
    pub monitor: Option<u32>,
    /// Controlla su GitHub se c'e' una versione nuova (e chiede prima di scaricarla).
    pub check_updates: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            limit_mb: 1,
            save: Save::Beside,
            conversion: Conversion::Ask,
            always_ocr: true,
            side: Side::Right,
            theme: Theme::System,
            monitor: None,
            check_updates: true,
        }
    }
}

impl Settings {
    /// Legge il formato chiave=valore; valori sconosciuti restano predefiniti.
    pub fn parse(text: &str) -> Self {
        let mut settings = Settings::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            match (key.trim(), value.trim()) {
                ("limite_mb", v) => {
                    if let Some(mb) = v.parse().ok().filter(|mb| LIMITS_MB.contains(mb)) {
                        settings.limit_mb = mb;
                    }
                }
                ("dove_salvare", "chiedi_sempre") => settings.save = Save::Ask,
                ("dove_salvare", "accanto") => settings.save = Save::Beside,
                ("dove_salvare", "cartella") => settings.save = Save::Folder,
                ("conversione", "chiedi") => settings.conversion = Conversion::Ask,
                ("conversione", "sempre") => settings.conversion = Conversion::Always,
                ("conversione", "mai") => settings.conversion = Conversion::Never,
                ("ocr", "sempre") => settings.always_ocr = true,
                ("ocr", "solo_testo") => settings.always_ocr = false,
                ("lato", "destra") => settings.side = Side::Right,
                ("lato", "sinistra") => settings.side = Side::Left,
                ("tema", "sistema") => settings.theme = Theme::System,
                ("tema", "chiaro") => settings.theme = Theme::Light,
                ("tema", "scuro") => settings.theme = Theme::Dark,
                ("schermo", "auto") => settings.monitor = None,
                ("schermo", v) => settings.monitor = v.parse().ok().filter(|n| *n > 0),
                ("aggiornamenti", "chiedi") => settings.check_updates = true,
                ("aggiornamenti", "mai") => settings.check_updates = false,
                _ => {}
            }
        }
        settings
    }

    pub fn to_text(&self) -> String {
        let conversion = match self.conversion {
            Conversion::Ask => "chiedi",
            Conversion::Always => "sempre",
            Conversion::Never => "mai",
        };
        let side = if self.side == Side::Left { "sinistra" } else { "destra" };
        let theme = match self.theme {
            Theme::System => "sistema",
            Theme::Light => "chiaro",
            Theme::Dark => "scuro",
        };
        let save = match self.save {
            Save::Beside => "accanto",
            Save::Folder => "cartella",
            Save::Ask => "chiedi_sempre",
        };
        let monitor = self.monitor.map_or_else(|| "auto".to_string(), |n| n.to_string());
        let ocr = if self.always_ocr { "sempre" } else { "solo_testo" };
        let updates = if self.check_updates { "chiedi" } else { "mai" };
        format!(
            "# Impostazioni di Tools PDF\r\nlimite_mb={}\r\ndove_salvare={save}\r\nconversione={conversion}\r\nocr={ocr}\r\nlato={side}\r\ntema={theme}\r\nschermo={monitor}\r\naggiornamenti={updates}\r\n",
            self.limit_mb
        )
    }
}

/// Preferenze della zona Strumenti, nello stesso ini. La password non si salva mai.
#[derive(Clone, PartialEq, Debug)]
pub struct ToolsPrefs {
    /// Testo del timbro diagonale.
    pub stamp: String,
    /// Immagine della firma scelta l'ultima volta.
    pub signature: Option<PathBuf>,
    /// Griglia 3x3 letta per righe dall'alto: 0 in alto a sinistra, 8 in basso a destra.
    pub signature_position: u8,
    /// 0 = S, 1 = M, 2 = L.
    pub signature_size: u8,
}

pub const STAMP_MAX_CHARS: usize = 40;
const SIZES: [&str; 3] = ["S", "M", "L"];

impl Default for ToolsPrefs {
    fn default() -> Self {
        ToolsPrefs { stamp: "RISERVATO".into(), signature: None, signature_position: 8, signature_size: 1 }
    }
}

impl ToolsPrefs {
    pub fn parse(text: &str) -> Self {
        let mut prefs = ToolsPrefs::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            match (key.trim(), value.trim()) {
                ("timbro", v) if !v.is_empty() => prefs.stamp = v.chars().take(STAMP_MAX_CHARS).collect(),
                ("firma", "") => prefs.signature = None,
                ("firma", v) => prefs.signature = Some(PathBuf::from(v)),
                ("firma_posizione", v) => {
                    if let Some(n) = v.parse().ok().filter(|n| *n < 9) {
                        prefs.signature_position = n;
                    }
                }
                ("firma_larghezza", v) => {
                    if let Some(n) = SIZES.iter().position(|s| s.eq_ignore_ascii_case(v)) {
                        prefs.signature_size = n as u8;
                    }
                }
                _ => {}
            }
        }
        prefs
    }

    pub fn to_text(&self) -> String {
        let signature = self.signature.as_ref().map_or_else(String::new, |p| p.display().to_string());
        let stamp: String = self.stamp.chars().filter(|c| !c.is_control()).collect();
        format!(
            "timbro={stamp}\r\nfirma={signature}\r\nfirma_posizione={}\r\nfirma_larghezza={}\r\n",
            self.signature_position.min(8),
            SIZES[usize::from(self.signature_size.min(2))]
        )
    }
}

static CURRENT: Mutex<Option<Settings>> = Mutex::new(None);
/// Cartella fissa per i file prodotti (Save::Folder). Fuori da Settings, che resta Copy.
static OUTPUT: Mutex<Option<PathBuf>> = Mutex::new(None);
const OUTPUT_KEY: &str = "cartella_output";

/// Cartella fissa scelta, anche se oggi la scelta attiva non e' Save::Folder.
pub fn output_folder() -> Option<PathBuf> {
    OUTPUT.lock().ok().and_then(|o| o.clone())
}

fn set_output_folder(folder: Option<PathBuf>) {
    if let Ok(mut current) = OUTPUT.lock() {
        *current = folder;
    }
}

fn parse_output(text: &str) -> Option<PathBuf> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .find(|(key, _)| key.trim() == OUTPUT_KEY)
        .map(|(_, value)| value.trim())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn output_text() -> String {
    let folder = output_folder().map_or_else(String::new, |p| p.display().to_string());
    format!("{OUTPUT_KEY}={folder}\r\n")
}
static TOOLS: Mutex<Option<ToolsPrefs>> = Mutex::new(None);

pub fn tools_prefs() -> ToolsPrefs {
    TOOLS.lock().ok().and_then(|t| t.clone()).unwrap_or_default()
}

fn set_tools_prefs(prefs: ToolsPrefs) {
    if let Ok(mut current) = TOOLS.lock() {
        *current = Some(prefs);
    }
}

pub fn current() -> Settings {
    CURRENT.lock().ok().and_then(|s| *s).unwrap_or_default()
}

fn set_current(settings: Settings) {
    if let Ok(mut current) = CURRENT.lock() {
        *current = Some(settings);
    }
}

/// Cartella dell'exe: l'app e' portable e ci tiene impostazioni e firme.
pub fn portable_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(PathBuf::from)
}

/// Vero se l'exe (e quindi firme e impostazioni accanto) sta in una cartella OneDrive.
pub fn on_onedrive() -> bool {
    let roots: Vec<PathBuf> = ["OneDrive", "OneDriveCommercial", "OneDriveConsumer"].iter().filter_map(|name| std::env::var_os(name)).map(PathBuf::from).collect();
    portable_dir().is_some_and(|dir| inside(&dir, &roots))
}

/// dir dentro una delle cartelle indicate, senza badare a maiuscole e barre finali.
fn inside(dir: &Path, roots: &[PathBuf]) -> bool {
    let norm = |p: &Path| p.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase();
    let dir = norm(dir);
    roots.iter().map(|r| norm(r)).any(|root| !root.is_empty() && (dir == root || dir.starts_with(&format!("{root}\\"))))
}

/// %APPDATA%\ToolsPDF, se la cartella dell'exe non e' scrivibile.
pub fn appdata_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|dir| PathBuf::from(dir).join("ToolsPDF"))
}

fn portable_path() -> Option<PathBuf> {
    portable_dir().map(|dir| dir.join(FILE_NAME))
}

fn appdata_path() -> Option<PathBuf> {
    appdata_dir().map(|dir| dir.join(FILE_NAME))
}

/// File in uso: quello accanto all'exe se c'e', altrimenti quello in AppData.
pub fn file_path() -> Option<PathBuf> {
    let portable = portable_path();
    if portable.as_ref().is_some_and(|p| p.exists()) {
        return portable;
    }
    appdata_path().filter(|p| p.exists()).or(portable)
}

pub fn load() -> Settings {
    let text = file_path().and_then(|p| fs::read_to_string(p).ok()).unwrap_or_default();
    let settings = Settings::parse(&text);
    set_current(settings);
    set_tools_prefs(ToolsPrefs::parse(&text));
    set_output_folder(parse_output(&text));
    settings
}

/// Salva e rende attive le impostazioni e la cartella fissa. Restituisce il file scritto.
pub fn save(settings: Settings, folder: Option<PathBuf>) -> Result<PathBuf, String> {
    set_current(settings);
    set_output_folder(folder);
    write_file(&full_text())
}

/// Salva le preferenze della zona Strumenti, nello stesso file.
pub fn save_tools(prefs: ToolsPrefs) -> Result<PathBuf, String> {
    set_tools_prefs(prefs);
    write_file(&full_text())
}

/// Il file si riscrive sempre intero: impostazioni generali e zona Strumenti.
fn full_text() -> String {
    current().to_text() + &output_text() + &tools_prefs().to_text()
}

fn write_file(text: &str) -> Result<PathBuf, String> {
    if let Some(path) = portable_path() {
        if fs::write(&path, text).is_ok() {
            return Ok(path);
        }
    }
    let path = appdata_path().ok_or("cartella AppData non trovata")?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::write(&path, text).map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_keeps_every_choice() {
        let settings = Settings { limit_mb: 5, save: Save::Folder, conversion: Conversion::Never, always_ocr: false, side: Side::Left, theme: Theme::Dark, monitor: Some(2), check_updates: false };
        assert_eq!(Settings::parse(&settings.to_text()), settings);
    }

    #[test]
    fn unknown_or_broken_values_fall_back_to_defaults() {
        let parsed = Settings::parse("limite_mb=7\nconversione=boh\nocr=forse\nlato\n# commento\ntema=chiaro\nschermo=0");
        assert_eq!(parsed, Settings { theme: Theme::Light, ..Settings::default() });
    }

    #[test]
    fn tools_prefs_round_trip_and_share_the_file() {
        let prefs = ToolsPrefs { stamp: "COPIA = conforme".into(), signature: Some(PathBuf::from(r"C:\firme\io.png")), signature_position: 2, signature_size: 0 };
        let text = Settings::default().to_text() + &prefs.to_text();
        assert_eq!(ToolsPrefs::parse(&text), prefs);
        assert_eq!(Settings::parse(&text), Settings::default());
        assert_eq!(ToolsPrefs::parse("firma_posizione=9\nfirma_larghezza=XL\ntimbro="), ToolsPrefs::default());
    }

    #[test]
    fn ocr_is_on_unless_disabled() {
        assert!(Settings::parse("").always_ocr);
        assert!(!Settings::parse("ocr=solo_testo").always_ocr);
    }

    #[test]
    fn output_folder_shares_the_file_and_empty_means_none() {
        let text = format!("{}cartella_output=D:\\PDF fatti\r\n", Settings::default().to_text());
        assert_eq!(parse_output(&text), Some(PathBuf::from(r"D:\PDF fatti")));
        assert_eq!(Settings::parse(&text), Settings::default());
        assert_eq!(parse_output("cartella_output=\r\n"), None);
        assert_eq!(Settings::parse("dove_salvare=cartella").save, Save::Folder);
    }

    #[test]
    fn onedrive_folder_is_recognized() {
        let roots = [PathBuf::from(r"C:\Users\a\OneDrive - Ditta SA\")];
        assert!(inside(Path::new(r"c:\users\a\onedrive - ditta sa\ToolsPDF"), &roots));
        assert!(inside(Path::new(r"C:\Users\a\OneDrive - Ditta SA"), &roots));
        assert!(!inside(Path::new(r"C:\Users\a\OneDrive - Ditta SA2\x"), &roots));
        assert!(!inside(Path::new(r"C:\Tools"), &roots));
        assert!(!inside(Path::new(r"C:\Tools"), &[PathBuf::new()]));
    }
}

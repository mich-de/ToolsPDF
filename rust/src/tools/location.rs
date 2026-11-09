//! Riconosce le cartelle temporanee: da li' non si salva mai senza chiedere.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Parti di percorso usate dalle cache di Outlook e del browser.
const TEMPORARY_SEGMENTS: [&str; 5] = [
    "\\content.outlook\\",
    "\\inetcache\\",
    "\\temporary internet files\\",
    "\\microsoft.outlookforwindows_",
    "\\temp\\",
];

/// Vero se il percorso sta in una cartella temporanea, di Windows o di Outlook.
pub fn is_temporary(path: &Path) -> bool {
    let resolved = normalized(path);
    let roots: Vec<String> = temporary_roots().iter().map(|root| normalized(root)).collect();
    matches_temporary(&resolved, &roots)
}

fn matches_temporary(path: &str, roots: &[String]) -> bool {
    let with_separator = format!("{}\\", path.trim_end_matches('\\'));
    TEMPORARY_SEGMENTS.iter().any(|segment| with_separator.contains(segment))
        || roots.iter().any(|root| {
            let root = format!("{}\\", root.trim_end_matches('\\'));
            with_separator.starts_with(&root)
        })
}

/// Percorso assoluto in minuscolo, senza nomi brevi di tipo MDEANG~1.
fn normalized(path: &Path) -> String {
    let full = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let text = full.to_string_lossy().to_lowercase();
    text.strip_prefix(r"\\?\").map_or_else(|| text.clone(), str::to_string)
}

fn temporary_roots() -> Vec<PathBuf> {
    let mut roots = vec![env::temp_dir()];
    for name in ["TEMP", "TMP"] {
        if let Some(value) = env::var_os(name) {
            roots.push(PathBuf::from(value));
        }
    }
    if let Some(local) = env::var_os("LOCALAPPDATA") {
        let local = PathBuf::from(local);
        roots.push(local.join("Temp"));
        roots.push(local.join(r"Microsoft\Windows\INetCache"));
    }
    if let Some(windows) = env::var_os("WINDIR") {
        roots.push(PathBuf::from(windows).join("Temp"));
    }
    roots
}

#[cfg(test)]
mod tests {
    use super::matches_temporary;

    #[test]
    fn outlook_and_windows_temp_are_recognized() {
        let roots = vec![r"c:\users\a\appdata\local\temp".to_string()];
        for path in [
            r"c:\users\a\appdata\local\microsoft\windows\inetcache\content.outlook\jx0qdu0u\f.pdf",
            r"d:\altro\content.outlook\abc\f.pdf",
            r"c:\users\a\appdata\local\temp\f.pdf",
            r"c:\users\a\appdata\local\packages\microsoft.outlookforwindows_8wekyb3d8bbwe\f.pdf",
        ] {
            assert!(matches_temporary(path, &roots), "{path}");
        }
    }

    #[test]
    fn normal_folders_are_not_temporary() {
        let roots = vec![r"c:\users\a\appdata\local\temp".to_string()];
        for path in [r"d:\documenti\f.pdf", r"c:\users\a\desktop\temperature\f.pdf", r"c:\users\a\appdata\local\temporaneo\f.pdf"] {
            assert!(!matches_temporary(path, &roots), "{path}");
        }
    }
}

//! Scritte stampate dei moduli accanto alle firme ("Signature of Seafarer", "SIGNATURE - FIRMA"),
//! trovate con ocrs: OCR in Rust puro con i modelli dentro l'exe, nessun file o rete.
//! ocrs legge anche la firma, quindi valgono solo le righe con parole tipiche dei moduli,
//! riconosciute anche se lette male o tagliate al bordo.

use std::sync::OnceLock;

use image::RgbImage;
use ocrs::{ImageSource, OcrEngine, OcrEngineParams, TextItem};
use rten::Model;

static DETECTION: &[u8] = include_bytes!("../../models/text-detection.onnx");
static RECOGNITION: &[u8] = include_bytes!("../../models/text-recognition.onnx");

/// Parole dei moduli (minuscole, senza accenti). Le corte valgono solo in una riga che ne ha
/// gia' una lunga: da sole potrebbero essere un pezzo di firma letto per caso.
const KEYWORDS: &[&str] = &[
    "signature", "signatures", "sign", "signed", "signing", "firma", "firme", "firmato", "firmare", "firmante", "sottoscritto",
    "undersigned", "signataire", "seafarer", "seaman", "seamans", "master", "captain", "capitano", "comandante", "holder",
    "titolare", "bearer", "date", "data", "fecha", "name", "nome", "nombre", "surname", "cognome", "place", "luogo",
    "presence", "presenza", "medical", "medico", "doctor", "practitioner", "officer", "authorized", "authorised", "official",
    "applicant", "richiedente", "employee", "dipendente", "witness", "testimone", "stamp", "timbro", "seal", "sello",
    "company", "owner", "armatore", "crew", "member", "examiner", "issuing", "authority", "autorita", "issued", "rilasciato",
    "interesado", "here", "qui", "of", "the", "del", "della", "di", "and", "por", "by", "for", "per", "en", "de",
];
/// Sotto questa lunghezza una parola conta solo accanto a una parola lunga riconosciuta.
const SHORT: usize = 4;
/// Quota delle lettere della riga in parole riconosciute oltre la quale si toglie la riga intera.
const WHOLE_LINE: f64 = 0.5;

/// Riquadro in pixel dell'immagine: x0, y0, x1, y1.
pub type Box4 = [f32; 4];

fn engine() -> Option<&'static OcrEngine> {
    static ENGINE: OnceLock<Option<OcrEngine>> = OnceLock::new();
    ENGINE
        .get_or_init(|| {
            let detection = Model::load_static_slice(DETECTION).ok()?;
            let recognition = Model::load_static_slice(RECOGNITION).ok()?;
            OcrEngine::new(OcrEngineParams { detection_model: Some(detection), recognition_model: Some(recognition), ..Default::default() }).ok()
        })
        .as_ref()
}

/// Riquadri delle scritte stampate dei moduli nell'immagine. Vuoto se non ce ne sono o se
/// l'OCR non parte.
pub fn printed_boxes(image: &RgbImage) -> Vec<Box4> {
    let Some(engine) = engine() else { return Vec::new() };
    let Ok(source) = ImageSource::from_bytes(image.as_raw(), image.dimensions()) else { return Vec::new() };
    let Ok(input) = engine.prepare_input(source) else { return Vec::new() };
    let Ok(words) = engine.detect_words(&input) else { return Vec::new() };
    let lines = engine.find_text_lines(&input, &words);
    let Ok(texts) = engine.recognize_text(&input, &lines) else { return Vec::new() };

    let mut boxes = Vec::new();
    for line in texts.iter().flatten() {
        let words: Vec<(String, Box4)> = line
            .words()
            .map(|word| {
                let r = word.bounding_rect();
                (word.to_string(), [r.left() as f32, r.top() as f32, r.right() as f32, r.bottom() as f32])
            })
            .collect();
        let texts: Vec<&str> = words.iter().map(|(text, _)| text.as_str()).collect();
        let picked = pick(&texts);
        if picked.iter().all(|&p| !p) {
            continue;
        }
        let letters = |only: bool| -> usize {
            texts.iter().zip(&picked).filter(|&(_, p)| *p || !only).map(|(t, _)| t.chars().filter(|c| c.is_alphanumeric()).count()).sum()
        };
        if letters(true) as f64 >= letters(false) as f64 * WHOLE_LINE {
            let r = line.bounding_rect();
            boxes.push([r.left() as f32, r.top() as f32, r.right() as f32, r.bottom() as f32]);
        } else {
            boxes.extend(words.iter().zip(&picked).filter(|&(_, p)| *p).map(|((_, b), _)| *b));
        }
    }
    boxes
}

/// Quali parole di una riga sono scritte dei moduli.
fn pick(words: &[&str]) -> Vec<bool> {
    let parts: Vec<Vec<String>> = words.iter().map(|w| split(w)).collect();
    let strong = parts.iter().flatten().any(|p| p.chars().count() >= SHORT && known(p));
    parts.iter().map(|ps| strong && ps.iter().any(|p| known(p))).collect()
}

/// Pezzi di sole lettere, minuscoli e senza accenti: "SIGNATIRE-FIRM" = "signatire", "firm".
fn split(word: &str) -> Vec<String> {
    let plain: String = word
        .chars()
        .map(|c| match c {
            'à' | 'á' | 'À' | 'Á' => 'a',
            'è' | 'é' | 'È' | 'É' => 'e',
            'ì' | 'í' => 'i',
            'ò' | 'ó' => 'o',
            'ù' | 'ú' => 'u',
            c => c,
        })
        .collect();
    plain.split(|c: char| !c.is_alphabetic()).filter(|p| !p.is_empty()).map(|p| p.to_lowercase()).collect()
}

/// Parola dei moduli, anche letta con un errore (due se lunga) o tagliata all'inizio o alla fine.
fn known(part: &str) -> bool {
    let p: Vec<char> = part.chars().collect();
    KEYWORDS.iter().any(|k| {
        let k: Vec<char> = k.chars().collect();
        let tolerance = match p.len() {
            0..=3 => 0,
            4..=7 => 1,
            _ => 2,
        };
        if distance(&p, &k) <= tolerance {
            return true;
        }
        // Tagliata al bordo: almeno 4 lettere uguali (a meno di un errore) all'inizio o alla fine.
        p.len() >= SHORT && p.len() < k.len() && (distance(&p, &k[..p.len()]) <= tolerance.min(1) || distance(&p, &k[k.len() - p.len()..]) <= tolerance.min(1))
    })
}

/// Distanza di Levenshtein.
fn distance(a: &[char], b: &[char]) -> usize {
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let current = row[j + 1];
            row[j + 1] = (previous + usize::from(ca != cb)).min(row[j] + 1).min(current + 1);
            previous = current;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_words_are_found_even_misread_or_cut() {
        // Letture vere di ocrs sulle firme dell'utente.
        assert_eq!(pick(&["SIGNATIRE-FIRM"]), [true]);
        assert_eq!(pick(&["ighature/of", "Seafar"]), [true, true]);
        assert_eq!(pick(&["presece", "of", "Medica"]), [true, true, true]);
        // La firma letta come testo non e' una scritta del modulo.
        assert_eq!(pick(&["Porud", "ellbosce"]), [false, false]);
        assert_eq!(pick(&["Mamuorco", "Coenz"]), [false, false]);
        assert_eq!(pick(&["C2SSSTRMNSP"]), [false]);
        // Parole corte da sole non bastano.
        assert_eq!(pick(&["of", "di"]), [false, false]);
    }

    #[test]
    fn levenshtein_distance() {
        let d = |a: &str, b: &str| distance(&a.chars().collect::<Vec<_>>(), &b.chars().collect::<Vec<_>>());
        assert_eq!(d("signatire", "signature"), 1);
        assert_eq!(d("presece", "presence"), 1);
        assert_eq!(d("", "abc"), 3);
        assert_eq!(d("kitten", "sitting"), 3);
    }
}

//! Novita' di ogni versione: compaiono una volta, al primo avvio dopo l'aggiornamento.
//! Il testo sta qui, dentro l'exe: niente rete.

/// Versione e novita', la piu' recente per prima.
const NEWS: &[(&str, &str)] = &[(
    "1.3.0",
    "• Spazio sullo schermo: ora la barra si apre \"Sopra le finestre\" senza spostare le icone del desktop (modificabile in Impostazioni).\n\
     • Scanner di sistema: se sul PC è collegato uno scanner WIA, compare l'icona in alto per scansionare (vetro o alimentatore ADF multipagina) direttamente in Testo (OCR) o Comprimi.\n\
     • Aggiornamenti: banner nella barra quando è disponibile una nuova versione e tasto \"Cerca aggiornamenti\" in Info.\n\
     • Firme: cattura una firma dallo schermo o da una foto; diventa nera su sfondo trasparente. Clic per copiarla, trascinala in Word o Outlook, usala in Strumenti. Gomma e gomma magica, di misura a scelta, per togliere le scritte del modulo.\n\
     • Foto tessera: nuova zona. Trascina una foto: lo sfondo si stacca da solo (bianco o trasparente), formati 35x45, 2x2\" USA e altri con le guide per il volto, messa a fuoco, salvata a 600 DPI. Gli oggetti dietro la persona spariscono da soli; per il resto ci sono gomma e gomma magica.\n\
     • Cartella per zona: in Impostazioni ogni zona può salvare in una cartella sua.",
)];

/// Novita' della versione indicata, se ce ne sono.
pub fn of(version: &str) -> Option<&'static str> {
    NEWS.iter().find(|(v, _)| *v == version).map(|(_, text)| *text)
}

/// Le novita' si mostrano dopo un aggiornamento automatico, oppure se c'era gia' un file di
/// impostazioni e l'ultima versione vista e' diversa (exe sostituito a mano). Mai al primo uso.
pub fn should_show(updated: bool, had_file: bool, seen: Option<&str>, current: &str) -> bool {
    if seen == Some(current) || of(current).is_none() {
        return false;
    }
    updated || had_file
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn news_appear_once_after_an_update_but_not_on_first_use() {
        assert!(should_show(true, false, None, "1.3.0"));
        assert!(should_show(false, true, Some("1.1.1"), "1.3.0"));
        assert!(should_show(false, true, None, "1.3.0"));
        assert!(!should_show(false, false, None, "1.3.0"), "primo uso");
        assert!(!should_show(true, true, Some("1.3.0"), "1.3.0"), "gia' viste");
        assert!(!should_show(true, true, None, "9.9.9"), "versione senza novita'");
        assert!(of(env!("CARGO_PKG_VERSION")).is_some(), "manca il testo delle novita' di questa versione");
    }
}

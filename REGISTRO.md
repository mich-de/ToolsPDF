# Registro della Codebase

```yaml
registro_codebase:
  ultimo_aggiornamento: "2026-11-12 (gomma magica, ocrs, remap percorsi, consiglio OneDrive, cartella fissa di output); prima 2026-11-11 (1.2.0 Firme: passi 1-9 fatti in locale, da provare dal vivo; release non pubblicata)"
  ultima_pulizia: "2026-11-05"

  moduli_esistenti:
    - nome: "tools (Rust) - versione attiva"
      file: "rust/src/tools/mod.rs"
      responsabilita: "tipi comuni, riconoscimento file, smistamento operazioni, salvataggio; OCR in Comprimi/Dividi/Unisci se attivo (set_always_ocr)"
      funzioni_chiave: ["load", "run", "run_with", "run_join", "join_with", "set_always_ocr", "extra_ocr", "compress_pdf", "write_new", "save_pages"]
    - nome: "imaging"
      file: "rust/src/tools/imaging.rs"
      responsabilita: "immagine in PDF di una pagina sotto il limite"
      funzioni_chiave: ["compress_image", "decode_rgb"]
    - nome: "keep_text"
      file: "rust/src/tools/keep_text.rs"
      responsabilita: "riduce un PDF ricomprimendo le immagini interne, senza toccare il testo"
      funzioni_chiave: ["shrink", "load_error"]
    - nome: "raster"
      file: "rust/src/tools/raster.rs"
      responsabilita: "pagine in JPEG con hayro, con testo OCR opzionale"
      funzioni_chiave: ["rasterize", "render_rgb", "with_text_layer", "open"]
    - nome: "ocr"
      file: "rust/src/tools/ocr.rs"
      responsabilita: "OCR di Windows (Windows.Media.Ocr) in un thread MTA"
      funzioni_chiave: ["available", "recognize"]
    - nome: "text_layer"
      file: "rust/src/tools/text_layer.rs"
      responsabilita: "testo invisibile allineato alla pagina, con font TrueType generato"
      funzioni_chiave: ["add_layers", "inherited", "invisible_truetype", "append_layer (q/Q + livello con risorse generiche, usata anche da edit)", "PageGeometry::upright_matrix (usata da edit)"]
    - nome: "textize"
      file: "rust/src/tools/textize.rs"
      responsabilita: "zona Testo; usata anche da Comprimi, Dividi e Unisci con OCR attivo"
      funzioni_chiave: ["from_pdf", "from_image", "add_text (OCR senza riduzione, usata da Dividi)", "has_original_text (testo vero, escluso il nostro font TPDF; usata da Word)"]
    - nome: "docx"
      file: "rust/src/tools/docx.rs"
      responsabilita: "zona Word per scansioni e immagini: docx con pagina come immagine (PNG bianco e nero o JPEG) dietro al testo OCR trasparente (w14:noFill, cornici framePr, fitText); zip minimo scritto a mano"
      funzioni_chiave: ["from_pdf", "from_image", "encode_page", "build", "Zip", "read_entry (solo test)"]
    - nome: "edit"
      file: "rust/src/tools/edit.rs"
      responsabilita: "zona Strumenti (backend): ruota, estrai/elimina, numeri Pagina i di N, timbro diagonale semitrasparente (Helvetica-Bold, ExtGState ca 0.3), firma immagine con SMask, password AES-256 (lopdf V5, chiave da getrandom), sblocco, pagine in JPG 150 DPI; Plan applicato tutto insieme"
      funzioni_chiave: ["Plan", "Signature", "apply", "extract", "to_jpegs", "is_protected", "unlock", "page_count", "prepare", "save_modified", "save_extract", "save_jpegs"]
    - nome: "split"
      file: "rust/src/tools/split.rs"
      responsabilita: "zona Dividi: un PDF per pagina"
      funzioni_chiave: ["split", "DOCUMENT_WIDE e drop_internal_links (usate anche da edit)"]
    - nome: "join"
      file: "rust/src/tools/join.rs"
      responsabilita: "zona Unisci: fonde i documenti in un PDF"
      funzioni_chiave: ["merge"]
    - nome: "office"
      file: "rust/src/tools/office.rs"
      responsabilita: "Word ed Excel in PDF via COM, macro disattivate; PDF con testo in docx con Word (PDF Reflow), con DisableConvertPdfWarning=1 solo durante la conversione e poi ripristinato"
      funzioni_chiave: ["to_pdf", "pdf_to_docx", "word_available", "start_word", "open_in_word", "PdfWarningOff"]
    - nome: "preview"
      file: "rust/src/tools/preview.rs"
      responsabilita: "miniature di tutte le pagine (fino a 40) per il pannello Unisci"
      funzioni_chiave: ["preview"]
    - nome: "settings"
      file: "rust/src/settings.rs"
      responsabilita: "impostazioni in ToolsPDF.ini accanto all'exe, altrimenti in %APPDATA%\\ToolsPDF; preferenze Strumenti (ToolsPrefs: timbro, firma, firma_posizione, firma_larghezza) nello stesso file, riscritto sempre intero; password mai salvata"
      funzioni_chiave: ["load", "save", "save_tools", "current", "tools_prefs", "Settings::parse", "Settings::to_text", "ToolsPrefs::parse", "ToolsPrefs::to_text"]
    - nome: "panel"
      file: "rust/src/ui/panel.rs"
      responsabilita: "pannello accanto alla barra: lista Unisci ordinabile con anteprime, Strumenti (delegato a tools_panel), Firme (delegato a signatures_panel), Info, Impostazioni; drop target proprio (file -> Strumenti, firma, vassoio Unisci, immagine -> Firme)"
      funzioni_chiave: ["show_join", "show_tools", "show_signatures", "toggle", "hide", "reposition", "drop_zone_at", "set_drop_zone", "on_drop", "paint_join", "paint_settings", "paint_info", "draw_thumb/paint_placeholder/paint_remove (usate da tools_panel e signatures_panel)"]
    - nome: "tools_panel"
      file: "rust/src/ui/tools_panel.rs"
      responsabilita: "pannello Strumenti: ToolsDoc (stato documento), layout dal basso (miniature nello spazio libero), selezione con Maiusc, rotazione miniature in memoria, sottopannelli Timbro/Firma/Password, 4 campi EDIT Win32 (attivazione solo sui campi, Invio sblocca), Salva/Estrai/JPG in coda"
      funzioni_chiave: ["ToolsDoc", "DocState", "ToolsHit", "layout", "paint", "hit_test", "on_click", "sync_children", "handle_key", "edit_colors", "load_signature", "save", "remember_prefs", "rotate_thumb"]
    - nome: "signature"
      file: "rust/src/tools/signature.rs"
      responsabilita: "firme: da RGB a nero su trasparente (luminanza, carta al 90 percentile, Otsu col centro del vuoto, rampa alfa spostata dallo slider, via le macchioline 8-connesse, ritaglio + margine), PNG, archivio Firme accanto all exe o in AppData (mai temporanee), formati appunti CF_DIBV5 e CF_DIB su bianco"
      funzioni_chiave: ["decode", "from_bgra", "limit (lato max 1600)", "clean (solo test)", "clean_erased (gomma + origine ritaglio)", "Options {darkness, remove_print, for_print, solid}", "Stroke (magic)", "brush/magic_erase/split_mixed (tubi dei tratti entranti)", "View (fit/zoom_at)", "preview_view", "Prepared/source_rect", "printed_strokes (OCR -> gomma magica)", "prepare(img, options, strokes)", "for_print(img, solid)", "solid_ink", "preview_fit", "remove_lines/remove_dotted_lines/remove_noise (distance_map)", "to_png(img, dpi) + pHYs", "png_dpi", "clean_note/note_of/label", "load", "preview (BGRA su scacchiera, firma centrata)", "DARKNESS_RANGE", "existing_folder", "list", "save", "delete", "dib_v5", "dib_on_white"]
    - nome: "form_text"
      file: "rust/src/tools/form_text.rs"
      responsabilita: "scritte stampate dei moduli accanto alle firme con OCR ocrs (Rust puro, modelli in rust/models dentro l'exe): solo righe con parole dei moduli, tolleranti a errori e tagli"
      funzioni_chiave: ["printed_boxes", "pick", "split", "known", "distance (Levenshtein)", "KEYWORDS"]
    - nome: "signatures_panel"
      file: "rust/src/ui/signatures_panel.rs"
      responsabilita: "pannello Firme: tasto Cattura, elenco firme salvate su scacchiera (clic = copia, trascinamento = drag_file, tasto destro = Copia/Elimina/Apri cartella), bozza con anteprima, gomma magica e normale (Gomma magica/Gomma/Annulla/Fine, anteprima grande con zoom a rotella, passate in pixel sorgente) e slider piu chiara/piu scura, spunte Rendi nera/Stampa/Togli righe, Salva (salva + appunti)/Scarta; immagine trascinata = bozza; menu firme salvate per Strumenti > Firma"
      funzioni_chiave: ["SigHit", "reload", "load_dropped", "hit_test", "paint", "mouse_down/mouse_move/mouse_up", "context_menu", "on_click", "pick_for_tools -> Pick", "is_dragging (drop_target ignora le firme in uscita)", "popup", "label"]
    - nome: "capture"
      file: "rust/src/ui/capture.rs"
      responsabilita: "copia dello schermo virtuale (BitBlt in DIB) prima della finestra di selezione, immagine congelata scurita, rettangolo col trascinamento, Esc/tasto destro annullano (timer GetAsyncKeyState se manca il fuoco); nasconde le finestre indicate durante la copia"
      funzioni_chiave: ["capture(hide) -> Option<RgbImage>"]
    - nome: "clipboard"
      file: "rust/src/ui/clipboard.rs"
      responsabilita: "firma negli appunti: formato registrato PNG, CF_DIBV5 con alfa, CF_DIB su bianco"
      funzioni_chiave: ["copy_signature"]
    - nome: "drag_source"
      file: "rust/src/ui/drag_source.rs"
      responsabilita: "trascinamento in uscita di un file salvato: IDataObject della shell (SHCreateItemFromParsingName + BHID_DataObject) e IDropSource proprio"
      funzioni_chiave: ["drag_file"]
    - nome: "monitors"
      file: "rust/src/ui/monitors.rs"
      responsabilita: "schermi collegati; la barra va sul bordo esterno del desktop o sullo schermo scelto"
      funzioni_chiave: ["list", "target", "containing"]
    - nome: "location"
      file: "rust/src/tools/location.rs"
      responsabilita: "riconosce cartelle temporanee e cache di Outlook"
      funzioni_chiave: ["is_temporary"]
    - nome: "ui"
      file: "rust/src/ui/"
      responsabilita: "appbar, 6 zone (Comprimi, Dividi, Testo, Word, Strumenti, Unisci), vassoio Unisci, tasto cartella, coda di lavoro; thread anteprime con PreviewJob (Tray, Tools, Unlock)"
      funzioni_chiave: ["run", "wnd_proc", "on_drop", "process_one", "process_join", "process_tools", "start_tools", "load_tools_doc", "unlock_tools_doc", "beside", "reveal", "drop_target::register(hwnd, Target)", "dialogs::pick_file", "dialogs::pick_folder(nome cartella completo)", "dialogs::pick_image", "paint::paint", "start_update_checks", "offer_update", "offer_restart"]
    - nome: "update"
      file: "rust/src/update.rs"
      responsabilita: "aggiornamento da GitHub (mich-de/ToolsPDF): WinHTTP col proxy di sistema, /releases/latest/download/version.txt (versione + SHA-256, niente API e niente limiti), exe scaricato accanto a quello in uso (.new), verifica BCrypt SHA-256, scambio .old/.new; unico accesso alla rete; spegnibile con aggiornamenti=mai"
      funzioni_chiave: ["latest", "parse_version_file", "is_newer", "install", "cleanup", "get", "sha256"]
    - nome: "release.ps1"
      file: "release.ps1"
      responsabilita: "pubblica una versione: controlla git pulito e versione nuova, test, build, version.txt con impronta, gh release create vX.Y.Z"
      funzioni_chiave: []

  funzioni_utility:
    - nome: "format_mb"
      file: "rust/src/tools/mod.rs"
      usata_in: ["rust/src/tools/mod.rs", "rust/src/ui/mod.rs"]
    - nome: "save_files / file_stem"
      file: "rust/src/tools/mod.rs"
      usata_in: ["save_pages (Dividi)", "tools/edit.rs save_jpegs"]
    - nome: "bgra_info"
      file: "rust/src/ui/paint.rs"
      usata_in: ["ui/panel.rs draw_thumb", "ui/capture.rs dib"]
    - nome: "portable_dir / appdata_dir"
      file: "rust/src/settings.rs"
      usata_in: ["settings.rs (ini)", "tools/signature.rs (cartella Firme)"]
    - nome: "inherited"
      file: "rust/src/tools/text_layer.rs"
      usata_in: ["rust/src/tools/text_layer.rs", "rust/src/tools/join.rs"]

  pattern_ricorrenti:
    - descrizione: "nessun file nelle cartelle temporanee: gli allegati restano in memoria"
      dove: "ui/drop_target.rs, tools/mod.rs"
    - descrizione: "da Outlook o da una temporanea si chiede dove salvare; i dialoghi rifiutano le temporanee"
      dove: "ui/mod.rs destination_for, ui/dialogs.rs"
    - descrizione: "prima si tenta di mantenere il testo; la conversione in immagini chiede conferma se c'e' testo originale"
      dove: "compress_pdf, textize::from_pdf, split::split"
    - descrizione: "tutti i risultati stanno sotto tools::limit() (1, 2, 5 o 10 MB decimali, da impostazioni); nomi con tools::limit_suffix()"
      dove: "tools/mod.rs save, tools/split.rs"
    - descrizione: "versione da Cargo.toml (CARGO_PKG_VERSION), data di build da build.rs (TOOLSPDF_BUILD_DATE)"
      dove: "ui/mod.rs, build.rs"
    - descrizione: "Office lavora su appoggi nascosti nella cartella di destinazione, cancellati da WorkFile"
      dove: "tools/office.rs"

  file_temporanei:
    - percorso: "D:/ToolsPDF_prova"
      motivo: "cartella delle prove: solo copie e risultati di prova, da eliminare a mano"
      tag: "[TEMPORANEO]"
  file_eliminati_ultima_sessione:
    - "main.go, go.mod, go.sum, internal/" # prima versione in Go, cancellata su richiesta dell'utente (9 nov); resta nella storia git (commit 55fed3c)

  gitignore_pattern_aggiunti: []

  todo_e_criticita:
    - "Firme: testo stampato del modulo tolto a mano con la Gomma (automatico scartato); da provare dal vivo"
    - "Firme 1.2.0: da provare dal vivo (cattura multi-schermo, slider, copia in Word/Outlook, trascinamento, menu destro, Strumenti > Scegli); release.ps1 non ancora eseguito"
    - "barra a 7 zone (ZONE_SIGNATURES=5, ZONE_JOIN=6): su schermi bassi (1080p al 150%) restano circa 140 px per l'elenco dei file"
    - "zona Strumenti (1.1.0): compilata e testata solo a livello backend; pannello, campi di testo, drop della firma e Salva da provare dal vivo"
    - "Strumenti: un'immagine trascinata diventa PDF tramite compress_image, quindi ridotta al limite di peso"
    - "zona Word, PDF con testo: Word ricostruisce bene ma non identico (a capo e salti pagina possono cambiare); Office aggiunge l'etichetta di riservatezza aziendale al salvataggio"
    - "zona Word, PDF misti (testo + scansioni): vanno a Word, le pagine scansionate restano immagini senza OCR"
    - "zona Word: provata con Word vero (19 pagine scansione, 27 pagine testo); da provare dal vivo la quinta zona nella barra"
    - "da provare con Outlook vero il trascinamento di un allegato"
    - "da provare in Adobe Acrobat la selezione del testo OCR: verificata solo con pdfium"
    - "non provati dal vivo: tasto Svuota, tema scuro, scorrimento di una lista lunga"
    - "HEIC non supportato"
    - "OCR: non gestisce pagine scansionate di traverso"
    - "hayro non gestisce blending e knockout: alcune pagine convertite possono differire"
    - "documenti lunghi di sole scansioni: per stare sotto 1 MB possono scendere a 60 DPI o meno"
    - "Office puo' creare file temporanei suoi, fuori dal controllo dell'app"
```

## Compilare

Dalla cartella `rust`, con `%USERPROFILE%\.cargo\bin` e il `bin` di llvm-mingw nel PATH:

```
cargo build --release
```

L'exe esce in `rust\target\x86_64-pc-windows-gnullvm\release\toolspdf.exe`.

## Provare

```
cargo test
set TOOLSPDF_CORPUS=cartella con PDF
cargo test --release corpus -- --ignored --nocapture
set TOOLSPDF_OFFICE=cartella normale con relazione.docx e conti.xlsx
cargo test real_office -- --ignored --nocapture
```

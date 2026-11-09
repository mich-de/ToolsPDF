# Tools PDF

Barra laterale per Windows, portable: un solo `ToolsPDF.exe`, nessuna installazione, solo DLL di sistema.
Si trascinano i file sulle zone della barra.

| Zona | Cosa fa |
|---|---|
| Comprimi | Porta PDF, immagini, Word ed Excel sotto il limite scelto (1, 2, 5 o 10 MB). |
| Dividi | Un PDF per pagina nella cartella `nome_pagine`. |
| Testo | OCR di Windows: le scansioni diventano selezionabili e ricercabili. |
| Word | PDF e scansioni in `.docx`. |
| Strumenti | Ruota, estrai o elimina pagine; numeri di pagina, timbro, firma come immagine, password AES-256; pagine in JPG. |
| Unisci | Vassoio di file da ordinare e unire in un PDF. |

I file si salvano accanto all'originale. Per gli allegati di Outlook e i file nelle cartelle temporanee chiede dove salvare: nulla finisce in una cartella temporanea.

## Scaricare

Ultima versione: [Releases](https://github.com/mich-de/ToolsPDF/releases/latest), file `ToolsPDF.exe`.

## Aggiornamenti

La barra controlla una volta al giorno se su GitHub c'e' una versione nuova e chiede prima di scaricarla.
Il file viene verificato con l'impronta SHA-256 di `version.txt` e sostituisce l'exe; la nuova versione parte al riavvio della barra.
Nessun documento viene inviato in rete. Per spegnere il controllo: `aggiornamenti=mai` in `ToolsPDF.ini`.

## Compilare

Rust con target `x86_64-pc-windows-gnullvm` e [llvm-mingw](https://github.com/mstorsjo/llvm-mingw) nel `PATH`. Dalla cartella `rust`:

```
cargo test
cargo build --release
```

Struttura del codice e decisioni: [REGISTRO.md](REGISTRO.md).

## Pubblicare una versione

1. Alzare `version` in `rust/Cargo.toml` e fare commit e push.
2. `powershell -File release.ps1 -Notes "cosa cambia"`: test, build, `version.txt` con l'impronta, release `vX.Y.Z` su GitHub.

La cartella `internal/` e `main.go` sono la prima versione in Go (solo compressione), non piu' sviluppata.

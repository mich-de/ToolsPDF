// Package compress riduce immagini e PDF sotto TargetBytes e produce sempre un PDF.
package compress

import (
	"bytes"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// TargetBytes e' il limite in decimale: 1 000 000 byte stanno sotto "1 MB" anche
// per i portali che contano 1 048 576.
const TargetBytes = 1_000_000

// ErrCancelled segnala che l'utente ha rifiutato la conversione in immagini.
var ErrCancelled = errors.New("operazione annullata")

// Progress riceve l'avanzamento (0..1) e una breve descrizione della fase.
type Progress func(pct float64, msg string)

// Confirm chiede se convertire le pagine in immagini. keptBytes e' il minimo
// raggiunto mantenendo il testo, 0 se il PDF non si puo' ottimizzare cosi'.
type Confirm func(keptBytes int64) bool

// Result descrive il file prodotto.
type Result struct {
	Output  string
	InSize  int64
	OutSize int64
	Note    string
}

// Process comprime path e scrive <nome>_1MB.pdf nella stessa cartella.
func Process(path string, progress Progress, confirm Confirm) (res Result, err error) {
	defer func() {
		if r := recover(); r != nil {
			err = fmt.Errorf("errore interno: %v", r)
		}
	}()

	info, err := os.Stat(path)
	if err != nil {
		return res, fmt.Errorf("file non leggibile: %w", err)
	}
	if info.IsDir() {
		return res, errors.New("le cartelle non sono supportate")
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return res, fmt.Errorf("file non leggibile: %w", err)
	}

	var out []byte
	var note string
	if isPDF(data) {
		out, note, err = compressPDF(data, progress, confirm)
	} else {
		out, note, err = compressImage(data, progress)
	}
	if err != nil {
		return res, err
	}
	if len(out) > TargetBytes {
		return res, fmt.Errorf("impossibile scendere sotto 1 MB (minimo %s)", FormatMB(int64(len(out))))
	}

	progress(0.98, "Salvataggio…")
	output, err := writeNew(path, out)
	if err != nil {
		return res, fmt.Errorf("salvataggio non riuscito: %w", err)
	}
	return Result{Output: output, InSize: info.Size(), OutSize: int64(len(out)), Note: note}, nil
}

func isPDF(data []byte) bool {
	head := data
	if len(head) > 1024 {
		head = head[:1024]
	}
	return bytes.Contains(head, []byte("%PDF-"))
}

// writeNew crea <nome>_1MB.pdf senza mai sovrascrivere: se esiste aggiunge un numero.
func writeNew(input string, data []byte) (string, error) {
	base := strings.TrimSuffix(input, filepath.Ext(input)) + "_1MB"
	for i := 1; i < 10000; i++ {
		name := base + ".pdf"
		if i > 1 {
			name = fmt.Sprintf("%s (%d).pdf", base, i)
		}
		f, err := os.OpenFile(name, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o644)
		if errors.Is(err, os.ErrExist) {
			continue
		}
		if err != nil {
			return "", err
		}
		_, err = f.Write(data)
		if cerr := f.Close(); err == nil {
			err = cerr
		}
		if err != nil {
			os.Remove(name)
			return "", err
		}
		return name, nil
	}
	return "", errors.New("troppi file con lo stesso nome")
}

// FormatMB formatta i byte in MB decimali con la virgola.
func FormatMB(n int64) string {
	return strings.Replace(fmt.Sprintf("%.2f MB", float64(n)/1e6), ".", ",", 1)
}

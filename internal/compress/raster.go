package compress

import (
	"errors"
	"fmt"
	"io"
	"math"
	"sync"
	"time"

	"github.com/klippa-app/go-pdfium"
	"github.com/klippa-app/go-pdfium/enums"
	"github.com/klippa-app/go-pdfium/references"
	"github.com/klippa-app/go-pdfium/requests"
	"github.com/klippa-app/go-pdfium/webassembly"
)

var (
	rasterDPIs      = []int{200, 150, 120, 100, 85, 72, 60, 50}
	rasterQualities = []int{80, 60, 40, 30}

	poolOnce sync.Once
	pool     pdfium.Pool
	poolErr  error
)

// pdfiumPool avvia il motore WebAssembly solo quando serve davvero: costa qualche secondo.
func pdfiumPool() (pdfium.Pool, error) {
	poolOnce.Do(func() {
		// Stdout e Stderr espliciti: in un exe senza console quelli di sistema non sono validi.
		pool, poolErr = webassembly.Init(webassembly.Config{MinIdle: 1, MaxIdle: 1, MaxTotal: 1, Stdout: io.Discard, Stderr: io.Discard})
	})
	return pool, poolErr
}

// rasterize converte ogni pagina in JPEG, scendendo di DPI finche' il PDF sta nel limite.
func rasterize(data []byte, progress Progress) ([]byte, int, error) {
	progress(0.05, "Avvio motore PDFâ€¦")
	p, err := pdfiumPool()
	if err != nil {
		return nil, 0, fmt.Errorf("motore PDF non disponibile: %w", err)
	}
	instance, err := p.GetInstance(time.Minute)
	if err != nil {
		return nil, 0, fmt.Errorf("motore PDF non disponibile: %w", err)
	}
	defer instance.Close()

	doc, err := instance.OpenDocument(&requests.OpenDocument{File: &data})
	if err != nil {
		if isPasswordError(err) {
			return nil, 0, errors.New("PDF protetto da password")
		}
		return nil, 0, errors.New("PDF danneggiato o non leggibile")
	}
	defer instance.FPDF_CloseDocument(&requests.FPDF_CloseDocument{Document: doc.Document})

	count, err := instance.FPDF_GetPageCount(&requests.FPDF_GetPageCount{Document: doc.Document})
	if err != nil {
		return nil, 0, errors.New("PDF danneggiato o non leggibile")
	}
	if count.PageCount == 0 {
		return nil, 0, errors.New("il PDF non ha pagine")
	}

	start := startDPI(count.PageCount)
	for i, dpi := range rasterDPIs {
		if dpi > start {
			continue
		}
		pct := 0.1 + 0.85*float64(i)/float64(len(rasterDPIs))
		pages, err := renderPages(instance, doc.Document, count.PageCount, dpi, func(page int) {
			progress(pct, fmt.Sprintf("Pagina %d di %d a %d DPIâ€¦", page+1, count.PageCount, dpi))
		})
		if err != nil {
			return nil, 0, err
		}
		if pages == nil {
			continue
		}
		out, err := writeJPEGPDF(pages)
		if err != nil {
			return nil, 0, err
		}
		if len(out) <= TargetBytes {
			return out, dpi, nil
		}
	}
	return nil, 0, fmt.Errorf("troppe pagine (%d): sotto 1 MB non resterebbero leggibili", count.PageCount)
}

// startDPI evita passate inutili sui documenti lunghi. 0,03 byte/pixel e' una
// stima prudente per pagine A4 al livello di qualita' piu' basso.
func startDPI(pages int) int {
	perPage := float64(TargetBytes) / float64(pages)
	dpi := int(math.Sqrt(perPage / (0.03 * 8.27 * 11.69)))
	return min(max(dpi, rasterDPIs[len(rasterDPIs)-1]), rasterDPIs[0])
}

// minQuality: ad alti DPI meglio scendere di risoluzione che sporcare il testo.
func minQuality(dpi int) int {
	switch {
	case dpi > 100:
		return 60
	case dpi >= 72:
		return 40
	}
	return 30
}

// renderPages tiene in memoria solo i JPEG, non i pixel. Restituisce nil se a
// questi DPI il documento non puo' stare nel limite.
func renderPages(instance pdfium.Pdfium, doc references.FPDF_DOCUMENT, pageCount, dpi int, onPage func(int)) ([]jpegPage, error) {
	budget := TargetBytes - 1024 - pageOverhead*pageCount
	floor := minQuality(dpi)
	encoded := make([][][]byte, pageCount)
	pages := make([]jpegPage, pageCount)
	totals := make([]int, len(rasterQualities))

	for i := range pageCount {
		onPage(i)
		render, err := instance.RenderPageInDPI(&requests.RenderPageInDPI{
			DPI:         dpi,
			RenderFlags: enums.FPDF_RENDER_FLAG_ANNOT,
			Page:        requests.Page{ByIndex: &requests.PageByIndex{Document: doc, Index: i}},
		})
		if err != nil {
			return nil, fmt.Errorf("pagina %d non renderizzabile: %w", i+1, err)
		}
		img := render.Result.RenderedImage
		if render.Result.HasTransparency {
			img = flatten(img)
		}
		pages[i].widthPt = float64(render.Result.Width) * 72 / float64(dpi)
		pages[i].heightPt = float64(render.Result.Height) * 72 / float64(dpi)

		encoded[i] = make([][]byte, len(rasterQualities))
		errs := make([]error, len(rasterQualities))
		var wg sync.WaitGroup
		for q, quality := range rasterQualities {
			wg.Go(func() { encoded[i][q], errs[q] = encodeJPEG(img, quality) })
		}
		wg.Wait()
		render.Cleanup()
		if err := errors.Join(errs...); err != nil {
			return nil, err
		}
		for q := range rasterQualities {
			totals[q] += len(encoded[i][q])
		}
		// Basta il livello piu' leggero per sapere che qui non ci si sta.
		if totals[len(totals)-1] > budget {
			return nil, nil
		}
	}

	for q, quality := range rasterQualities {
		if quality >= floor && totals[q] <= budget {
			for i := range pages {
				pages[i].data = encoded[i][q]
			}
			return pages, nil
		}
	}
	return nil, nil
}

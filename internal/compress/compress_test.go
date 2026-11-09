package compress

import (
	"bytes"
	"context"
	"fmt"
	"image"
	"image/color"
	"image/jpeg"
	"image/png"
	"math/rand/v2"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/klippa-app/go-pdfium/requests"
	"github.com/pdfcpu/pdfcpu/pkg/api"
	"github.com/pdfcpu/pdfcpu/pkg/pdfcpu/model"
)

func noop(float64, string) {}

func noisy(w, h int) *image.RGBA {
	img := image.NewRGBA(image.Rect(0, 0, w, h))
	r := rand.New(rand.NewPCG(1, 2))
	for i := range img.Pix {
		if i%4 == 3 {
			img.Pix[i] = 255
		} else {
			img.Pix[i] = byte(r.IntN(256))
		}
	}
	return img
}

// buildPDF crea un PDF di prova: testo reale piu' immagine o contenuto incomprimibile.
func buildPDF(t *testing.T, text string, jpegData []byte, filler int) []byte {
	t.Helper()
	cfg, err := jpeg.DecodeConfig(bytes.NewReader(jpegData))
	if err != nil {
		t.Fatal(err)
	}
	dict := fmt.Sprintf("/Width %d/Height %d/ColorSpace/DeviceRGB/BitsPerComponent 8/Filter/DCTDecode", cfg.Width, cfg.Height)
	return buildPDFImage(t, text, dict, jpegData, filler)
}

// buildPDFImage accetta un'immagine gia' codificata con il dizionario che la descrive.
func buildPDFImage(t *testing.T, text, imageDict string, imageData []byte, filler int) []byte {
	t.Helper()
	var b bytes.Buffer
	var offs []int
	add := func(s string) {
		offs = append(offs, b.Len())
		fmt.Fprintf(&b, "%d 0 obj\n%s\nendobj\n", len(offs), s)
	}
	b.WriteString("%PDF-1.5\n")
	add("<</Type/Catalog/Pages 2 0 R>>")
	add("<</Type/Pages/Kids[3 0 R]/Count 1>>")
	add("<</Type/Page/Parent 2 0 R/MediaBox[0 0 595 842]/Resources<</Font<</F 5 0 R>>/XObject<</I 6 0 R>>>>/Contents 4 0 R>>")
	content := fmt.Sprintf("BT /F 18 Tf 50 780 Td (%s) Tj ET q 400 0 0 500 90 100 cm /I Do Q", text)
	if filler > 0 {
		r := rand.New(rand.NewPCG(3, 4))
		var f strings.Builder
		for range filler {
			fmt.Fprintf(&f, " BT /F 1 Tf 1 1 Td <%016x> Tj ET", r.Uint64())
		}
		content += f.String()
	}
	add(fmt.Sprintf("<</Length %d>>\nstream\n%s\nendstream", len(content), content))
	add("<</Type/Font/Subtype/Type1/BaseFont/Helvetica>>")
	add(fmt.Sprintf("<</Type/XObject/Subtype/Image%s/Length %d>>\nstream\n%s\nendstream", imageDict, len(imageData), imageData))
	x := b.Len()
	fmt.Fprintf(&b, "xref\n0 %d\n0000000000 65535 f \n", len(offs)+1)
	for _, o := range offs {
		fmt.Fprintf(&b, "%010d 00000 n \n", o)
	}
	fmt.Fprintf(&b, "trailer\n<</Size %d/Root 1 0 R>>\nstartxref\n%d\n%%%%EOF\n", len(offs)+1, x)
	return b.Bytes()
}

func pageTexts(t *testing.T, data []byte) (pages int, text string, side int) {
	t.Helper()
	p, err := pdfiumPool()
	if err != nil {
		t.Fatal(err)
	}
	in, err := p.GetInstance(time.Minute)
	if err != nil {
		t.Fatal(err)
	}
	defer in.Close()
	doc, err := in.OpenDocument(&requests.OpenDocument{File: &data})
	if err != nil {
		t.Fatalf("il risultato non si apre: %v", err)
	}
	defer in.FPDF_CloseDocument(&requests.FPDF_CloseDocument{Document: doc.Document})
	c, err := in.FPDF_GetPageCount(&requests.FPDF_GetPageCount{Document: doc.Document})
	if err != nil {
		t.Fatal(err)
	}
	for i := range c.PageCount {
		pt, err := in.GetPageText(&requests.GetPageText{Page: requests.Page{ByIndex: &requests.PageByIndex{Document: doc.Document, Index: i}}})
		if err != nil {
			t.Fatal(err)
		}
		text += pt.Text
	}
	return c.PageCount, text, 0
}

func writeTemp(t *testing.T, name string, data []byte) string {
	t.Helper()
	path := filepath.Join(t.TempDir(), name)
	if err := os.WriteFile(path, data, 0o644); err != nil {
		t.Fatal(err)
	}
	return path
}

func checkResult(t *testing.T, res Result, input string) []byte {
	t.Helper()
	if filepath.Dir(res.Output) != filepath.Dir(input) {
		t.Fatalf("cartella diversa: %s", res.Output)
	}
	out, err := os.ReadFile(res.Output)
	if err != nil {
		t.Fatal(err)
	}
	if len(out) > TargetBytes {
		t.Fatalf("output troppo grande: %d", len(out))
	}
	return out
}

func TestLargePNGBecomesSmallPDF(t *testing.T) {
	var b bytes.Buffer
	if err := png.Encode(&b, noisy(3000, 2000)); err != nil {
		t.Fatal(err)
	}
	in := writeTemp(t, "foto.png", b.Bytes())
	res, err := Process(in, noop, nil)
	if err != nil {
		t.Fatal(err)
	}
	if !strings.HasSuffix(res.Output, "foto_1MB.pdf") {
		t.Fatalf("nome errato: %s", res.Output)
	}
	out := checkResult(t, res, in)
	if pages, _, _ := pageTexts(t, out); pages != 1 {
		t.Fatalf("pagine: %d", pages)
	}
}

func TestExistingOutputIsNotOverwritten(t *testing.T) {
	var b bytes.Buffer
	if err := png.Encode(&b, noisy(50, 50)); err != nil {
		t.Fatal(err)
	}
	in := writeTemp(t, "a.png", b.Bytes())
	first, err := Process(in, noop, nil)
	if err != nil {
		t.Fatal(err)
	}
	second, err := Process(in, noop, nil)
	if err != nil {
		t.Fatal(err)
	}
	if first.Output == second.Output || !strings.HasSuffix(second.Output, "a_1MB (2).pdf") {
		t.Fatalf("output: %s, %s", first.Output, second.Output)
	}
}

func TestPDFWithHeavyImageKeepsTextWithoutAsking(t *testing.T) {
	var b bytes.Buffer
	if err := jpeg.Encode(&b, noisy(2600, 2000), &jpeg.Options{Quality: 90}); err != nil {
		t.Fatal(err)
	}
	src := buildPDF(t, "TestoSelezionabile", b.Bytes(), 0)
	in := writeTemp(t, "doc.pdf", src)
	res, err := Process(in, noop, func(int64) bool {
		t.Error("non doveva chiedere conferma")
		return false
	})
	if err != nil {
		t.Fatal(err)
	}
	out := checkResult(t, res, in)
	if _, text, _ := pageTexts(t, out); !strings.Contains(text, "TestoSelezionabile") {
		t.Fatalf("testo perso: %q", text)
	}
}

func TestPDFWithRawImageKeepsTextWithoutAsking(t *testing.T) {
	const w, h = 2000, 1500
	pixels := noisy(w, h).Pix
	rgb := make([]byte, 0, w*h*3)
	for i := 0; i < len(pixels); i += 4 {
		rgb = append(rgb, pixels[i], pixels[i+1], pixels[i+2])
	}
	dict := fmt.Sprintf("/Width %d/Height %d/ColorSpace/DeviceRGB/BitsPerComponent 8", w, h)
	in := writeTemp(t, "raw.pdf", buildPDFImage(t, "TestoGrezzo", dict, rgb, 0))
	res, err := Process(in, noop, func(int64) bool {
		t.Error("non doveva chiedere conferma")
		return false
	})
	if err != nil {
		t.Fatal(err)
	}
	out := checkResult(t, res, in)
	if _, text, _ := pageTexts(t, out); !strings.Contains(text, "TestoGrezzo") {
		t.Fatalf("testo perso: %q", text)
	}
}

func TestUncompressedContentIsDeflatedWithoutAsking(t *testing.T) {
	var b bytes.Buffer
	if err := jpeg.Encode(&b, noisy(40, 40), nil); err != nil {
		t.Fatal(err)
	}
	in := writeTemp(t, "testo.pdf", buildPDF(t, "SoloTesto", b.Bytes(), 80000))
	res, err := Process(in, noop, func(int64) bool {
		t.Error("non doveva chiedere conferma")
		return false
	})
	if err != nil {
		t.Fatal(err)
	}
	out := checkResult(t, res, in)
	if _, text, _ := pageTexts(t, out); !strings.Contains(text, "SoloTesto") {
		t.Fatalf("testo perso: %q", text)
	}
}

func heavyContentPDF(t *testing.T) []byte {
	var b bytes.Buffer
	if err := jpeg.Encode(&b, noisy(40, 40), nil); err != nil {
		t.Fatal(err)
	}
	return buildPDF(t, "ContenutoPesante", b.Bytes(), 200000)
}

func TestHeavyContentAsksAndRespectsNo(t *testing.T) {
	in := writeTemp(t, "pesante.pdf", heavyContentPDF(t))
	asked := false
	_, err := Process(in, noop, func(kept int64) bool {
		asked = true
		if kept <= TargetBytes {
			t.Errorf("kept=%d", kept)
		}
		return false
	})
	if !asked || err != ErrCancelled {
		t.Fatalf("asked=%v err=%v", asked, err)
	}
	if _, statErr := os.Stat(filepath.Join(filepath.Dir(in), "pesante_1MB.pdf")); statErr == nil {
		t.Fatal("non doveva creare file")
	}
}

func TestHeavyContentRasterizesOnYes(t *testing.T) {
	in := writeTemp(t, "pesante.pdf", heavyContentPDF(t))
	res, err := Process(in, noop, func(int64) bool { return true })
	if err != nil {
		t.Fatal(err)
	}
	out := checkResult(t, res, in)
	if pages, _, _ := pageTexts(t, out); pages != 1 {
		t.Fatalf("pagine: %d", pages)
	}
	if !strings.Contains(res.Note, "DPI") {
		t.Fatalf("nota: %q", res.Note)
	}
}

func TestUnsupportedFile(t *testing.T) {
	in := writeTemp(t, "x.txt", []byte("ciao"))
	if _, err := Process(in, noop, nil); err == nil || !strings.Contains(err.Error(), "formato non supportato") {
		t.Fatalf("err=%v", err)
	}
}

func TestOrientation(t *testing.T) {
	src := image.NewRGBA(image.Rect(0, 0, 2, 1))
	src.Set(0, 0, color.RGBA{255, 0, 0, 255})
	got := applyOrientation(src, 6)
	if got.Bounds().Dx() != 1 || got.Bounds().Dy() != 2 {
		t.Fatalf("dimensioni %v", got.Bounds())
	}
	if r, _, _, _ := got.At(0, 0).RGBA(); r>>8 != 255 {
		t.Fatal("rotazione errata")
	}
}

func TestPasswordProtectedPDFIsReported(t *testing.T) {
	var b bytes.Buffer
	if err := jpeg.Encode(&b, noisy(2600, 2000), &jpeg.Options{Quality: 90}); err != nil {
		t.Fatal(err)
	}
	plain := writeTemp(t, "plain.pdf", buildPDF(t, "Segreto", b.Bytes(), 0))
	locked := filepath.Join(filepath.Dir(plain), "locked.pdf")
	conf := model.NewAESConfiguration("utente", "proprietario", 256)
	if err := api.EncryptFile(context.Background(), plain, locked, conf); err != nil {
		t.Fatal(err)
	}
	_, err := Process(locked, noop, func(int64) bool {
		t.Error("non doveva chiedere conferma")
		return false
	})
	if err == nil || !strings.Contains(err.Error(), "password") {
		t.Fatalf("err=%v", err)
	}
}

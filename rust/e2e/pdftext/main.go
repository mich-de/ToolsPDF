// Strumento di verifica: estrae il testo con pdfium e disegna i riquadri delle parole sulla pagina.
package main

import (
	"fmt"
	"image"
	"image/color"
	"image/png"
	"io"
	"os"
	"strconv"
	"strings"
	"time"

	"github.com/klippa-app/go-pdfium/enums"
	"github.com/klippa-app/go-pdfium/requests"
	"github.com/klippa-app/go-pdfium/webassembly"
)

func main() {
	data, err := os.ReadFile(os.Args[1])
	if err != nil {
		panic(err)
	}
	pageIndex, _ := strconv.Atoi(os.Args[2])
	pool, err := webassembly.Init(webassembly.Config{MinIdle: 1, MaxIdle: 1, MaxTotal: 1, Stdout: io.Discard, Stderr: io.Discard})
	if err != nil {
		panic(err)
	}
	in, err := pool.GetInstance(time.Minute)
	if err != nil {
		panic(err)
	}
	doc, err := in.OpenDocument(&requests.OpenDocument{File: &data})
	if err != nil {
		panic(err)
	}
	count, _ := in.FPDF_GetPageCount(&requests.FPDF_GetPageCount{Document: doc.Document})
	fmt.Println("pagine:", count.PageCount)
	page := requests.Page{ByIndex: &requests.PageByIndex{Document: doc.Document, Index: pageIndex}}
	text, err := in.GetPageText(&requests.GetPageText{Page: page})
	if err != nil {
		panic(err)
	}
	t := strings.ReplaceAll(text.Text, "\r\n", " | ")
	if len(t) > 700 {
		t = t[:700]
	}
	fmt.Println("testo:", t)

	const dpi = 110
	render, err := in.RenderPageInDPI(&requests.RenderPageInDPI{DPI: dpi, RenderFlags: enums.FPDF_RENDER_FLAG_ANNOT, Page: page})
	if err != nil {
		panic(err)
	}
	img := image.NewRGBA(render.Result.Image.Bounds())
	copy(img.Pix, render.Result.Image.Pix)
	structured, err := in.GetPageTextStructured(&requests.GetPageTextStructured{
		Page: page, Mode: requests.GetPageTextStructuredModeRects, PixelPositions: requests.GetPageTextStructuredPixelPositions{Calculate: true, DPI: dpi},
	})
	if err != nil {
		panic(err)
	}
	fmt.Println("riquadri:", len(structured.Rects))
	red := color.RGBA{255, 0, 0, 255}; n := 0
	for _, r := range structured.Rects {
		if false {
			continue
		}
		k := float64(dpi) / 72; h := float64(img.Bounds().Dy()); pp := r.PointPosition; if n < 4 { fmt.Printf("rect %q L%.1f T%.1f R%.1f B%.1f\n", r.Text, pp.Left, pp.Top, pp.Right, pp.Bottom) }; n++; box(img, pp.Left*k, h-pp.Top*k, pp.Right*k, h-pp.Bottom*k, red)
	}
	f, _ := os.Create(os.Args[3])
	defer f.Close()
	png.Encode(f, img)
}

func box(img *image.RGBA, l, t, r, b float64, c color.RGBA) {
	x0, y0, x1, y1 := int(l), int(t), int(r), int(b)
	for x := x0; x <= x1; x++ {
		img.SetRGBA(x, y0, c)
		img.SetRGBA(x, y1, c)
	}
	for y := y0; y <= y1; y++ {
		img.SetRGBA(x0, y, c)
		img.SetRGBA(x1, y, c)
	}
}

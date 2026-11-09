package compress

import (
	"bytes"
	"fmt"
	"image/color"
	"image/jpeg"
)

// jpegPage e' una pagina PDF interamente occupata da un JPEG.
type jpegPage struct {
	data     []byte
	widthPt  float64
	heightPt float64
}

// pageOverhead stima i byte di struttura PDF per pagina.
const pageOverhead = 400

// writeJPEGPDF assembla un PDF minimale con un JPEG a pagina, senza ricodificarli.
func writeJPEGPDF(pages []jpegPage) ([]byte, error) {
	var b bytes.Buffer
	offsets := make([]int, 0, 2+3*len(pages))
	obj := func(body func()) {
		offsets = append(offsets, b.Len())
		fmt.Fprintf(&b, "%d 0 obj\n", len(offsets))
		body()
		b.WriteString("\nendobj\n")
	}

	b.WriteString("%PDF-1.5\n%\xE2\xE3\xCF\xD3\n")
	obj(func() { b.WriteString("<</Type/Catalog/Pages 2 0 R>>") })
	obj(func() {
		b.WriteString("<</Type/Pages/Kids[")
		for i := range pages {
			fmt.Fprintf(&b, "%d 0 R ", 3+3*i)
		}
		fmt.Fprintf(&b, "]/Count %d>>", len(pages))
	})
	for i, p := range pages {
		cfg, err := jpeg.DecodeConfig(bytes.NewReader(p.data))
		if err != nil {
			return nil, fmt.Errorf("JPEG non valido: %w", err)
		}
		cs := "DeviceRGB"
		if cfg.ColorModel == color.GrayModel {
			cs = "DeviceGray"
		}
		first := 3 + 3*i
		obj(func() {
			fmt.Fprintf(&b, "<</Type/Page/Parent 2 0 R/MediaBox[0 0 %.2f %.2f]/Resources<</XObject<</I %d 0 R>>>>/Contents %d 0 R>>",
				p.widthPt, p.heightPt, first+1, first+2)
		})
		obj(func() {
			fmt.Fprintf(&b, "<</Type/XObject/Subtype/Image/Width %d/Height %d/ColorSpace/%s/BitsPerComponent 8/Filter/DCTDecode/Length %d>>\nstream\n",
				cfg.Width, cfg.Height, cs, len(p.data))
			b.Write(p.data)
			b.WriteString("\nendstream")
		})
		content := fmt.Sprintf("q %.2f 0 0 %.2f 0 0 cm /I Do Q", p.widthPt, p.heightPt)
		obj(func() {
			fmt.Fprintf(&b, "<</Length %d>>\nstream\n%s\nendstream", len(content), content)
		})
	}

	xref := b.Len()
	fmt.Fprintf(&b, "xref\n0 %d\n0000000000 65535 f \n", len(offsets)+1)
	for _, o := range offsets {
		fmt.Fprintf(&b, "%010d 00000 n \n", o)
	}
	fmt.Fprintf(&b, "trailer\n<</Size %d/Root 1 0 R>>\nstartxref\n%d\n%%%%EOF\n", len(offsets)+1, xref)
	return b.Bytes(), nil
}

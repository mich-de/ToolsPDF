package compress

import (
	"bytes"
	"compress/zlib"
	"context"
	"errors"
	"fmt"
	"image"
	"image/jpeg"
	"slices"
	"strings"

	"github.com/pdfcpu/pdfcpu/pkg/api"
	"github.com/pdfcpu/pdfcpu/pkg/pdfcpu/model"
	"github.com/pdfcpu/pdfcpu/pkg/pdfcpu/types"
)

func init() {
	// App portable: pdfcpu non deve creare cartelle di configurazione nel profilo.
	api.DisableConfigDir()
}

// imageRung e' un livello di riduzione delle immagini interne.
type imageRung struct {
	side    int
	quality int
}

// imageRungs va dal piu' fedele al piu' leggero.
var imageRungs = []imageRung{
	{2400, 75}, {2000, 70}, {1600, 65}, {1280, 60}, {1024, 50}, {800, 45}, {640, 40}, {480, 30},
}

func compressPDF(data []byte, progress Progress, confirm Confirm) ([]byte, string, error) {
	if len(data) <= TargetBytes {
		return data, "già sotto 1 MB", nil
	}
	kept, err := shrinkKeepingText(data, progress)
	if err == nil && len(kept) <= TargetBytes {
		return kept, "testo mantenuto", nil
	}
	if err != nil && isPasswordError(err) {
		return nil, "", errors.New("PDF protetto da password")
	}
	if !confirm(int64(len(kept))) {
		return nil, "", ErrCancelled
	}
	out, dpi, err := rasterize(data, progress)
	if err != nil {
		return nil, "", err
	}
	return out, fmt.Sprintf("%d DPI, testo non selezionabile", dpi), nil
}

func isPasswordError(err error) bool {
	return strings.Contains(strings.ToLower(err.Error()), "password")
}

// shrinkKeepingText ottimizza il PDF e riduce le immagini interne senza toccare
// il testo. Restituisce il risultato migliore, anche se ancora sopra il limite.
func shrinkKeepingText(data []byte, progress Progress) (best []byte, err error) {
	defer func() {
		if r := recover(); r != nil {
			err = fmt.Errorf("PDF non ottimizzabile: %v", r)
		}
	}()

	progress(0.05, "Ottimizzazione PDF…")
	base, err := rewritePDF(data, nil)
	if err != nil {
		return nil, err
	}
	if len(base) <= TargetBytes {
		return base, nil
	}

	// Il livello piu' basso dice subito se le immagini possono bastare.
	last := len(imageRungs) - 1
	progress(0.15, "Riduzione immagini…")
	smallest, err := rewritePDF(base, &imageRungs[last])
	if err != nil {
		return base, err
	}
	best = base
	if len(smallest) < len(best) {
		best = smallest
	}
	if len(smallest) > TargetBytes {
		return best, nil
	}

	// Ricerca binaria del livello piu' fedele che sta sotto il limite.
	lo, hi := 0, last-1
	step := 0
	for lo <= hi {
		step++
		progress(0.15+0.75*float64(step)/4, "Riduzione immagini…")
		mid := (lo + hi) / 2
		out, err := rewritePDF(base, &imageRungs[mid])
		if err == nil && len(out) <= TargetBytes {
			best, hi = out, mid-1
		} else {
			lo = mid + 1
		}
	}
	return best, nil
}

// rewritePDF rilegge il PDF, applica il livello richiesto e lo riscrive ottimizzato.
func rewritePDF(data []byte, rung *imageRung) ([]byte, error) {
	conf := model.NewDefaultConfiguration()
	conf.Cmd = model.OPTIMIZE
	bg := context.Background()
	ctx, err := api.ReadValidateAndOptimize(bg, bytes.NewReader(data), conf, nil)
	if err != nil {
		return nil, err
	}
	numbers := make([]int, 0, len(ctx.Table))
	for nr := range ctx.Table {
		numbers = append(numbers, nr)
	}
	slices.Sort(numbers)
	for _, nr := range numbers {
		entry := ctx.Table[nr]
		deflateEntry(entry)
		if rung != nil {
			reencodeEntry(ctx, entry, *rung)
		}
	}
	var b bytes.Buffer
	if err := api.Write(bg, ctx, &b, conf); err != nil {
		return nil, err
	}
	return b.Bytes(), nil
}

// deflateEntry comprime i flussi senza filtro: pdfcpu li riscriverebbe tali e quali.
// I metadati XMP restano leggibili, come chiede lo standard PDF/A.
func deflateEntry(entry *model.XRefTableEntry) {
	if entry == nil || entry.Free {
		return
	}
	sd, ok := entry.Object.(types.StreamDict)
	if !ok || len(sd.FilterPipeline) > 0 || sd.HasEntry("Filter") || len(sd.Raw) == 0 {
		return
	}
	if t := sd.Type(); t != nil && *t == "Metadata" {
		return
	}
	var b bytes.Buffer
	w, err := zlib.NewWriterLevel(&b, zlib.BestCompression)
	if err != nil {
		return
	}
	if _, err := w.Write(sd.Raw); err != nil || w.Close() != nil {
		return
	}
	compressed := b.Bytes()
	if len(compressed) >= len(sd.Raw) {
		return
	}
	sd.Raw = compressed
	sd.Content = nil
	sd.FilterPipeline = []types.PDFFilter{{Name: "FlateDecode"}}
	length := int64(len(compressed))
	sd.StreamLength = &length
	sd.StreamLengthObjNr = nil
	sd.Update("Filter", types.Name("FlateDecode"))
	sd.Update("Length", types.Integer(length))
	entry.Object = sd
}

// reencodeEntry sostituisce un'immagine solo se sa decodificarla e il risultato
// e' piu' piccolo; ogni altro oggetto resta intatto.
func reencodeEntry(ctx *model.Context, entry *model.XRefTableEntry, rung imageRung) {
	if entry == nil || entry.Free {
		return
	}
	sd, ok := entry.Object.(types.StreamDict)
	if !ok || !sd.Image() || !canReencode(ctx, sd) {
		return
	}
	img, err := decodePDFImage(ctx, &sd)
	if err != nil || img == nil {
		return
	}
	if longSide(img.Bounds()) > rung.side {
		if gray, isGray := img.(*image.Gray); isGray {
			img = resizeGray(gray, rung.side)
		} else {
			img = resize(img, rung.side)
		}
	}
	encoded, err := encodeJPEG(img, rung.quality)
	if err != nil || len(encoded) >= len(sd.Raw) {
		return
	}

	sd.Raw = encoded
	sd.Content = nil
	sd.FilterPipeline = []types.PDFFilter{{Name: "DCTDecode"}}
	length := int64(len(encoded))
	sd.StreamLength = &length
	sd.StreamLengthObjNr = nil
	sd.Delete("DecodeParms")
	sd.Update("Filter", types.Name("DCTDecode"))
	sd.Update("Length", types.Integer(length))
	sd.Update("Width", types.Integer(img.Bounds().Dx()))
	sd.Update("Height", types.Integer(img.Bounds().Dy()))
	sd.Update("BitsPerComponent", types.Integer(8))
	if _, isGray := img.(*image.Gray); isGray {
		sd.Update("ColorSpace", types.Name("DeviceGray"))
	} else {
		sd.Update("ColorSpace", types.Name("DeviceRGB"))
	}
	entry.Object = sd
}

// canReencode esclude cio' che un JPEG cambierebbe: maschere stencil o a chiave
// di colore, curve Decode, Matte (richiede le stesse dimensioni dell'immagine).
func canReencode(ctx *model.Context, sd types.StreamDict) bool {
	if b := sd.BooleanEntry("ImageMask"); b != nil && *b {
		return false
	}
	if sd.HasEntry("Decode") || sd.HasEntry("Matte") {
		return false
	}
	if obj, found := sd.Find("Mask"); found {
		resolved, err := ctx.Dereference(obj)
		if err != nil {
			return false
		}
		if _, isArray := resolved.(types.Array); isArray {
			return false
		}
	}
	if obj, found := sd.Find("SMask"); found {
		resolved, err := ctx.Dereference(obj)
		if err != nil {
			return false
		}
		if mask, isStream := resolved.(types.StreamDict); isStream && mask.HasEntry("Matte") {
			return false
		}
	}
	return colorComponents(ctx, sd) != 0
}

// colorComponents restituisce 1 o 3 per gli spazi colore convertibili, altrimenti 0.
func colorComponents(ctx *model.Context, sd types.StreamDict) int {
	obj, found := sd.Find("ColorSpace")
	if !found {
		return 0
	}
	resolved, err := ctx.Dereference(obj)
	if err != nil {
		return 0
	}
	switch cs := resolved.(type) {
	case types.Name:
		switch cs {
		case "DeviceRGB":
			return 3
		case "DeviceGray":
			return 1
		}
	case types.Array:
		if len(cs) < 2 {
			return 0
		}
		family, err := ctx.Dereference(cs[0])
		if err != nil {
			return 0
		}
		switch family {
		case types.Name("CalRGB"):
			return 3
		case types.Name("CalGray"):
			return 1
		case types.Name("ICCBased"):
			profile, err := ctx.Dereference(cs[1])
			if err != nil {
				return 0
			}
			if stream, ok := profile.(types.StreamDict); ok {
				if n := stream.IntEntry("N"); n != nil && (*n == 1 || *n == 3) {
					return *n
				}
			}
		}
	}
	return 0
}

// decodePDFImage restituisce *image.Gray o *image.RGBA; nil se il formato non e' gestito.
func decodePDFImage(ctx *model.Context, sd *types.StreamDict) (image.Image, error) {
	comps := colorComponents(ctx, *sd)
	for _, f := range sd.FilterPipeline {
		switch f.Name {
		case "JPXDecode", "JBIG2Decode", "CCITTFaxDecode":
			return nil, nil
		}
	}
	if sd.HasSoleFilterNamed("DCTDecode") {
		img, err := jpeg.Decode(bytes.NewReader(sd.Raw))
		if err != nil {
			return nil, err
		}
		switch src := img.(type) {
		case *image.Gray:
			if comps == 1 {
				return src, nil
			}
		case *image.YCbCr:
			if comps == 3 {
				return flatten(src), nil
			}
		}
		return nil, nil
	}
	if slices.ContainsFunc(sd.FilterPipeline, func(f types.PDFFilter) bool { return f.Name == "DCTDecode" }) {
		return nil, nil
	}

	bits, width, height := sd.IntEntry("BitsPerComponent"), sd.IntEntry("Width"), sd.IntEntry("Height")
	if bits == nil || width == nil || height == nil || *bits != 8 || *width <= 0 || *height <= 0 {
		return nil, nil
	}
	w, h := *width, *height
	if w*h > maxPixels {
		return nil, nil
	}
	if err := ctx.DecodeStream(sd); err != nil {
		return nil, err
	}
	if len(sd.Content) < w*h*comps {
		return nil, nil
	}
	if comps == 1 {
		img := image.NewGray(image.Rect(0, 0, w, h))
		copy(img.Pix, sd.Content)
		return img, nil
	}
	img := image.NewRGBA(image.Rect(0, 0, w, h))
	for i := range w * h {
		img.Pix[i*4], img.Pix[i*4+1], img.Pix[i*4+2], img.Pix[i*4+3] =
			sd.Content[i*3], sd.Content[i*3+1], sd.Content[i*3+2], 255
	}
	return img, nil
}

// resizeGray mantiene un solo canale: i JPEG grigi pesano meno e le SMask lo richiedono.
func resizeGray(src *image.Gray, side int) *image.Gray {
	rgba := resize(src, side)
	dst := image.NewGray(rgba.Bounds())
	for i := range dst.Pix {
		dst.Pix[i] = rgba.Pix[i*4]
	}
	return dst
}

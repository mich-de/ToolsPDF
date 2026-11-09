package compress

import (
	"bytes"
	"encoding/binary"
	"errors"
	"image"
	"image/color"
	"image/draw"
	_ "image/gif"
	"image/jpeg"
	_ "image/png"

	_ "golang.org/x/image/bmp"
	xdraw "golang.org/x/image/draw"
	_ "golang.org/x/image/tiff"
	_ "golang.org/x/image/webp"
)

const (
	// maxPixels evita di esaurire la RAM su immagini enormi o malformate.
	maxPixels = 400_000_000
	// pageLongSidePt e' il lato lungo della pagina: quello di un A4.
	pageLongSidePt = 842.0
)

// imageSides elenca i lati lunghi da provare; 0 significa dimensione originale.
var imageSides = []int{0, 3508, 2480, 2000, 1654, 1240, 1000, 800, 640, 480}

func compressImage(data []byte, progress Progress) ([]byte, string, error) {
	progress(0.05, "Lettura immagine…")
	cfg, format, err := image.DecodeConfig(bytes.NewReader(data))
	if err != nil {
		return nil, "", errors.New("formato non supportato: usa JPG, PNG, GIF, BMP, TIFF, WebP o PDF")
	}
	if cfg.Width*cfg.Height > maxPixels {
		return nil, "", errors.New("immagine troppo grande (oltre 400 megapixel)")
	}

	orientation := 1
	if format == "jpeg" {
		orientation = jpegOrientation(data)
		// JPEG gia' piccolo e dritto: si incorpora tal quale, senza perdere qualita'.
		if orientation == 1 && len(data)+1024 <= TargetBytes && cfg.ColorModel != color.CMYKModel {
			w, h := pageSize(cfg.Width, cfg.Height)
			out, err := writeJPEGPDF([]jpegPage{{data, w, h}})
			return out, "", err
		}
	}

	decoded, _, err := image.Decode(bytes.NewReader(data))
	if err != nil {
		return nil, "", errors.New("immagine danneggiata o non leggibile")
	}
	src := applyOrientation(flatten(decoded), orientation)
	budget := TargetBytes - 1024

	for i, side := range imageSides {
		progress(0.15+0.8*float64(i)/float64(len(imageSides)), "Compressione immagine…")
		var scaled image.Image = src
		if side > 0 {
			if longSide(src.Bounds()) <= side {
				continue
			}
			scaled = resize(src, side)
		}
		minQ := 60
		if i == len(imageSides)-1 {
			minQ = 25
		}
		if encoded, ok := bestJPEG(scaled, minQ, 90, budget); ok {
			w, h := pageSize(scaled.Bounds().Dx(), scaled.Bounds().Dy())
			out, err := writeJPEGPDF([]jpegPage{{encoded, w, h}})
			return out, "", err
		}
	}
	return nil, "", errors.New("immagine non comprimibile sotto 1 MB")
}

// bestJPEG cerca la qualita' piu' alta in [minQ, maxQ] che sta nel budget.
func bestJPEG(img image.Image, minQ, maxQ, budget int) ([]byte, bool) {
	best, err := encodeJPEG(img, minQ)
	if err != nil || len(best) > budget {
		return nil, false
	}
	lo, hi := minQ+1, maxQ
	for lo <= hi {
		q := (lo + hi) / 2
		enc, err := encodeJPEG(img, q)
		if err == nil && len(enc) <= budget {
			best, lo = enc, q+1
		} else {
			hi = q - 1
		}
	}
	return best, true
}

func encodeJPEG(img image.Image, quality int) ([]byte, error) {
	var b bytes.Buffer
	err := jpeg.Encode(&b, img, &jpeg.Options{Quality: quality})
	return b.Bytes(), err
}

func pageSize(w, h int) (float64, float64) {
	if w >= h {
		return pageLongSidePt, pageLongSidePt * float64(h) / float64(w)
	}
	return pageLongSidePt * float64(w) / float64(h), pageLongSidePt
}

func longSide(r image.Rectangle) int {
	return max(r.Dx(), r.Dy())
}

// resize porta il lato lungo a side mantenendo le proporzioni.
func resize(src image.Image, side int) *image.RGBA {
	b := src.Bounds()
	w, h := side, max(1, b.Dy()*side/b.Dx())
	if b.Dy() > b.Dx() {
		w, h = max(1, b.Dx()*side/b.Dy()), side
	}
	dst := image.NewRGBA(image.Rect(0, 0, w, h))
	xdraw.CatmullRom.Scale(dst, dst.Bounds(), src, b, xdraw.Src, nil)
	return dst
}

// flatten appoggia l'immagine su bianco: il JPEG non ha trasparenza.
func flatten(src image.Image) *image.RGBA {
	b := src.Bounds()
	dst := image.NewRGBA(image.Rect(0, 0, b.Dx(), b.Dy()))
	draw.Draw(dst, dst.Bounds(), image.White, image.Point{}, draw.Src)
	draw.Draw(dst, dst.Bounds(), src, b.Min, draw.Over)
	return dst
}

// jpegOrientation legge il tag EXIF Orientation; 1 se assente o non valido.
func jpegOrientation(data []byte) int {
	for i := 2; i+4 <= len(data) && data[i] == 0xFF; {
		marker := data[i+1]
		size := int(binary.BigEndian.Uint16(data[i+2:]))
		if marker == 0xDA || size < 2 || i+2+size > len(data) {
			return 1
		}
		if marker == 0xE1 {
			if o := exifOrientation(data[i+4 : i+2+size]); o != 0 {
				return o
			}
		}
		i += 2 + size
	}
	return 1
}

func exifOrientation(seg []byte) int {
	if len(seg) < 14 || !bytes.HasPrefix(seg, []byte("Exif\x00\x00")) {
		return 0
	}
	tiff := seg[6:]
	var order binary.ByteOrder
	switch string(tiff[:2]) {
	case "II":
		order = binary.LittleEndian
	case "MM":
		order = binary.BigEndian
	default:
		return 0
	}
	ifd := int(order.Uint32(tiff[4:]))
	if ifd < 0 || ifd+2 > len(tiff) {
		return 0
	}
	count := int(order.Uint16(tiff[ifd:]))
	for i := range count {
		e := ifd + 2 + i*12
		if e+12 > len(tiff) {
			return 0
		}
		if order.Uint16(tiff[e:]) == 0x0112 {
			if o := int(order.Uint16(tiff[e+8:])); o >= 1 && o <= 8 {
				return o
			}
			return 0
		}
	}
	return 0
}

// applyOrientation raddrizza l'immagine secondo il tag EXIF (foto da telefono).
func applyOrientation(src *image.RGBA, o int) *image.RGBA {
	if o <= 1 || o > 8 {
		return src
	}
	w, h := src.Bounds().Dx(), src.Bounds().Dy()
	dw, dh := w, h
	if o >= 5 {
		dw, dh = h, w
	}
	dst := image.NewRGBA(image.Rect(0, 0, dw, dh))
	for y := range h {
		for x := range w {
			var dx, dy int
			switch o {
			case 2:
				dx, dy = w-1-x, y
			case 3:
				dx, dy = w-1-x, h-1-y
			case 4:
				dx, dy = x, h-1-y
			case 5:
				dx, dy = y, x
			case 6:
				dx, dy = h-1-y, x
			case 7:
				dx, dy = h-1-y, w-1-x
			case 8:
				dx, dy = y, w-1-x
			}
			s := src.PixOffset(x, y)
			copy(dst.Pix[dst.PixOffset(dx, dy):], src.Pix[s:s+4])
		}
	}
	return dst
}

package ui

import (
	"unsafe"

	"golang.org/x/sys/windows"
	"golang.org/x/sys/windows/registry"
)

// Dimensioni logiche a 96 DPI.
const (
	headerHeight   = 44
	margin         = 12
	dropHeight     = 150
	rowHeight      = 84
	closeSize      = 32
	progressHeight = 6
)

// palette usa COLORREF (0x00BBGGRR); i valori vengono dal design system del progetto.
type palette struct {
	bg, card, fg, muted, border, primary, ok, err, track, hover uintptr
}

func rgb(hex uint32) uintptr {
	return uintptr((hex>>16)&0xFF | hex&0xFF00 | (hex&0xFF)<<16)
}

var (
	lightPalette = palette{
		bg: rgb(0xF8FAFC), card: rgb(0xFFFFFF), fg: rgb(0x0F172A), muted: rgb(0x475569),
		border: rgb(0xCBD5E1), primary: rgb(0x2563EB), ok: rgb(0x15803D), err: rgb(0xDC2626),
		track: rgb(0xE2E8F0), hover: rgb(0xE2E8F0),
	}
	darkPalette = palette{
		bg: rgb(0x0F172A), card: rgb(0x1E293B), fg: rgb(0xF1F5F9), muted: rgb(0x94A3B8),
		border: rgb(0x475569), primary: rgb(0x60A5FA), ok: rgb(0x4ADE80), err: rgb(0xF87171),
		track: rgb(0x334155), hover: rgb(0x334155),
	}
)

func currentPalette() palette {
	key, err := registry.OpenKey(registry.CURRENT_USER,
		`Software\Microsoft\Windows\CurrentVersion\Themes\Personalize`, registry.QUERY_VALUE)
	if err != nil {
		return lightPalette
	}
	defer key.Close()
	if v, _, err := key.GetIntegerValue("AppsUseLightTheme"); err == nil && v == 0 {
		return darkPalette
	}
	return lightPalette
}

type fontSet struct{ title, body, bodyBold, small uintptr }

func newFont(a *app, size int32, weight uintptr) uintptr {
	h, _, _ := pCreateFontW.Call(uintptr(-a.scale(size)), 0, 0, 0, weight, 0, 0, 0, 1, 0, 0, cleartypeQual, 0,
		ptr(wstr("Segoe UI")))
	return h
}

func (f *fontSet) rebuild(a *app) {
	for _, h := range []uintptr{f.title, f.body, f.bodyBold, f.small} {
		if h != 0 {
			pDeleteObject.Call(h)
		}
	}
	f.title = newFont(a, 15, fwSemibold)
	f.body = newFont(a, 13, fwNormal)
	f.bodyBold = newFont(a, 13, fwSemibold)
	f.small = newFont(a, 12, fwNormal)
}

func (a *app) closeRect() rect {
	var client rect
	pGetClientRect.Call(a.hwnd, ptr(&client))
	size := a.scale(closeSize)
	gap := (a.scale(headerHeight) - size) / 2
	return rect{client.right - size - gap, gap, client.right - gap, gap + size}
}

func (a *app) paint(hwnd uintptr) {
	var ps paintStruct
	hdc, _, _ := pBeginPaint.Call(hwnd, ptr(&ps))
	defer pEndPaint.Call(hwnd, ptr(&ps))

	var client rect
	pGetClientRect.Call(hwnd, ptr(&client))
	w, h := client.right, client.bottom

	// Doppio buffer: la barra di avanzamento si aggiorna spesso e non deve sfarfallare.
	mem, _, _ := pCreateCompatibleDC.Call(hdc)
	defer pDeleteDC.Call(mem)
	bmp, _, _ := pCreateCompatibleBmp.Call(hdc, uintptr(w), uintptr(h))
	defer pDeleteObject.Call(bmp)
	old, _, _ := pSelectObject.Call(mem, bmp)
	defer pSelectObject.Call(mem, old)
	pSetBkMode.Call(mem, transparent)

	fillRect(mem, client, a.theme.bg)
	fillRect(mem, rect{0, 0, a.scale(1), h}, a.theme.border)
	a.paintRows(mem, w, h)
	a.paintHeader(mem, w)
	a.paintDropZone(mem, w)

	pBitBlt.Call(hdc, 0, 0, uintptr(w), uintptr(h), mem, 0, 0, srcCopy)
}

func (a *app) paintHeader(dc uintptr, w int32) {
	s := a.scale
	fillRect(dc, rect{s(1), 0, w, s(headerHeight)}, a.theme.bg)
	drawText(dc, appTitle, rect{s(margin), 0, w, s(headerHeight)}, a.fonts.title, a.theme.fg, dtSingleLine|dtVCenter|dtNoPrefix)

	c := a.closeRect()
	if a.hoverClose {
		fillRoundRect(dc, c, s(6), a.theme.hover, a.theme.hover)
	}
	inset := s(11)
	pen := selectPen(dc, psSolid, s(1)+1, a.theme.fg)
	line(dc, c.left+inset, c.top+inset, c.right-inset, c.bottom-inset)
	line(dc, c.right-inset, c.top+inset, c.left+inset, c.bottom-inset)
	pen()
}

func (a *app) paintDropZone(dc uintptr, w int32) {
	s := a.scale
	top := s(headerHeight)
	fillRect(dc, rect{s(1), top, w, top + s(dropHeight) + s(margin)}, a.theme.bg)
	zone := rect{s(margin), top, w - s(margin), top + s(dropHeight)}

	fillRoundRect(dc, zone, s(8), a.theme.card, a.theme.card)
	restorePen := selectPen(dc, psDash, 1, a.theme.primary)
	brush, _, _ := pGetStockObject.Call(nullBrush)
	oldBrush, _, _ := pSelectObject.Call(dc, brush)
	pRoundRect.Call(dc, uintptr(zone.left), uintptr(zone.top), uintptr(zone.right), uintptr(zone.bottom), uintptr(s(8)), uintptr(s(8)))
	pSelectObject.Call(dc, oldBrush)
	restorePen()

	// Icona documento con angolo piegato, disegnata per restare nitida a ogni DPI.
	cx := (zone.left + zone.right) / 2
	il, ir, it, ib := cx-s(16), cx+s(16), zone.top+s(20), zone.top+s(62)
	fold := s(10)
	iconPen := selectPen(dc, psSolid, s(1)+1, a.theme.primary)
	line(dc, il, it, ir-fold, it)
	line(dc, ir-fold, it, ir, it+fold)
	line(dc, ir, it+fold, ir, ib)
	line(dc, ir, ib, il, ib)
	line(dc, il, ib, il, it)
	line(dc, ir-fold, it, ir-fold, it+fold)
	line(dc, ir-fold, it+fold, ir, it+fold)
	iconPen()
	drawText(dc, "PDF", rect{il, it + fold, ir, ib}, a.fonts.small, a.theme.primary, dtCenter|dtVCenter|dtSingleLine|dtNoPrefix)

	drawText(dc, "Trascina qui immagini o PDF", rect{zone.left + s(8), zone.top + s(72), zone.right - s(8), zone.top + s(112)},
		a.fonts.bodyBold, a.theme.fg, dtCenter|dtWordBreak|dtNoPrefix)
	drawText(dc, "Max 1 MB, nella stessa cartella", rect{zone.left + s(8), zone.top + s(112), zone.right - s(8), zone.bottom - s(4)},
		a.fonts.small, a.theme.muted, dtCenter|dtWordBreak|dtNoPrefix)
}

// paintRows mostra per primo l'ultimo file aggiunto, cosi' resta visibile senza scorrere.
func (a *app) paintRows(dc uintptr, w, h int32) {
	s := a.scale
	a.mu.Lock()
	defer a.mu.Unlock()

	top := s(headerHeight) + s(dropHeight) + s(margin)
	maxScroll := int32(len(a.rows))*s(rowHeight) - (h - top)
	a.scroll = min(a.scroll, maxScroll)
	a.scroll = max(a.scroll, 0)

	y := top - a.scroll
	for i := len(a.rows) - 1; i >= 0; i-- {
		if y+s(rowHeight) > top && y < h {
			a.paintRow(dc, a.rows[i], y, w)
		}
		y += s(rowHeight)
	}
}

func (a *app) paintRow(dc uintptr, r *row, y, w int32) {
	s := a.scale
	left, right := s(margin), w-s(margin)
	color := a.theme.muted
	prefix := ""
	switch r.state {
	case stateDone:
		color, prefix = a.theme.ok, "OK  "
	case stateFailed:
		color, prefix = a.theme.err, "Errore  "
	}
	if r.state == stateFailed && r.status == "Annullato: nessun file creato" {
		color, prefix = a.theme.muted, ""
	}

	drawText(dc, r.name, rect{left, y + s(4), right, y + s(24)}, a.fonts.bodyBold, a.theme.fg, dtSingleLine|dtVCenter|dtNoPrefix|dtEndEllipsis)

	bar := rect{left, y + s(28), right, y + s(28) + s(progressHeight)}
	fillRoundRect(dc, bar, s(progressHeight), a.theme.track, a.theme.track)
	fill := a.theme.primary
	switch r.state {
	case stateDone:
		fill = a.theme.ok
	case stateFailed:
		fill = a.theme.err
	}
	if width := int32(float64(bar.right-bar.left) * r.pct); r.state != stateQueued && width > 0 {
		done := bar
		done.right = bar.left + max(width, s(progressHeight))
		fillRoundRect(dc, done, s(progressHeight), fill, fill)
	}

	drawText(dc, prefix+r.status, rect{left, y + s(38), right, y + s(rowHeight) - s(2)}, a.fonts.small, color,
		dtWordBreak|dtEditControl|dtEndEllipsis|dtNoPrefix)
}

func fillRect(dc uintptr, r rect, color uintptr) {
	brush, _, _ := pCreateSolidBrush.Call(color)
	pFillRect.Call(dc, ptr(&r), brush)
	pDeleteObject.Call(brush)
}

func fillRoundRect(dc uintptr, r rect, radius int32, fill, outline uintptr) {
	brush, _, _ := pCreateSolidBrush.Call(fill)
	oldBrush, _, _ := pSelectObject.Call(dc, brush)
	restore := selectPen(dc, psSolid, 1, outline)
	pRoundRect.Call(dc, uintptr(r.left), uintptr(r.top), uintptr(r.right), uintptr(r.bottom), uintptr(radius), uintptr(radius))
	restore()
	pSelectObject.Call(dc, oldBrush)
	pDeleteObject.Call(brush)
}

// selectPen seleziona una penna e restituisce la funzione che la libera.
func selectPen(dc, style uintptr, width int32, color uintptr) func() {
	pen, _, _ := pCreatePen.Call(style, uintptr(width), color)
	old, _, _ := pSelectObject.Call(dc, pen)
	return func() {
		pSelectObject.Call(dc, old)
		pDeleteObject.Call(pen)
	}
}

func line(dc uintptr, x1, y1, x2, y2 int32) {
	pMoveToEx.Call(dc, uintptr(x1), uintptr(y1), 0)
	pLineTo.Call(dc, uintptr(x2), uintptr(y2))
}

func drawText(dc uintptr, text string, r rect, font, color, flags uintptr) {
	old, _, _ := pSelectObject.Call(dc, font)
	pSetTextColor.Call(dc, color)
	u, err := windows.UTF16FromString(text)
	if err == nil && len(u) > 1 {
		pDrawTextW.Call(dc, uintptr(unsafe.Pointer(&u[0])), uintptr(len(u)-1), ptr(&r), flags)
	}
	pSelectObject.Call(dc, old)
}

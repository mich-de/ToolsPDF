package ui

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"sync"
	"unsafe"

	"golang.org/x/sys/windows"

	"onembpdf/internal/compress"
)

const (
	logicalWidth = 220
	appTitle     = "1MbPDF"
	queueSize    = 4096
)

type rowState int

const (
	stateQueued rowState = iota
	stateWorking
	stateDone
	stateFailed
)

// row e' un file nell'elenco; i campi sono protetti da app.mu.
type row struct {
	path   string
	name   string
	pct    float64
	status string
	state  rowState
}

type app struct {
	hwnd          uintptr
	dpi           uint32
	taskbarMsg    uint32
	registered    bool
	hoverClose    bool
	trackingMouse bool
	scroll        int32
	theme         palette
	fonts         fontSet

	mu    sync.Mutex
	rows  []*row
	queue chan *row
}

var theApp = &app{queue: make(chan *row, queueSize)}

// Run mostra la barra e resta in esecuzione fino alla chiusura.
func Run(files []string) error {
	runtime.LockOSThread()
	pSetDpiAwarenessCtx.Call(dpiPerMonitorV2)

	mutexName := wstr("Local\\1MbPDF-SingleInstance")
	mutex, err := windows.CreateMutex(nil, false, mutexName)
	if errors.Is(err, windows.ERROR_ALREADY_EXISTS) {
		messageBox(0, "La barra è già aperta sul bordo destro dello schermo.", appTitle, mbIconWarn|mbTopmost)
		return nil
	}
	if err != nil {
		return err
	}
	defer windows.CloseHandle(mutex)

	var instance windows.Handle
	if err := windows.GetModuleHandleEx(0, nil, &instance); err != nil {
		return err
	}
	cursor, _, _ := pLoadCursorW.Call(0, idcArrow)
	className := wstr("OneMbPDFSidebar")
	class := wndClassEx{
		wndProc:   windows.NewCallback(wndProc),
		instance:  instance,
		cursor:    windows.Handle(cursor),
		className: className,
	}
	class.size = uint32(unsafe.Sizeof(class))
	if r, _, e := pRegisterClassExW.Call(ptr(&class)); r == 0 {
		return fmt.Errorf("RegisterClassEx: %w", e)
	}

	a := theApp
	a.theme = currentPalette()
	a.taskbarMsg = registerTaskbarMessage()

	hwnd, _, e := pCreateWindowExW.Call(
		wsExTopmost|wsExToolWindow|wsExAcceptFiles,
		ptr(className), ptr(wstr(appTitle)), wsPopup,
		0, 0, 0, 0, 0, 0, uintptr(instance), 0)
	if hwnd == 0 {
		return fmt.Errorf("CreateWindowEx: %w", e)
	}
	a.hwnd = hwnd
	a.updateDPI()
	a.registerAppBar()
	defer a.removeAppBar()
	pShowWindow.Call(hwnd, swShowNA)

	go a.worker()
	a.enqueue(files)

	var m msg
	for {
		r, _, _ := pGetMessageW.Call(ptr(&m), 0, 0, 0)
		if int32(r) <= 0 {
			break
		}
		pTranslateMessage.Call(ptr(&m))
		pDispatchMessageW.Call(ptr(&m))
	}
	return nil
}

func registerTaskbarMessage() uint32 {
	r, _, _ := pRegisterWindowMsgW.Call(ptr(wstr("TaskbarCreated")))
	return uint32(r)
}

func (a *app) scale(v int32) int32 {
	return v * int32(a.dpi) / 96
}

func (a *app) updateDPI() {
	d, _, _ := pGetDpiForWindow.Call(a.hwnd)
	if d == 0 {
		d = 96
	}
	a.dpi = uint32(d)
	a.fonts.rebuild(a)
}

func (a *app) registerAppBar() {
	abd := a.appBarData()
	abd.callback = wmAppBarNotify
	pSHAppBarMessage.Call(abmNew, ptr(&abd))
	a.registered = true
	a.positionAppBar()
}

func (a *app) removeAppBar() {
	if !a.registered {
		return
	}
	abd := a.appBarData()
	pSHAppBarMessage.Call(abmRemove, ptr(&abd))
	a.registered = false
}

func (a *app) appBarData() appBarData {
	abd := appBarData{hwnd: a.hwnd, edge: abeRight}
	abd.size = uint32(unsafe.Sizeof(abd))
	return abd
}

// positionAppBar riserva la striscia a destra: da qui le finestre massimizzate si fermano prima.
func (a *app) positionAppBar() {
	width := a.scale(logicalWidth)
	cx, _, _ := pGetSystemMetrics.Call(smCxScreen)
	cy, _, _ := pGetSystemMetrics.Call(smCyScreen)
	abd := a.appBarData()
	abd.rc = rect{int32(cx) - width, 0, int32(cx), int32(cy)}
	pSHAppBarMessage.Call(abmQueryPos, ptr(&abd))
	abd.rc.left = abd.rc.right - width
	pSHAppBarMessage.Call(abmSetPos, ptr(&abd))
	pSetWindowPos.Call(a.hwnd, 0,
		uintptr(abd.rc.left), uintptr(abd.rc.top),
		uintptr(abd.rc.right-abd.rc.left), uintptr(abd.rc.bottom-abd.rc.top),
		swpNoZOrder|swpNoActivate)
}

func (a *app) requestRepaint() {
	pPostMessageW.Call(a.hwnd, wmRepaint, 0, 0)
}

func (a *app) enqueue(paths []string) {
	for _, path := range paths {
		r := &row{path: path, name: filepath.Base(path), status: "In coda"}
		a.mu.Lock()
		a.rows = append(a.rows, r)
		a.mu.Unlock()
		select {
		case a.queue <- r:
		default:
			a.update(r, func() { r.state, r.status = stateFailed, "Troppi file in coda" })
		}
	}
	a.scroll = 0
	a.requestRepaint()
}

func (a *app) update(r *row, change func()) {
	a.mu.Lock()
	change()
	a.mu.Unlock()
	a.requestRepaint()
}

// worker comprime un file alla volta: i passaggi sono pesanti su CPU e RAM.
func (a *app) worker() {
	for r := range a.queue {
		a.update(r, func() { r.state, r.status = stateWorking, "Avvio…" })
		res, err := compress.Process(r.path,
			func(pct float64, msg string) {
				a.update(r, func() { r.pct, r.status = pct, msg })
			},
			func(kept int64) bool {
				a.update(r, func() { r.status = "In attesa di conferma" })
				return a.askRaster(r.name, kept)
			})
		a.update(r, func() {
			r.pct = 1
			switch {
			case errors.Is(err, compress.ErrCancelled):
				r.state, r.status = stateFailed, "Annullato: nessun file creato"
			case err != nil:
				r.state, r.status = stateFailed, err.Error()
			default:
				r.state, r.status = stateDone, doneStatus(res)
			}
		})
	}
}

func doneStatus(res compress.Result) string {
	s := fmt.Sprintf("%s → %s", compress.FormatMB(res.InSize), compress.FormatMB(res.OutSize))
	if res.Note != "" {
		s += " · " + res.Note
	}
	return s
}

func (a *app) askRaster(name string, kept int64) bool {
	reason := "Questo PDF non si può ridurre mantenendo il testo selezionabile."
	if kept > 0 {
		reason = fmt.Sprintf("Mantenendo il testo selezionabile questo PDF non scende sotto %s.", compress.FormatMB(kept))
	}
	text := fmt.Sprintf("%s\n\n%s\n\nConvertire le pagine in immagini per stare sotto 1 MB?\nIl testo non sarà più selezionabile; l'originale resta intatto.", name, reason)
	return messageBox(a.hwnd, text, appTitle, mbYesNo|mbIconQuest|mbTopmost) == idYes
}

func (a *app) busy() bool {
	a.mu.Lock()
	defer a.mu.Unlock()
	for _, r := range a.rows {
		if r.state == stateQueued || r.state == stateWorking {
			return true
		}
	}
	return false
}

func (a *app) droppedFiles(drop uintptr) []string {
	defer pDragFinish.Call(drop)
	count, _, _ := pDragQueryFileW.Call(drop, 0xFFFFFFFF, 0, 0)
	paths := make([]string, 0, count)
	for i := uintptr(0); i < count; i++ {
		n, _, _ := pDragQueryFileW.Call(drop, i, 0, 0)
		buf := make([]uint16, n+1)
		pDragQueryFileW.Call(drop, i, ptr(&buf[0]), n+1)
		paths = append(paths, windows.UTF16ToString(buf))
	}
	return paths
}

func wndProc(hwnd, message, wParam, lParam uintptr) uintptr {
	a := theApp
	switch uint32(message) {
	case wmPaint:
		a.paint(hwnd)
		return 0
	case wmEraseBkgnd:
		return 1
	case wmRepaint:
		pInvalidateRect.Call(hwnd, 0, 0)
		return 0
	case wmDropFiles:
		a.enqueue(a.droppedFiles(wParam))
		return 0
	case wmMouseMove:
		a.onMouseMove(hwnd, lParam)
		return 0
	case wmMouseLeave:
		a.trackingMouse = false
		if a.hoverClose {
			a.hoverClose = false
			pInvalidateRect.Call(hwnd, 0, 0)
		}
		return 0
	case wmLButtonUp:
		if a.closeRect().contains(lowWord(lParam), highWord(lParam)) {
			pPostMessageW.Call(hwnd, wmClose, 0, 0)
		}
		return 0
	case wmMouseWheel:
		delta := highWord(wParam)
		a.scroll -= delta * a.scale(rowHeight) / 120
		pInvalidateRect.Call(hwnd, 0, 0)
		return 0
	case wmClose:
		if a.busy() && messageBox(hwnd, "Ci sono file in lavorazione. Chiudere comunque?", appTitle, mbYesNo|mbIconWarn|mbTopmost) != idYes {
			return 0
		}
		pDestroyWindow.Call(hwnd)
		return 0
	case wmDestroy:
		a.removeAppBar()
		pPostQuitMessage.Call(0)
		return 0
	case wmQueryEndSession:
		return 1
	case wmEndSession:
		if wParam != 0 {
			a.removeAppBar()
		}
		return 0
	case wmAppBarNotify:
		a.onAppBarNotify(hwnd, wParam, lParam)
		return 0
	case wmActivate:
		abd := a.appBarData()
		pSHAppBarMessage.Call(abmActivate, ptr(&abd))
	case wmWindowPosChgd:
		if a.registered {
			abd := a.appBarData()
			pSHAppBarMessage.Call(abmWindowPosChanged, ptr(&abd))
		}
	case wmDisplayChange:
		a.positionAppBar()
	case wmDPIChanged:
		a.updateDPI()
		a.positionAppBar()
		return 0
	case wmSettingChange:
		a.theme = currentPalette()
		pInvalidateRect.Call(hwnd, 0, 0)
	default:
		// Se Explorer riparte perde le appbar: ci si registra di nuovo.
		if a.taskbarMsg != 0 && uint32(message) == a.taskbarMsg {
			a.registerAppBar()
			return 0
		}
	}
	r, _, _ := pDefWindowProcW.Call(hwnd, message, wParam, lParam)
	return r
}

func (a *app) onAppBarNotify(hwnd, wParam, lParam uintptr) {
	switch wParam {
	case abnPosChanged:
		a.positionAppBar()
	case abnFullScreenApp:
		// Un'app a schermo intero deve poter coprire la barra.
		order := hwndTopmost
		if lParam != 0 {
			order = hwndNotopmost
		}
		pSetWindowPos.Call(hwnd, order, 0, 0, 0, 0, swpNoMove|swpNoSize|swpNoActivate)
	}
}

func (a *app) onMouseMove(hwnd, lParam uintptr) {
	if !a.trackingMouse {
		tme := trackMouseEvent{flags: tmeLeave, hwnd: hwnd}
		tme.size = uint32(unsafe.Sizeof(tme))
		pTrackMouseEvent.Call(ptr(&tme))
		a.trackingMouse = true
	}
	hover := a.closeRect().contains(lowWord(lParam), highWord(lParam))
	if hover != a.hoverClose {
		a.hoverClose = hover
		pInvalidateRect.Call(hwnd, 0, 0)
	}
}

func lowWord(v uintptr) int32  { return int32(int16(v & 0xFFFF)) }
func highWord(v uintptr) int32 { return int32(int16((v >> 16) & 0xFFFF)) }

func (r rect) contains(x, y int32) bool {
	return x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

// Fatal mostra un errore di avvio: l'app non ha console.
func Fatal(err error) {
	messageBox(0, "Avvio non riuscito: "+err.Error(), appTitle, mbIconError|mbTopmost)
	os.Exit(1)
}

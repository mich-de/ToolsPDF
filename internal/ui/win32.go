// Package ui disegna la barra laterale con Win32 e GDI, senza dipendenze esterne.
package ui

import (
	"unsafe"

	"golang.org/x/sys/windows"
)

var (
	user32  = windows.NewLazySystemDLL("user32.dll")
	gdi32   = windows.NewLazySystemDLL("gdi32.dll")
	shell32 = windows.NewLazySystemDLL("shell32.dll")

	pRegisterClassExW    = user32.NewProc("RegisterClassExW")
	pCreateWindowExW     = user32.NewProc("CreateWindowExW")
	pDefWindowProcW      = user32.NewProc("DefWindowProcW")
	pDestroyWindow       = user32.NewProc("DestroyWindow")
	pGetMessageW         = user32.NewProc("GetMessageW")
	pTranslateMessage    = user32.NewProc("TranslateMessage")
	pDispatchMessageW    = user32.NewProc("DispatchMessageW")
	pPostQuitMessage     = user32.NewProc("PostQuitMessage")
	pPostMessageW        = user32.NewProc("PostMessageW")
	pInvalidateRect      = user32.NewProc("InvalidateRect")
	pBeginPaint          = user32.NewProc("BeginPaint")
	pEndPaint            = user32.NewProc("EndPaint")
	pGetClientRect       = user32.NewProc("GetClientRect")
	pSetWindowPos        = user32.NewProc("SetWindowPos")
	pShowWindow          = user32.NewProc("ShowWindow")
	pLoadCursorW         = user32.NewProc("LoadCursorW")
	pMessageBoxW         = user32.NewProc("MessageBoxW")
	pGetSystemMetrics    = user32.NewProc("GetSystemMetrics")
	pGetDpiForWindow     = user32.NewProc("GetDpiForWindow")
	pSetDpiAwarenessCtx  = user32.NewProc("SetProcessDpiAwarenessContext")
	pRegisterWindowMsgW  = user32.NewProc("RegisterWindowMessageW")
	pTrackMouseEvent     = user32.NewProc("TrackMouseEvent")
	pFillRect            = user32.NewProc("FillRect")
	pDrawTextW           = user32.NewProc("DrawTextW")
	pCreateCompatibleDC  = gdi32.NewProc("CreateCompatibleDC")
	pCreateCompatibleBmp = gdi32.NewProc("CreateCompatibleBitmap")
	pSelectObject        = gdi32.NewProc("SelectObject")
	pDeleteObject        = gdi32.NewProc("DeleteObject")
	pDeleteDC            = gdi32.NewProc("DeleteDC")
	pBitBlt              = gdi32.NewProc("BitBlt")
	pSetBkMode           = gdi32.NewProc("SetBkMode")
	pSetTextColor        = gdi32.NewProc("SetTextColor")
	pCreateFontW         = gdi32.NewProc("CreateFontW")
	pCreateSolidBrush    = gdi32.NewProc("CreateSolidBrush")
	pCreatePen           = gdi32.NewProc("CreatePen")
	pRoundRect           = gdi32.NewProc("RoundRect")
	pMoveToEx            = gdi32.NewProc("MoveToEx")
	pLineTo              = gdi32.NewProc("LineTo")
	pGetStockObject      = gdi32.NewProc("GetStockObject")
	pSHAppBarMessage     = shell32.NewProc("SHAppBarMessage")
	pDragQueryFileW      = shell32.NewProc("DragQueryFileW")
	pDragFinish          = shell32.NewProc("DragFinish")
)

const (
	wmDestroy         = 0x0002
	wmSize            = 0x0005
	wmActivate        = 0x0006
	wmPaint           = 0x000F
	wmClose           = 0x0010
	wmQueryEndSession = 0x0011
	wmEraseBkgnd      = 0x0014
	wmEndSession      = 0x0016
	wmSettingChange   = 0x001A
	wmWindowPosChgd   = 0x0047
	wmDisplayChange   = 0x007E
	wmMouseMove       = 0x0200
	wmLButtonUp       = 0x0202
	wmMouseWheel      = 0x020A
	wmDropFiles       = 0x0233
	wmMouseLeave      = 0x02A3
	wmDPIChanged      = 0x02E0
	wmApp             = 0x8000
	wmAppBarNotify    = wmApp + 1
	wmRepaint         = wmApp + 2

	wsPopup         = 0x80000000
	wsExTopmost     = 0x00000008
	wsExAcceptFiles = 0x00000010
	wsExToolWindow  = 0x00000080

	swpNoSize     = 0x0001
	swpNoMove     = 0x0002
	swpNoZOrder   = 0x0004
	swpNoActivate = 0x0010

	smCxScreen = 0
	smCyScreen = 1

	abmNew              = 0
	abmRemove           = 1
	abmQueryPos         = 2
	abmSetPos           = 3
	abmActivate         = 6
	abmWindowPosChanged = 9
	abeRight            = 2
	abnPosChanged       = 1
	abnFullScreenApp    = 2

	dtCenter      = 0x0001
	dtVCenter     = 0x0004
	dtWordBreak   = 0x0010
	dtSingleLine  = 0x0020
	dtNoPrefix    = 0x0800
	dtEditControl = 0x2000
	dtEndEllipsis = 0x8000

	mbYesNo       = 0x00000004
	mbIconError   = 0x00000010
	mbIconQuest   = 0x00000020
	mbIconWarn    = 0x00000030
	mbTopmost     = 0x00040000
	idYes         = 6
	tmeLeave      = 0x00000002
	nullBrush     = 5
	psSolid       = 0
	psDash        = 1
	transparent   = 1
	srcCopy       = 0x00CC0020
	idcArrow      = 32512
	cleartypeQual = 5
	fwNormal      = 400
	fwSemibold    = 600

	dpiPerMonitorV2 = ^uintptr(3) // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 (-4)
	hwndTopmost     = ^uintptr(0) // -1
	hwndNotopmost   = ^uintptr(1) // -2
	swShowNA        = 8
)

type rect struct{ left, top, right, bottom int32 }

type point struct{ x, y int32 }

type wndClassEx struct {
	size       uint32
	style      uint32
	wndProc    uintptr
	clsExtra   int32
	wndExtra   int32
	instance   windows.Handle
	icon       windows.Handle
	cursor     windows.Handle
	background windows.Handle
	menuName   *uint16
	className  *uint16
	iconSm     windows.Handle
}

type msg struct {
	hwnd    uintptr
	message uint32
	wParam  uintptr
	lParam  uintptr
	time    uint32
	pt      point
	private uint32
}

type paintStruct struct {
	hdc       uintptr
	erase     int32
	paint     rect
	restore   int32
	incUpdate int32
	reserved  [32]byte
}

type appBarData struct {
	size     uint32
	hwnd     uintptr
	callback uint32
	edge     uint32
	rc       rect
	lParam   uintptr
}

type trackMouseEvent struct {
	size      uint32
	flags     uint32
	hwnd      uintptr
	hoverTime uint32
}

func wstr(s string) *uint16 {
	p, err := windows.UTF16PtrFromString(s)
	if err != nil {
		p, _ = windows.UTF16PtrFromString("?")
	}
	return p
}

func ptr[T any](v *T) uintptr { return uintptr(unsafe.Pointer(v)) }

func messageBox(hwnd uintptr, text, title string, flags uintptr) uintptr {
	r, _, _ := pMessageBoxW.Call(hwnd, ptr(wstr(text)), ptr(wstr(title)), flags)
	return r
}

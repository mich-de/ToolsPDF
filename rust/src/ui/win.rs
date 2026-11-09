//! Piccoli aiuti sopra le API Win32.

use std::cell::Cell;
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MESSAGEBOX_RESULT, MESSAGEBOX_STYLE};

/// Cell con replace e lettura per tipi Copy.
pub struct Cell2<T: Copy>(Cell<T>);

impl<T: Copy> Cell2<T> {
    pub const fn new(value: T) -> Self {
        Cell2(Cell::new(value))
    }
    pub fn get(&self) -> T {
        self.0.get()
    }
    pub fn set(&self, value: T) {
        self.0.set(value);
    }
    pub fn replace(&self, value: T) -> T {
        self.0.replace(value)
    }
}

pub fn wide(text: &str) -> Vec<u16> {
    OsStr::new(text).encode_wide().chain(std::iter::once(0)).collect()
}

pub fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
    RECT { left, top, right, bottom }
}

pub trait RectExt {
    fn contains(&self, x: i32, y: i32) -> bool;
}

impl RectExt for RECT {
    fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

pub fn message_box(hwnd: HWND, text: &str, title: &str, flags: MESSAGEBOX_STYLE) -> MESSAGEBOX_RESULT {
    let (text, title) = (wide(text), wide(title));
    unsafe { MessageBoxW(hwnd, text.as_ptr(), title.as_ptr(), flags) }
}

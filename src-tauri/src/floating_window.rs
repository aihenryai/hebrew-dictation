//! Native toolbar visibility: show and raise without activating on EVERY show.
//! Tao's focus:false is one-shot, and its show() uses SW_SHOW afterwards.
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SetWindowPos, ShowWindow, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SW_SHOWNOACTIVATE, SW_HIDE,
};

pub(crate) fn set_visible(hwnd: HWND, visible: bool) -> Result<(), String> {
    unsafe { ShowWindow(hwnd, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE }); }
    if !visible { return Ok(()); }
    let flags = SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE;
    let result = unsafe { SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, flags) };
    if result == 0 {
        return Err(format!("שינוי תצוגת הפס נכשל: {}", std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    #[test]
    fn repeated_shows_raise_without_focus_and_hides_really_hide() {
        // An offscreen 1px test window: never type into or change another app.
        // Match the toolbar: a top-level window created topmost and no-activate.
        let class: Vec<u16> = "DictationFloatingWindowTest\0".encode_utf16().collect();
        let definition = WNDCLASSW {
            lpfnWndProc: Some(DefWindowProcW),
            lpszClassName: class.as_ptr(),
            ..unsafe { std::mem::zeroed() }
        };
        assert_ne!(unsafe { RegisterClassW(&definition) }, 0);
        let hwnd = unsafe {
            CreateWindowExW(WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST, class.as_ptr(),
                std::ptr::null(), WS_POPUP, -32000, -32000, 1, 1,
                std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null())
        };
        assert!(!hwnd.is_null());
        struct WindowGuard(HWND);
        impl Drop for WindowGuard {
            fn drop(&mut self) { unsafe { DestroyWindow(self.0); } }
        }
        let _guard = WindowGuard(hwnd);
        let foreground = unsafe { GetForegroundWindow() };
        for _ in 0..3 {
            set_visible(hwnd, true).unwrap();
            let mut message = unsafe { std::mem::zeroed() };
            while unsafe { PeekMessageW(&mut message, hwnd, 0, 0, PM_REMOVE) } != 0 {
                unsafe { TranslateMessage(&message); DispatchMessageW(&message); }
            }
            assert_ne!(unsafe { IsWindowVisible(hwnd) }, 0);
            let style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
            assert_ne!(style & WS_EX_TOPMOST as isize, 0, "style={style:#x}");
            assert_eq!(unsafe { GetForegroundWindow() }, foreground);
            // Simulate loss of topmost status outside Tao's cached flags.
            unsafe { SetWindowPos(hwnd, HWND_NOTOPMOST, 0, 0, 0, 0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE); }
            assert_eq!(unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } & WS_EX_TOPMOST as isize, 0, "negative control: native demotion must work");
            set_visible(hwnd, true).unwrap();
            assert_ne!(unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } & WS_EX_TOPMOST as isize, 0);
            set_visible(hwnd, false).unwrap();
            assert_eq!(unsafe { IsWindowVisible(hwnd) }, 0);
            assert_eq!(unsafe { GetForegroundWindow() }, foreground);
        }
    }
}

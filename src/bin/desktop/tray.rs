//! The tray (notification area) icon, on Windows: the window's close button hides the window and the app goes on; the icon brings it back
//! (a click or a double click) or ends the app (its menu), and a second start of the app brings the hidden window back too.
//!
//! The icon lives on a thread of its own with a window nobody sees: the Slint window can be hidden and made again, and its message handler
//! belongs to the windowing library. What the icon asks for is passed to the Slint event loop as a `TrayCmd`. Without an icon (not Windows,
//! or the icon could not be added) `present()` is false and the close button ends the app, as it always did: the app is never left running
//! with nothing to show for it.
#[cfg(not(windows))]
use std::path::Path;

#[cfg(not(windows))]
use crate::AppWindow;

/// What the icon asks of the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
pub enum TrayCmd {
    /// Show the window and bring it to the front.
    Open,
    /// End the app (after asking about edits that are not saved).
    Quit,
    /// Windows is ending the session: end the app at once.
    EndSession,
}

/// The words of the icon (its tooltip and menu).
#[cfg_attr(not(windows), allow(dead_code))]
pub struct Labels {
    pub open: String,
    pub quit: String,
    pub tip: String,
}

/// What one call of the icon's callback means (the notify icon of version 4: the low word of `lParam` is the event).
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
enum Ev {
    /// A click, a double click, or the Enter key on the icon.
    Select,
    /// A right click, the menu key.
    Menu,
}

#[cfg_attr(not(windows), allow(dead_code))]
fn event(lparam: isize) -> Option<Ev> {
    match (lparam & 0xFFFF) as u32 {
        // NIN_SELECT, NIN_KEYSELECT, WM_LBUTTONDBLCLK, and a click on a notice (NIN_BALLOONUSERCLICK)
        0x400 | 0x401 | 0x203 | 0x405 => Some(Ev::Select),
        // WM_CONTEXTMENU
        0x7B => Some(Ev::Menu),
        _ => None,
    }
}

#[cfg(not(windows))]
pub struct Tray;

#[cfg(not(windows))]
impl Drop for Tray {
    // Nothing to take away: there is no icon (here only so that dropping it reads the same on every system).
    fn drop(&mut self) {}
}

#[cfg(not(windows))]
pub fn start(_data: &Path, _labels: Labels, _on_cmd: Box<dyn Fn(TrayCmd) + Send + 'static>) -> Option<Tray> {
    None
}

#[cfg(not(windows))]
pub fn present() -> bool {
    false
}

#[cfg(not(windows))]
pub fn signal_running(_data: &Path) -> bool {
    false
}

/// A notice from the notification area (Windows), with a fixed text: not on other systems (they have their own way, see `notify`).
#[cfg(not(windows))]
#[allow(dead_code)]
pub fn notify(_title: &str, _body: &str) {}

/// Whether one of this app's windows is the one in front (always true where it cannot be told: a chat on screen is then taken as seen).
#[cfg(not(windows))]
pub fn app_in_front() -> bool {
    true
}

#[cfg(not(windows))]
pub fn raise(ui: &AppWindow) {
    let _ = slint::ComponentHandle::show(ui);
}

#[cfg(windows)]
#[allow(unused_imports)] // `Tray` is only held, never named
pub use imp::{app_in_front, notify, present, raise, signal_running, start, Tray};

#[cfg(windows)]
mod imp {
    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::path::Path;
    use std::ptr::{null, null_mut};
    use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
    use std::sync::{mpsc, Mutex};
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;
    use windows_sys::Win32::UI::Shell::{Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIIF_INFO, NIF_INFO, NIM_SETVERSION, NOTIFYICONDATAW, NOTIFYICON_VERSION_4};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AllowSetForegroundWindow, AppendMenuW, CreateIconFromResourceEx, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow, DispatchMessageW, FindWindowW, GetForegroundWindow, GetMessageW, IsWindowVisible, GetSystemMetrics, GetWindowThreadProcessId, IsIconic,
        LoadIconW, LoadImageW, PostMessageW, PostQuitMessage, RegisterClassExW, RegisterWindowMessageW, SetForegroundWindow, SetMenuDefaultItem, ShowWindow, TrackPopupMenuEx, TranslateMessage, HICON, IDI_APPLICATION, IMAGE_ICON, LR_DEFAULTCOLOR, MF_SEPARATOR,
        MF_STRING, MSG, SM_CXSMICON, SM_CYSMICON, SM_MENUDROPALIGNMENT, SW_RESTORE, TPM_BOTTOMALIGN, TPM_LEFTALIGN, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTALIGN, TPM_RIGHTBUTTON, WM_APP, WM_DESTROY, WM_ENDSESSION, WM_NULL, WM_QUERYENDSESSION, WNDCLASSEXW,
        WS_EX_TOOLWINDOW, WS_OVERLAPPED,
    };

    use super::{event, Ev, Labels, TrayCmd};
    use crate::AppWindow;

    /// The icon's callback, a second start asking for the window, and the end of the icon.
    const WM_TRAY: u32 = WM_APP + 1;
    const WM_TRAY_SHOW: u32 = WM_APP + 2;
    const WM_TRAY_EXIT: u32 = WM_APP + 3;
    const WM_TRAY_NOTIFY: u32 = WM_APP + 4;
    const CLASS: &str = "TenrecTray";
    const ICON_ID: u32 = 1;
    const MENU_OPEN: usize = 1;
    const MENU_QUIT: usize = 2;
    /// A 32-pixel picture of the icon, for when the program file carries none.
    const ICON_PNG: &[u8] = include_bytes!("../../../assets/icon-32.png");

    /// Whether the icon is on the screen now (so a close can hide the window).
    static PRESENT: AtomicBool = AtomicBool::new(false);
    /// The message Explorer sends when its taskbar is made again (it was restarted): the icon is added again.
    static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
    /// The icon's window (0 when there is none), and the notice waiting to be shown on its thread.
    static ICON_WINDOW: AtomicIsize = AtomicIsize::new(0);
    static NOTICE: Mutex<Option<(String, String)>> = Mutex::new(None);

    struct State {
        on_cmd: Box<dyn Fn(TrayCmd) + Send>,
        labels: Labels,
        icon: HICON,
    }

    thread_local! {
        // The icon thread's own: its window procedure has no other way to reach them.
        static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    /// The title of the invisible window: it is how a second start finds the first (one per data folder).
    fn title(data: &Path) -> Vec<u16> {
        wide(&format!("Tenrec tray|{}", data.display()))
    }

    /// The handle of the running app held by whoever started the icon. Dropping it takes the icon away.
    pub struct Tray {
        hwnd: isize,
    }

    impl Drop for Tray {
        fn drop(&mut self) {
            PRESENT.store(false, Ordering::SeqCst);
            ICON_WINDOW.store(0, Ordering::SeqCst);
            let hwnd = self.hwnd as HWND;
            delete_icon(hwnd);
            // The thread ends on its own; it is not waited for (it may be inside a menu).
            // SAFETY: the window belongs to the icon thread, and a message may be posted to it from anywhere.
            unsafe { PostMessageW(hwnd, WM_TRAY_EXIT, 0, 0) };
        }
    }

    pub fn present() -> bool {
        PRESENT.load(Ordering::SeqCst)
    }

    /// Whether a window of this process is the one in front.
    pub fn app_in_front() -> bool {
        // SAFETY: plain queries; the handle is checked before it is used.
        unsafe {
            let front = GetForegroundWindow();
            if front.is_null() {
                return false;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(front, &mut pid);
            // A window of ours that is minimized or hidden may still be the foreground one, and nobody sees it then.
            pid == GetCurrentProcessId() && IsIconic(front) == 0 && IsWindowVisible(front) != 0
        }
    }

    /// A notice from the icon (a balloon, which Windows shows as a toast), with the text it is given. Nothing when there is no icon.
    /// It is made on the icon's thread (the message is posted to its window), so any thread may ask.
    pub fn notify(title: &str, body: &str) {
        let hwnd = ICON_WINDOW.load(Ordering::SeqCst);
        if hwnd == 0 || !present() {
            return;
        }
        *NOTICE.lock().unwrap_or_else(|e| e.into_inner()) = Some((title.to_owned(), body.to_owned()));
        // SAFETY: a message may be posted to the window of another thread.
        unsafe { PostMessageW(hwnd as HWND, WM_TRAY_NOTIFY, 0, 0) };
    }

    /// Shows the notice that waits: the icon's own balloon (title up to 63 and text up to 255 UTF-16 units).
    fn show_notice(hwnd: HWND) {
        let Some((title, body)) = NOTICE.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
        let mut data = base(hwnd);
        data.uFlags = NIF_INFO;
        data.dwInfoFlags = NIIF_INFO;
        for (slot, unit) in data.szInfoTitle.iter_mut().zip(title.encode_utf16().take(63)) {
            *slot = unit;
        }
        for (slot, unit) in data.szInfo.iter_mut().zip(body.encode_utf16().take(255)) {
            *slot = unit;
        }
        // SAFETY: `data` is a filled structure of the size it says; the icon belongs to this thread's window.
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
    }

    /// Starts the icon. `None` when its window could not be made in time: the app then closes as it did before.
    pub fn start(data: &Path, labels: Labels, on_cmd: Box<dyn Fn(TrayCmd) + Send + 'static>) -> Option<Tray> {
        let (ready, answer) = mpsc::channel();
        let title = title(data);
        std::thread::Builder::new().name("tray".into()).spawn(move || thread_main(title, labels, on_cmd, ready)).ok()?;
        answer.recv_timeout(Duration::from_secs(2)).ok().flatten().map(|hwnd| Tray { hwnd })
    }

    /// Asks the app that runs on this data folder to show its window. True when it was asked.
    pub fn signal_running(data: &Path) -> bool {
        let (class, title) = (wide(CLASS), title(data));
        // SAFETY: both names are NUL-terminated; every handle is checked before it is used.
        unsafe {
            let hwnd = FindWindowW(class.as_ptr(), title.as_ptr());
            if hwnd.is_null() {
                return false;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            // This process was just started, so it may give the right to come to the front to the one that is running.
            AllowSetForegroundWindow(pid);
            PostMessageW(hwnd, WM_TRAY_SHOW, 0, 0) != 0
        }
    }

    /// Shows the window, out of the minimized state, in front.
    pub fn raise(ui: &AppWindow) {
        use slint::ComponentHandle;
        let _ = ui.show();
        ui.window().set_minimized(false);
        if !to_front(ui) {
            // A window that was hidden may not have its handle again at once.
            let weak = ui.as_weak();
            slint::Timer::single_shot(Duration::from_millis(50), move || {
                if let Some(ui) = weak.upgrade() {
                    to_front(&ui);
                }
            });
        }
    }

    fn to_front(ui: &AppWindow) -> bool {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use slint::ComponentHandle;
        let handle = ui.window().window_handle();
        let Ok(handle) = handle.window_handle() else { return false };
        let RawWindowHandle::Win32(win) = handle.as_raw() else { return false };
        let hwnd = win.hwnd.get() as HWND;
        // SAFETY: the handle was just given by the windowing library for a window that is open.
        unsafe {
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            }
            SetForegroundWindow(hwnd);
        }
        true
    }

    fn send(cmd: TrayCmd) {
        STATE.with(|state| {
            if let Some(state) = state.borrow().as_ref() {
                (state.on_cmd)(cmd);
            }
        });
    }

    fn thread_main(title: Vec<u16>, labels: Labels, on_cmd: Box<dyn Fn(TrayCmd) + Send>, ready: mpsc::Sender<Option<isize>>) {
        let class = wide(CLASS);
        // SAFETY: the class and window names are NUL-terminated and outlive the calls; the window procedure is an `extern "system"` function.
        unsafe {
            let instance = GetModuleHandleW(null());
            let window_class = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: 0,
                lpfnWndProc: Some(wndproc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: instance,
                hIcon: null_mut(),
                hCursor: null_mut(),
                hbrBackground: null_mut(),
                lpszMenuName: null(),
                lpszClassName: class.as_ptr(),
                hIconSm: null_mut(),
            };
            RegisterClassExW(&window_class);
            // A window of the top level that is never shown: a menu of the icon closes when the person clicks elsewhere only if its owner is one.
            let hwnd = CreateWindowExW(WS_EX_TOOLWINDOW, class.as_ptr(), title.as_ptr(), WS_OVERLAPPED, 0, 0, 0, 0, null_mut(), null_mut(), instance, null());
            if hwnd.is_null() {
                let _ = ready.send(None);
                return;
            }
            TASKBAR_CREATED.store(RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()), Ordering::SeqCst);
            STATE.with(|state| *state.borrow_mut() = Some(State { on_cmd, labels, icon: load_icon(instance) }));
            // The window is there: whoever started it is told before the icon is added, which can take long (Explorer busy). If nobody
            // listens any more (it gave up waiting and closes as before), this window and its icon must not appear later.
            if ready.send(Some(hwnd as isize)).is_err() {
                DestroyWindow(hwnd);
                return;
            }
            ICON_WINDOW.store(hwnd as isize, Ordering::SeqCst);
            // Until the icon is on the screen a close ends the app (`present` is false).
            PRESENT.store(add_icon(hwnd), Ordering::SeqCst);
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        PRESENT.store(false, Ordering::SeqCst);
    }

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        match msg {
            WM_TRAY => {
                match event(lparam) {
                    Some(Ev::Select) => send(TrayCmd::Open),
                    Some(Ev::Menu) => {
                        // The place of the icon, as two signed 16-bit numbers in the low and high word.
                        let (x, y) = ((wparam & 0xFFFF) as u16 as i16 as i32, ((wparam >> 16) & 0xFFFF) as u16 as i16 as i32);
                        if let Some(cmd) = show_menu(hwnd, x, y) {
                            send(cmd);
                        }
                    }
                    None => {}
                }
                0
            }
            WM_TRAY_SHOW => {
                send(TrayCmd::Open);
                0
            }
            WM_TRAY_EXIT => {
                delete_icon(hwnd);
                // SAFETY: the window belongs to this thread.
                unsafe { DestroyWindow(hwnd) };
                0
            }
            WM_DESTROY => {
                // SAFETY: called on the thread that runs the message loop.
                unsafe { PostQuitMessage(0) };
                0
            }
            WM_QUERYENDSESSION => 1,
            WM_ENDSESSION => {
                if wparam != 0 {
                    send(TrayCmd::EndSession);
                }
                0
            }
            WM_TRAY_NOTIFY => {
                show_notice(hwnd);
                0
            }
            m if m != 0 && m == TASKBAR_CREATED.load(Ordering::SeqCst) => {
                PRESENT.store(add_icon(hwnd), Ordering::SeqCst);
                0
            }
            // SAFETY: the arguments are the ones this procedure was called with.
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }

    fn base(hwnd: HWND) -> NOTIFYICONDATAW {
        // SAFETY: the structure holds integers, pointers and arrays of integers: all zeros is a valid (empty) value.
        let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = hwnd;
        data.uID = ICON_ID;
        data
    }

    /// Puts the icon on the screen (or, when one is already there, sets it again) and asks for the messages of version 4.
    fn add_icon(hwnd: HWND) -> bool {
        let Some((icon, tip)) = STATE.with(|state| state.borrow().as_ref().map(|s| (s.icon, s.labels.tip.clone()))) else { return false };
        let mut data = base(hwnd);
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        data.uCallbackMessage = WM_TRAY;
        data.hIcon = icon;
        for (slot, unit) in data.szTip.iter_mut().zip(tip.encode_utf16().take(127)) {
            *slot = unit;
        }
        // SAFETY: `data` is a filled structure of the size it says; a window of this thread owns the icon.
        let added = unsafe { Shell_NotifyIconW(NIM_ADD, &data) != 0 || Shell_NotifyIconW(NIM_MODIFY, &data) != 0 };
        if added {
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            // SAFETY: as above.
            unsafe { Shell_NotifyIconW(NIM_SETVERSION, &data) };
        }
        added
    }

    fn delete_icon(hwnd: HWND) {
        let data = base(hwnd);
        // SAFETY: only the window and the id are read; any thread may ask.
        unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
    }

    /// The program's own icon (a resource of the file) when it has one, else the picture built in, else the system's plain one.
    fn load_icon(instance: *mut c_void) -> HICON {
        // SAFETY: plain system calls; the resource id 1 is the id the build gives the icon, and the picture is a complete PNG.
        unsafe {
            let (cx, cy) = (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON));
            let own = LoadImageW(instance, 1 as *const u16, IMAGE_ICON, cx, cy, LR_DEFAULTCOLOR) as HICON;
            if !own.is_null() {
                return own;
            }
            let built = CreateIconFromResourceEx(ICON_PNG.as_ptr(), ICON_PNG.len() as u32, 1, 0x0003_0000, cx, cy, LR_DEFAULTCOLOR);
            if !built.is_null() {
                return built;
            }
            LoadIconW(null_mut(), IDI_APPLICATION)
        }
    }

    /// The menu of the icon at the place `(x, y)`; what was chosen.
    fn show_menu(hwnd: HWND, x: i32, y: i32) -> Option<TrayCmd> {
        let (open, quit) = STATE.with(|state| state.borrow().as_ref().map(|s| (wide(&s.labels.open), wide(&s.labels.quit))))?;
        // SAFETY: the strings are NUL-terminated and outlive the calls; the menu is made, used and destroyed here.
        let chosen = unsafe {
            let menu = CreatePopupMenu();
            if menu.is_null() {
                return None;
            }
            AppendMenuW(menu, MF_STRING, MENU_OPEN, open.as_ptr());
            AppendMenuW(menu, MF_SEPARATOR, 0, null());
            AppendMenuW(menu, MF_STRING, MENU_QUIT, quit.as_ptr());
            SetMenuDefaultItem(menu, MENU_OPEN as u32, 0);
            // The menu closes on a click elsewhere only when its owner is the window in front, and the next message ends its loop.
            SetForegroundWindow(hwnd);
            let side = if GetSystemMetrics(SM_MENUDROPALIGNMENT) != 0 { TPM_RIGHTALIGN } else { TPM_LEFTALIGN };
            let chosen = TrackPopupMenuEx(menu, TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY | TPM_BOTTOMALIGN | side, x, y, hwnd, null());
            PostMessageW(hwnd, WM_NULL, 0, 0);
            DestroyMenu(menu);
            chosen
        };
        match chosen as usize {
            MENU_OPEN => Some(TrayCmd::Open),
            MENU_QUIT => Some(TrayCmd::Quit),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_click_on_a_notice_brings_the_window_back_like_a_click_on_the_icon() {
        assert_eq!(event(0x405), Some(Ev::Select), "NIN_BALLOONUSERCLICK");
        assert_eq!(event(0x400), Some(Ev::Select));
        assert_eq!(event(0x7B), Some(Ev::Menu));
        // The high word of lParam is the icon id (version 4): only the low word is the event.
        assert_eq!(event(0x0001_0405), Some(Ev::Select));
        assert_eq!(event(0x404), None, "NIN_BALLOONTIMEOUT is nothing to act on");
    }

    #[test]
    fn the_icons_events_are_a_select_or_a_menu_and_nothing_else() {
        // The high word of lParam (the icon's id) does not matter.
        for select in [0x400, 0x401, 0x203, 0x0001_0400] {
            assert_eq!(event(select), Some(Ev::Select), "{select:#x}");
        }
        assert_eq!(event(0x7B), Some(Ev::Menu));
        for other in [0, 0x200, 0x201, 0x202, 0x205, 0x402] {
            assert_eq!(event(other), None, "{other:#x}");
        }
    }
}


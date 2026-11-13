//! The keeper's icon in the notification area, and its notifications.
//!
//! A hidden window on its own thread owns the icon. Every two seconds it
//! reads `settings.json` and shows or hides the icon to match the tray
//! setting. A notification about a device that didn't take its color is shown
//! from the icon; when the icon is off, it appears only for that notification.
//! Clicking the icon opens Keeplight; its menu also has "Put kept lights back
//! now". If Explorer restarts, the icon is added again.

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use keeplight_core::keeper::{Notice, Trigger};
use keeplight_core::settings::Settings;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_WARNING, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NIM_SETVERSION, NIN_BALLOONTIMEOUT, NIN_BALLOONUSERCLICK, NIN_SELECT, NOTIFYICON_VERSION_4, NOTIFYICONDATAW,
    Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW, GetCursorPos,
    GetMessageW, GetSystemMetrics, HICON, IDI_APPLICATION, IMAGE_ICON, LR_DEFAULTCOLOR, LoadIconW, LoadImageW,
    MF_STRING, MSG, PostMessageW, RegisterClassW, RegisterWindowMessageW, SM_CXSMICON, SM_CYSMICON,
    SetForegroundWindow, SetTimer, TPM_BOTTOMALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WINDOW_EX_STYLE,
    WM_APP, WM_CONTEXTMENU, WM_TIMER, WNDCLASSW, WS_OVERLAPPED,
};
use windows::core::{PCWSTR, w};

/// The icon's callback message, and the "a notice is waiting" message.
const WM_TRAY: u32 = WM_APP + 1;
const WM_NOTICE: u32 = WM_APP + 2;
/// Enter or Space on the focused icon: `NIN_SELECT | NINF_KEY` in the Win32
/// headers, which the windows crate doesn't define.
const NIN_KEYSELECT: u32 = NIN_SELECT | 0x1;
const SETTINGS_TIMER: usize = 1;
const MENU_OPEN: usize = 1;
const MENU_PUT_BACK: usize = 2;

/// A handle the keeper uses to show notifications.
#[derive(Clone)]
pub struct Tray {
    hwnd: isize,
    pending: Arc<Mutex<Option<Notice>>>,
}

impl Tray {
    /// Shows `notice` as a Windows notification.
    pub fn notify(&self, notice: Notice) {
        *self.pending.lock().unwrap_or_else(|e| e.into_inner()) = Some(notice);
        // SAFETY: the window lives for the life of the process.
        let _ = unsafe { PostMessageW(Some(HWND(self.hwnd as *mut _)), WM_NOTICE, WPARAM(0), LPARAM(0)) };
    }
}

struct State {
    hwnd: HWND,
    icon: HICON,
    /// The icon is in the notification area.
    shown: bool,
    /// It is there only for a notification, because the setting is off.
    temporary: bool,
    taskbar_created: u32,
    settings_path: PathBuf,
    triggers: Sender<Trigger>,
    pending: Arc<Mutex<Option<Notice>>>,
}

// Message handlers can run again while Windows is inside a tray call made by
// one of them, so they only ever try to borrow, and skip when they can't.
thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// Starts the tray thread, or says why Windows refused its hidden window.
pub fn start(triggers: Sender<Trigger>, settings_path: PathBuf) -> Result<Tray, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let pending = Arc::new(Mutex::new(None));
    let shared = pending.clone();
    std::thread::spawn(move || {
        let hwnd = match create_window() {
            Ok(h) => h,
            Err(e) => {
                let _ = tx.send(Err(e));
                return;
            }
        };
        // SAFETY: plain message registration.
        let taskbar_created = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
        STATE.with(|s| {
            *s.borrow_mut() = Some(State {
                hwnd,
                icon: load_icon(),
                shown: false,
                temporary: false,
                taskbar_created,
                settings_path,
                triggers,
                pending: shared,
            })
        });
        sync_with_settings();
        // SAFETY: the window belongs to this thread.
        unsafe { SetTimer(Some(hwnd), SETTINGS_TIMER, 2000, None) };
        let _ = tx.send(Ok(hwnd.0 as isize));
        let mut msg = MSG::default();
        // SAFETY: a standard message loop for this thread's window.
        while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
            unsafe { DispatchMessageW(&msg) };
        }
    });
    let hwnd = rx.recv().map_err(|_| "the tray thread stopped".to_string())??;
    Ok(Tray { hwnd, pending })
}

fn create_window() -> Result<HWND, String> {
    // SAFETY: registers a class whose procedure is `window_proc`, then makes a
    // window of it that is never shown.
    unsafe {
        let instance = GetModuleHandleW(None).map_err(|e| e.to_string())?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: w!("KeeplightTray"),
            ..Default::default()
        };
        RegisterClassW(&class);
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("KeeplightTray"),
            w!("Keeplight"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .map_err(|e| e.to_string())
    }
}

/// The exe's own icon (resource 1, embedded by build.rs) at the small size.
fn load_icon() -> HICON {
    // SAFETY: loads a shared icon resource from this module.
    unsafe {
        let instance = GetModuleHandleW(None).ok();
        let (cx, cy) = (GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON));
        match LoadImageW(
            instance.map(Into::into),
            PCWSTR(std::ptr::without_provenance(1)), /* MAKEINTRESOURCE(1) */
            IMAGE_ICON,
            cx,
            cy,
            LR_DEFAULTCOLOR,
        ) {
            Ok(h) => HICON(h.0),
            Err(_) => LoadIconW(None, IDI_APPLICATION).unwrap_or_default(),
        }
    }
}

fn base_data(state: &State) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: state.hwnd,
        uID: 1,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP,
        uCallbackMessage: WM_TRAY,
        hIcon: state.icon,
        ..Default::default()
    };
    copy_into(&mut data.szTip, "Keeplight is keeping your lights");
    data
}

fn add_icon(state: &mut State) {
    let mut data = base_data(state);
    // SAFETY: `data` is a fully initialised NOTIFYICONDATAW for our window.
    unsafe {
        if Shell_NotifyIconW(NIM_ADD, &data).as_bool() {
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            let _ = Shell_NotifyIconW(NIM_SETVERSION, &data);
            state.shown = true;
        }
    }
}

fn remove_icon(state: &mut State) {
    let data = base_data(state);
    // SAFETY: removes the icon this window added.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
    }
    state.shown = false;
    state.temporary = false;
}

fn sync_with_settings() {
    STATE.with(|s| {
        let Ok(mut guard) = s.try_borrow_mut() else { return };
        let Some(state) = guard.as_mut() else { return };
        let want = Settings::load(&state.settings_path).tray_icon;
        if want && !state.shown {
            add_icon(state);
        } else if want && state.temporary {
            state.temporary = false;
        } else if !want && state.shown && !state.temporary {
            remove_icon(state);
        }
    });
}

fn show_notice() {
    STATE.with(|s| {
        let Ok(mut guard) = s.try_borrow_mut() else { return };
        let Some(state) = guard.as_mut() else { return };
        let Some(notice) = state.pending.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
        if !state.shown {
            add_icon(state);
            state.temporary = state.shown;
        }
        let mut data = base_data(state);
        data.uFlags |= NIF_INFO;
        data.dwInfoFlags = NIIF_WARNING;
        copy_into(&mut data.szInfoTitle, &notice.title);
        copy_into(&mut data.szInfo, &notice.body);
        // SAFETY: modifies the icon this window added.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
        }
    });
}

fn notification_closed() {
    STATE.with(|s| {
        if let Ok(mut guard) = s.try_borrow_mut()
            && let Some(state) = guard.as_mut()
            && state.temporary
        {
            remove_icon(state);
        }
    });
}

fn open_keeplight() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).spawn();
    }
}

fn show_menu(hwnd: HWND) {
    // SAFETY: a popup menu owned by this thread's window, destroyed after use.
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let _ = AppendMenuW(menu, MF_STRING, MENU_OPEN, w!("Open Keeplight"));
        let _ = AppendMenuW(menu, MF_STRING, MENU_PUT_BACK, w!("Put kept lights back now"));
        let mut at = POINT::default();
        let _ = GetCursorPos(&mut at);
        // Without this the menu doesn't close when you click elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let chosen =
            TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, at.x, at.y, None, hwnd, None);
        let _ = DestroyMenu(menu);
        match chosen.0 as usize {
            MENU_OPEN => open_keeplight(),
            MENU_PUT_BACK => STATE.with(|s| {
                if let Ok(guard) = s.try_borrow()
                    && let Some(state) = guard.as_ref()
                {
                    let _ = state.triggers.send(Trigger::Asked);
                }
            }),
            _ => {}
        }
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER if wparam.0 == SETTINGS_TIMER => sync_with_settings(),
        WM_NOTICE => show_notice(),
        // With NOTIFYICON_VERSION_4 the event is in the low word of lparam.
        WM_TRAY => match (lparam.0 as u32) & 0xffff {
            NIN_SELECT | NIN_KEYSELECT | NIN_BALLOONUSERCLICK => {
                open_keeplight();
                if (lparam.0 as u32) & 0xffff == NIN_BALLOONUSERCLICK {
                    notification_closed();
                }
            }
            WM_CONTEXTMENU => show_menu(hwnd),
            NIN_BALLOONTIMEOUT => notification_closed(),
            _ => {}
        },
        _ => {
            let restarted =
                STATE.with(|s| s.try_borrow().is_ok_and(|g| g.as_ref().is_some_and(|st| st.taskbar_created == msg)));
            if restarted {
                // Explorer restarted and forgot every icon.
                STATE.with(|s| {
                    if let Ok(mut guard) = s.try_borrow_mut()
                        && let Some(state) = guard.as_mut()
                    {
                        state.shown = false;
                        state.temporary = false;
                    }
                });
                sync_with_settings();
                return LRESULT(0);
            }
            // SAFETY: default handling for everything else.
            return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
        }
    }
    LRESULT(0)
}

/// Copies `text` into a fixed UTF-16 field, cut short to fit with its NUL.
fn copy_into(field: &mut [u16], text: &str) {
    let units: Vec<u16> = text.encode_utf16().take(field.len() - 1).collect();
    field[..units.len()].copy_from_slice(&units);
    field[units.len()] = 0;
}

#[cfg(test)]
mod tests {
    use super::copy_into;

    #[test]
    fn long_text_is_cut_to_fit_with_its_nul() {
        let mut field = [7u16; 5];
        copy_into(&mut field, "abcdefgh");
        assert_eq!(String::from_utf16_lossy(&field[..4]), "abcd");
        assert_eq!(field[4], 0);
        let mut field = [7u16; 5];
        copy_into(&mut field, "ab");
        assert_eq!(&field[..3], &[97, 98, 0]);
    }
}

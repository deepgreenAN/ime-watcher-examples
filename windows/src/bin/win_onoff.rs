use std::sync::mpsc::{SyncSender, sync_channel};

use windows::Win32::{
    Devices::HumanInterfaceDevice::{HID_USAGE_GENERIC_KEYBOARD, HID_USAGE_PAGE_GENERIC},
    Foundation::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Accessibility::*,
        Input::{Ime::ImmGetDefaultIMEWnd, *},
        WindowsAndMessaging::*,
    },
};

use windows::core::{Error as WinError, w};

use once_cell::sync::OnceCell;

static GET_OPEN_STATUS_SENDER: OnceCell<SyncSender<GetOpenStatusNotification>> = OnceCell::new();

const IMC_GETOPENSTATUS: usize = 0x0005;

const VK_JP_IME_ON: u16 = 244; // VK_OEM_ENLW
const VK_JP_IME_OFF: u16 = 243; // VK_OEM_AUTO
const VK_JP_EISU: u16 = 240; // VK_OEM_ATTN

/// タイミングの通知用
struct GetOpenStatusNotification;

/// フォーカス変更時に実行されるコールバック
extern "system" fn win_event_proc(
    _hwineventhook: HWINEVENTHOOK,
    event: u32,
    _hwnd: HWND,
    _idobject: i32,
    _idchild: i32,
    _ideventthread: u32,
    _dwmseventtime: u32,
) {
    if event == EVENT_SYSTEM_FOREGROUND {
        //
        if let Some(sender) = GET_OPEN_STATUS_SENDER.get() {
            let _ = sender.try_send(GetOpenStatusNotification);
        }
    }
}

#[derive(Debug)]
struct GetOpenStatusError;

impl std::fmt::Display for GetOpenStatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "GetOpenStatusError")
    }
}

impl std::error::Error for GetOpenStatusError {}

// SendMessageを行うため、必ずUIスレッド、フックなどとは異なるスレッドから呼ぶ。
fn get_open_status() -> Result<String, GetOpenStatusError> {
    unsafe {
        let foreground_hwnd = GetForegroundWindow();

        if foreground_hwnd.is_invalid() {
            return Err(GetOpenStatusError);
        }

        let thread_id = GetWindowThreadProcessId(foreground_hwnd, None);

        // 前面ウィンドウのGUIスレッド情報
        let mut gui_info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };

        let target_hwnd = if GetGUIThreadInfo(thread_id, &mut gui_info).is_ok()
            && !gui_info.hwndFocus.is_invalid()
        {
            gui_info.hwndFocus
        } else {
            foreground_hwnd
        };

        // IME管理ウィンドウの取得
        let target_hwnd_ime = ImmGetDefaultIMEWnd(target_hwnd);

        if target_hwnd_ime.is_invalid() {
            return Err(GetOpenStatusError);
        }

        let result = {
            let mut result: usize = 0;
            if SendMessageTimeoutW(
                target_hwnd_ime,
                WM_IME_CONTROL,
                WPARAM(IMC_GETOPENSTATUS),
                LPARAM(0),
                SMTO_NORMAL | SMTO_ABORTIFHUNG,
                100,
                Some(&mut result),
            )
            .0 == 0
            {
                return Err(GetOpenStatusError);
            }

            result
        };

        if result == 0 {
            Ok("ime-off".to_string())
        } else {
            Ok("ime-on".to_string())
        }
    }
}

// windowprocコールバック
extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_INPUT => {
                let mut raw_input_size = 0_u32;
                GetRawInputData(
                    HRAWINPUT(lparam.0 as _),
                    RID_INPUT,
                    None,
                    &mut raw_input_size,
                    std::mem::size_of::<RAWINPUTHEADER>() as u32,
                ); // サイズを取得する

                let mut buf = vec![0_u8; raw_input_size as usize];
                if GetRawInputData(
                    HRAWINPUT(lparam.0 as _),
                    RID_INPUT,
                    Some(buf.as_mut_ptr() as _),
                    &mut raw_input_size,
                    std::mem::size_of::<RAWINPUTHEADER>() as u32,
                ) == raw_input_size
                {
                    let raw_input = &*(buf.as_ptr() as *const RAWINPUT);
                    if raw_input.header.dwType == RIM_TYPEKEYBOARD.0 {
                        let keyboard = raw_input.data.keyboard;

                        if keyboard.Message == WM_KEYDOWN {
                            // 特定の環境ではトグルとはならないためその都度取得する
                            if let VK_JP_IME_ON | VK_JP_IME_OFF | VK_JP_EISU = keyboard.VKey {
                                //
                                if let Some(sender) = GET_OPEN_STATUS_SENDER.get() {
                                    let _ = sender.try_send(GetOpenStatusNotification);
                                }
                            }
                        }
                    }
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

// windowsのuiループ
pub fn ui_loop() -> Result<(), WinError> {
    unsafe {
        // win_event_hook
        let hook = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(win_event_proc),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );

        // window
        let hinstance = GetModuleHandleW(None)?;

        let class_name = w!("ImeObserverWindow");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance.into(),
            lpszClassName: class_name,
            ..Default::default()
        };

        let atom = RegisterClassExW(&wc);
        debug_assert!(atom != 0);

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class_name,
            w!("instance"),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(hinstance.into()),
            None,
        )?;

        // rawinput
        let rid = RAWINPUTDEVICE {
            usUsagePage: HID_USAGE_PAGE_GENERIC,
            usUsage: HID_USAGE_GENERIC_KEYBOARD,
            dwFlags: RIDEV_INPUTSINK,
            hwndTarget: hwnd,
        };

        RegisterRawInputDevices(&[rid], std::mem::size_of::<RAWINPUTDEVICE>() as u32)?;

        let _ = ShowWindow(hwnd, SW_HIDE);

        // メッセージループ
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        if !hook.is_invalid() {
            let _ = UnhookWinEvent(hook);
        }
    }

    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (sender, receiver) = sync_channel(1);

    GET_OPEN_STATUS_SENDER.set(sender).unwrap();

    std::thread::spawn(move || -> Result<(), GetOpenStatusError> {
        while let Ok(_msg) = receiver.recv() {
            std::thread::sleep(std::time::Duration::from_millis(50));

            let open_status = get_open_status()?;

            println!("ime_open_status: {open_status}");
        }

        Ok(())
    });

    ui_loop()?;

    Ok(())
}

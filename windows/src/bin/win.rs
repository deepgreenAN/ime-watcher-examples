use std::{
    collections::HashMap,
    sync::mpsc::{SyncSender, sync_channel},
};

use windows::Win32::{
    Devices::HumanInterfaceDevice::{HID_USAGE_GENERIC_KEYBOARD, HID_USAGE_PAGE_GENERIC},
    Foundation::*,
    Globalization::LCIDToLocaleName,
    System::{LibraryLoader::GetModuleHandleW, SystemServices::LOCALE_NAME_MAX_LENGTH},
    UI::{
        Accessibility::*,
        Input::{KeyboardAndMouse::*, *},
        WindowsAndMessaging::*,
    },
};
use windows::core::{Error as WinError, w};

use once_cell::sync::OnceCell;

static GET_KEYBOARD_LAYOUT_SENDER: OnceCell<SyncSender<GetKeyboardLayoutNotification>> =
    OnceCell::new();

const VK_CONTROL: u16 = windows::Win32::UI::Input::KeyboardAndMouse::VK_CONTROL.0 as _;
const VK_LWIN: u16 = windows::Win32::UI::Input::KeyboardAndMouse::VK_LWIN.0 as _;
const VK_RWIN: u16 = windows::Win32::UI::Input::KeyboardAndMouse::VK_RWIN.0 as _;
const VK_MENU: u16 = windows::Win32::UI::Input::KeyboardAndMouse::VK_MENU.0 as _;

/// タイミングの通知用
struct GetKeyboardLayoutNotification;

#[derive(Debug)]
struct GetKeyboardLayoutError;

impl std::fmt::Display for GetKeyboardLayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "GetKeyboardLayoutError")
    }
}

impl std::error::Error for GetKeyboardLayoutError {}

/// フォーカス変更時に実行されるコールバック。これはキーによる変更でも一部の場合で呼ばれるが、入力メソッド変更の小ウィンドウに対して行われるため、長押しした場合などに想定した挙動とはならない。
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
        if let Some(sender) = GET_KEYBOARD_LAYOUT_SENDER.get() {
            let _ = sender.try_send(GetKeyboardLayoutNotification);
        }
    }
}

/// 特定の修飾キーのリリースを検出する。
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
                        // println!("vkey: {}", keyboard.VKey);
                        // println!("make code: {}", keyboard.MakeCode);

                        if keyboard.Message == WM_KEYUP {
                            // 修飾キーであった場合
                            if let VK_CONTROL | VK_LWIN | VK_RWIN | VK_MENU = keyboard.VKey {
                                //
                                if let Some(sender) = GET_KEYBOARD_LAYOUT_SENDER.get() {
                                    let _ = sender.try_send(GetKeyboardLayoutNotification);
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

// lang_idをlocaleに変更する。initialize_locale_map内でのみ利用する。
fn lang_id2locale(lang_id: u16) -> Option<String> {
    // MAKELCID(lang_id, SORT_DEFAULT) 相当
    let locale_id = lang_id as u32;

    let mut utf_16_buf = [0_u16; LOCALE_NAME_MAX_LENGTH as usize]; // 暫定的に最大まで作成する

    let written_len = unsafe { LCIDToLocaleName(locale_id, Some(&mut utf_16_buf), 0) };

    if written_len != 0 {
        Some(String::from_utf16_lossy(
            &utf_16_buf[..written_len as usize - 1],
        )) // written_lenから終端文字列を引いたもの
    } else {
        None
    }
}

/// lang_id -> locale のマップを作成する
fn initialize_locale_map() -> Result<HashMap<u16, String>, GetKeyboardLayoutError> {
    unsafe {
        // 言語IDのリストを取得する
        let size = GetKeyboardLayoutList(None);

        let mut hkl_list = vec![HKL(std::ptr::null_mut()); size as usize];

        GetKeyboardLayoutList(Some(&mut hkl_list));

        let lang_id_list: Vec<u16> = hkl_list
            .into_iter()
            .map(|hkl| (hkl.0 as usize & 0xFFFF) as u16)
            .collect();

        let mut locale_map: HashMap<u16, String> = HashMap::new();

        for lang_id in lang_id_list.iter() {
            if let Some(locale) = lang_id2locale(*lang_id) {
                locale_map.insert(*lang_id, locale);
            } else {
                return Err(GetKeyboardLayoutError);
            }
        }

        Ok(locale_map)
    }
}

/// ループの中で呼ぶ。
fn get_keyboard_layout(
    locale_map: &HashMap<u16, String>,
) -> Result<String, GetKeyboardLayoutError> {
    unsafe {
        let foreground_hwnd = GetForegroundWindow();

        if foreground_hwnd.is_invalid() {
            return Err(GetKeyboardLayoutError);
        }

        let thread_id = GetWindowThreadProcessId(foreground_hwnd, None);

        // 前面ウィンドウのGUIスレッド情報
        let mut gui_info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };

        let target_thread_id = if GetGUIThreadInfo(thread_id, &mut gui_info).is_ok()
            && !gui_info.hwndFocus.is_invalid()
        {
            GetWindowThreadProcessId(gui_info.hwndFocus, None)
        } else {
            thread_id
        };

        let hkl = GetKeyboardLayout(target_thread_id);

        if hkl.0 as usize == 0 {
            // コンソールアプリなどで起こる。
            return Err(GetKeyboardLayoutError);
        }

        match locale_map.get(&((hkl.0 as usize & 0xFFFF) as u16)) {
            Some(locale) => Ok(locale.to_owned()),
            None => Err(GetKeyboardLayoutError),
        }
    }
}

// windowsのuiループ。
pub fn ui_loop() -> Result<(), WinError> {
    unsafe {
        // hook
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

    GET_KEYBOARD_LAYOUT_SENDER.set(sender).unwrap();

    let locale_map = initialize_locale_map()?;

    std::thread::spawn(move || -> Result<(), GetKeyboardLayoutError> {
        while let Ok(_msg) = receiver.recv() {
            std::thread::sleep(std::time::Duration::from_millis(50));

            match get_keyboard_layout(&locale_map) {
                Ok(keyboard_layout) => {
                    println!("keyboard_layout: {keyboard_layout}");
                }
                Err(e) => {
                    println!("{e}"); // コンソールアプリなどではこちらになることがある。
                }
            }
        }

        Ok(())
    });

    ui_loop()?;

    Ok(())
}

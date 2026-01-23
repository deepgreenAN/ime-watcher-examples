use std::ffi::c_void;
use std::sync::mpsc::SyncSender;
use std::time::Duration;

use core_foundation::{
    base::{CFRelease, CFType, CFTypeRef, TCFType},
    dictionary::CFDictionaryRef,
    runloop::{CFRunLoop, CFRunLoopRunResult, kCFRunLoopDefaultMode},
    string::{CFString, CFStringRef},
};
use core_foundation_sys::notification_center::{
    CFNotificationCenterAddObserver, CFNotificationCenterGetDistributedCenter,
    CFNotificationCenterRef, CFNotificationCenterRemoveObserver, CFNotificationName,
    CFNotificationSuspensionBehavior,
};
use once_cell::sync::OnceCell;

static GET_IME_MESSAGE_SENDER: OnceCell<SyncSender<GetImeMessage>> = OnceCell::new();

struct GetImeMessage;

type TISInputSourceRef = *const c_void;

// #[allow(non_upper_case_globals)]
// const CFNotificationSuspensionBehaviorCoalesce: CFNotificationSuspensionBehavior = 2;

#[allow(non_upper_case_globals)]
const CFNotificationSuspensionBehaviorDeliverImmediately: CFNotificationSuspensionBehavior = 4;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    static kTISPropertyInputSourceID: CFStringRef;

    fn TISCopyCurrentKeyboardInputSource() -> TISInputSourceRef;
    fn TISGetInputSourceProperty(
        input_source: TISInputSourceRef,
        property_key: CFStringRef,
    ) -> CFTypeRef;

    static kTISNotifySelectedKeyboardInputSourceChanged: CFStringRef;
}

extern "C" fn callback(
    _center: CFNotificationCenterRef,
    _observer: *mut c_void,
    _name: CFNotificationName,
    _object: *const c_void,
    _user_info: CFDictionaryRef,
) {
    if let Some(message_sender) = GET_IME_MESSAGE_SENDER.get() {
        let _ = message_sender.try_send(GetImeMessage);
    }
}

#[derive(Debug)]
struct MacError(String);

impl std::fmt::Display for MacError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MacError: {}", self.0)
    }
}

impl std::error::Error for MacError {}

fn get_current_input_source() -> Result<String, MacError> {
    unsafe {
        let source = TISCopyCurrentKeyboardInputSource();

        let input_source = CFType::wrap_under_get_rule(TISGetInputSourceProperty(
            source,
            kTISPropertyInputSourceID,
        ))
        .downcast_into::<CFString>()
        .ok_or(MacError("Type Miss match.".to_string()))?
        .to_string();

        CFRelease(source);

        Ok(input_source)
    }
}

fn run_loop() -> Result<(), MacError> {
    unsafe {
        let observer_ptr = Box::into_raw(Box::new(1)); // observer自体はなんでも良い

        let notify_center = CFNotificationCenterGetDistributedCenter();

        CFNotificationCenterAddObserver(
            notify_center,
            observer_ptr as _,
            callback,
            kTISNotifySelectedKeyboardInputSourceChanged,
            std::ptr::null(),
            CFNotificationSuspensionBehaviorDeliverImmediately,
        );

        // run_loop
        while let run_result =
            CFRunLoop::run_in_mode(kCFRunLoopDefaultMode, Duration::from_secs(1), true)
            && run_result != CFRunLoopRunResult::Stopped
        {
            // println!("{run_result:?}");
        }

        // 終了処理
        CFNotificationCenterRemoveObserver(
            notify_center,
            observer_ptr as _,
            kTISNotifySelectedKeyboardInputSourceChanged,
            std::ptr::null(),
        );
        let _ = Box::from_raw(observer_ptr);
    }

    Ok(())
}

fn main() -> Result<(), MacError> {
    use std::sync::mpsc::sync_channel;

    let (message_sender, message_receiver) = sync_channel(1);

    let _ = GET_IME_MESSAGE_SENDER.set(message_sender);

    let mut pre_ime_status = "".to_string();

    std::thread::spawn(move || {
        while let Ok(_m) = message_receiver.recv() {
            std::thread::sleep(Duration::from_millis(40));
            if let Ok(ime_status) = get_current_input_source() {
                //
                if pre_ime_status != ime_status {
                    println!("{ime_status}");
                    pre_ime_status = ime_status;
                }
            }
        }
    });

    // 必ずメインスレッドとする。
    run_loop()?;

    Ok(())
}

use std::sync::{
    OnceLock,
    atomic::{AtomicBool, Ordering},
};

use objc2::{
    MainThreadMarker,
    runtime::{AnyClass, AnyObject, Imp, Sel},
    sel,
};
use objc2_app_kit::NSApplication;

static CONTEXT: OnceLock<eframe::egui::Context> = OnceLock::new();
static QUIT_REQUESTED: AtomicBool = AtomicBool::new(false);

unsafe extern "C-unwind" fn application_should_terminate(
    _delegate: &AnyObject,
    _selector: Sel,
    _application: *mut NSApplication,
) -> usize {
    QUIT_REQUESTED.store(true, Ordering::Release);
    if let Some(context) = CONTEXT.get() {
        context.request_repaint();
    }
    0 // NSTerminateCancel: the workbench handles saves and cleanup first.
}

pub fn install(context: eframe::egui::Context) {
    let mtm = MainThreadMarker::new().expect("macOS app must start on the main thread");
    let delegate = NSApplication::sharedApplication(mtm)
        .delegate()
        .expect("winit application delegate must exist");
    // SAFETY: a protocol object is an Objective-C object retained by `delegate`.
    let object = unsafe { &*(objc2::rc::Retained::as_ptr(&delegate).cast::<AnyObject>()) };
    let class = object.class();
    CONTEXT
        .set(context)
        .expect("quit handler already installed");
    assert!(
        install_on_class(class),
        "winit already handles native quit; review quit integration"
    );
}

fn install_on_class(class: &AnyClass) -> bool {
    // SAFETY: this callback matches NSApplicationDelegate's documented signature;
    // adding it to winit's delegate preserves its existing lifecycle methods.
    let installed = unsafe {
        let implementation: Imp = std::mem::transmute(
            application_should_terminate
                as unsafe extern "C-unwind" fn(&AnyObject, Sel, *mut NSApplication) -> usize,
        );
        objc2::ffi::class_addMethod(
            class as *const _ as *mut _,
            sel!(applicationShouldTerminate:),
            implementation,
            c"Q@:@".as_ptr(),
        )
    };
    installed.as_bool()
}

pub fn take_request() -> bool {
    QUIT_REQUESTED.swap(false, Ordering::AcqRel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, atomic::AtomicUsize};

    #[test]
    fn native_termination_requests_quit_and_wakes_the_workbench() {
        let context = eframe::egui::Context::default();
        let wakeups = Arc::new(AtomicUsize::new(0));
        let observed = wakeups.clone();
        context.set_request_repaint_callback(move |_| {
            observed.fetch_add(1, Ordering::Relaxed);
        });
        CONTEXT.set(context).unwrap();
        let superclass = AnyClass::get(c"NSObject").unwrap();
        let class = objc2::runtime::ClassBuilder::new(c"IntelliGenQuitTestDelegate", superclass)
            .unwrap()
            .register();
        assert!(install_on_class(class));
        // SAFETY: the test class inherits NSObject and owns the method just installed.
        let reply: usize = unsafe {
            let object: *mut AnyObject = objc2::msg_send![class, new];
            let reply = objc2::msg_send![object, applicationShouldTerminate: std::ptr::null_mut::<NSApplication>()];
            let _: () = objc2::msg_send![object, release];
            reply
        };
        assert_eq!(reply, 0);
        assert!(take_request());
        assert!(!take_request());
        assert!(wakeups.load(Ordering::Relaxed) > 0);
    }
}

//! Receive Finder document events independently of winit's application delegate.
//! Install before the event loop so cold launches and later opens use one queue.
use std::{
    cell::RefCell,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

use eframe::egui;
use objc2::{
    MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained,
    runtime::NSObjectProtocol, sel,
};
use objc2_app_kit::{NSApplication, NSApplicationWillFinishLaunchingNotification};
use objc2_foundation::{
    NSAppleEventDescriptor, NSAppleEventManager, NSNotification, NSNotificationCenter, NSObject,
};

static REQUESTS: Mutex<Vec<Vec<PathBuf>>> = Mutex::new(Vec::new());
static CONTEXT: OnceLock<egui::Context> = OnceLock::new();
thread_local! {
    static HANDLER: RefCell<Option<Retained<DocumentHandler>>> = const { RefCell::new(None) };
}

define_class!(
    // SAFETY: NSObject has no additional subclass requirements or instance state.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = ()]
    struct DocumentHandler;

    impl DocumentHandler {
        #[unsafe(method(applicationWillFinishLaunching:))]
        fn will_finish_launching(&self, _notification: &NSNotification) {
            // AppKit installs its defaults during launch. Register after those
            // defaults, but before it dispatches the initial open-document event.
            // SAFETY: This selector has the required two-descriptor signature;
            // HANDLER retains this target for the main thread's lifetime.
            unsafe {
                NSAppleEventManager::sharedAppleEventManager()
                    .setEventHandler_andSelector_forEventClass_andEventID(
                        self, sel!(handleOpenDocuments:withReplyEvent:),
                        u32::from_be_bytes(*b"aevt"), u32::from_be_bytes(*b"odoc"),
                    );
            }
        }

        #[unsafe(method(handleOpenDocuments:withReplyEvent:))]
        fn handle_open_documents(&self, event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
            // The direct object is a list of file descriptors. Foundation resolves
            // aliases and decodes file URLs, including spaces and Unicode paths.
            let Some(list) = event.paramDescriptorForKeyword(u32::from_be_bytes(*b"----")) else { return; };
            let paths = (1..=list.numberOfItems()).filter_map(|index| {
                let url = list.descriptorAtIndex(index)?.fileURLValue()?;
                if !url.isFileURL() { return None; }
                Some(PathBuf::from(url.path()?.to_string()))
            }).collect::<Vec<_>>();
            if !paths.is_empty() {
                REQUESTS.lock().unwrap().push(paths);
                if let Some(ctx) = CONTEXT.get() { ctx.request_repaint(); }
            }
        }
    }

    // SAFETY: NSObjectProtocol has no additional requirements.
    unsafe impl NSObjectProtocol for DocumentHandler {}
);

pub fn install() {
    let mtm = MainThreadMarker::new().expect("document events require the main thread");
    // Initialize AppKit before observing its launch notification.
    let _ = NSApplication::sharedApplication(mtm);
    let allocated = DocumentHandler::alloc(mtm).set_ivars(());
    // SAFETY: NSObject's init returns an initialized instance of this class.
    let handler: Retained<DocumentHandler> = unsafe { msg_send![super(allocated), init] };
    // SAFETY: The observer selector accepts an NSNotification and the target
    // remains alive for the main thread's lifetime.
    unsafe {
        NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
            &handler,
            sel!(applicationWillFinishLaunching:),
            Some(NSApplicationWillFinishLaunchingNotification),
            None,
        );
    }
    HANDLER.with(|slot| *slot.borrow_mut() = Some(handler));
}

pub fn set_context(ctx: &egui::Context) {
    let _ = CONTEXT.set(ctx.clone());
    ctx.request_repaint();
}

pub fn take_open_requests() -> Vec<Vec<PathBuf>> {
    std::mem::take(&mut *REQUESTS.lock().unwrap())
}

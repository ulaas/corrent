//! macOS: receive `magnet:` URLs and opened `.torrent` files via Apple Events.
//!
//! winit/eframe do not surface `application:openURLs:`, so we register handlers with the shared
//! `NSAppleEventManager` for kInternetEventClass/kAEGetURL ('GURL') and kCoreEventClass/
//! kAEOpenDocuments ('aevt'/'odoc'). Handlers are installed before the event loop starts (to catch
//! the launch event) and again on the first frame (AppKit may replace them while launching).

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send, sel};
use objc2_foundation::{NSAppleEventManager, NSString, NSURL};

use super::single_instance::IpcMessage;
use crate::engine::types::AddSource;

const fn fourcc(s: &[u8; 4]) -> u32 {
    ((s[0] as u32) << 24) | ((s[1] as u32) << 16) | ((s[2] as u32) << 8) | (s[3] as u32)
}

const K_INTERNET_EVENT_CLASS: u32 = fourcc(b"GURL");
const K_AE_GET_URL: u32 = fourcc(b"GURL");
const K_CORE_EVENT_CLASS: u32 = fourcc(b"aevt");
const K_AE_OPEN_DOCUMENTS: u32 = fourcc(b"odoc");
const KEY_DIRECT_OBJECT: u32 = fourcc(b"----");

pub struct Ivars {
    tx: Mutex<Sender<IpcMessage>>,
    ctx: Arc<OnceLock<egui::Context>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "CorrentAppleEventHandler"]
    #[ivars = Ivars]
    pub struct AppleEventHandler;

    impl AppleEventHandler {
        #[unsafe(method(handleGetURLEvent:withReplyEvent:))]
        fn handle_get_url(&self, event: &AnyObject, _reply: &AnyObject) {
            let desc: Option<Retained<AnyObject>> = unsafe { msg_send![event, paramDescriptorForKeyword: KEY_DIRECT_OBJECT] };
            let Some(desc) = desc else { return };
            let s: Option<Retained<NSString>> = unsafe { msg_send![&*desc, stringValue] };
            if let Some(s) = s {
                self.deliver(AddSource::parse(&s.to_string()));
            }
        }

        #[unsafe(method(handleOpenDocumentsEvent:withReplyEvent:))]
        fn handle_open_documents(&self, event: &AnyObject, _reply: &AnyObject) {
            let list: Option<Retained<AnyObject>> = unsafe { msg_send![event, paramDescriptorForKeyword: KEY_DIRECT_OBJECT] };
            let Some(list) = list else { return };
            let n: isize = unsafe { msg_send![&*list, numberOfItems] };
            // A single file may arrive as a plain descriptor rather than a list.
            let items: Vec<Retained<AnyObject>> = if n <= 0 {
                vec![list]
            } else {
                (1..=n)
                    .filter_map(|i| unsafe { msg_send![&*list, descriptorAtIndex: i] })
                    .collect()
            };
            for item in items {
                let url: Option<Retained<NSURL>> = unsafe { msg_send![&*item, fileURLValue] };
                let path = url.and_then(|u| u.path()).map(|p| p.to_string());
                if let Some(p) = path {
                    self.deliver(Some(AddSource::File(p.into())));
                }
            }
        }
    }
);

impl AppleEventHandler {
    fn deliver(&self, src: Option<AddSource>) {
        let Some(src) = src else { return };
        {
            let tx = crate::util::lock(&self.ivars().tx);
            let _ = tx.send(IpcMessage::Add(src));
            let _ = tx.send(IpcMessage::Focus);
        }
        if let Some(c) = self.ivars().ctx.get() {
            c.request_repaint();
        }
    }

    fn new(tx: Sender<IpcMessage>, ctx: Arc<OnceLock<egui::Context>>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(Ivars { tx: Mutex::new(tx), ctx });
        unsafe { msg_send![super(this), init] }
    }
}

thread_local! {
    // Apple Events are delivered on the main thread, where install/reinstall are called.
    static HANDLER: std::cell::OnceCell<Retained<AppleEventHandler>> = const { std::cell::OnceCell::new() };
}

fn register(handler: &AppleEventHandler) {
    let mgr = NSAppleEventManager::sharedAppleEventManager();
    let obj: &AnyObject = handler.as_ref();
    unsafe {
        let _: () = msg_send![
            &*mgr,
            setEventHandler: obj,
            andSelector: sel!(handleGetURLEvent:withReplyEvent:),
            forEventClass: K_INTERNET_EVENT_CLASS,
            andEventID: K_AE_GET_URL
        ];
        let _: () = msg_send![
            &*mgr,
            setEventHandler: obj,
            andSelector: sel!(handleOpenDocumentsEvent:withReplyEvent:),
            forEventClass: K_CORE_EVENT_CLASS,
            andEventID: K_AE_OPEN_DOCUMENTS
        ];
    }
}

/// Create the handler (once) and register it with the Apple Event manager. Main thread only.
pub fn install(tx: &Sender<IpcMessage>, ctx: &Arc<OnceLock<egui::Context>>) {
    HANDLER.with(|cell| {
        let handler = cell.get_or_init(|| AppleEventHandler::new(tx.clone(), ctx.clone()));
        register(handler);
    });
}

/// Re-register after launch, in case AppKit replaced the handlers while finishing launching.
pub fn reinstall() {
    HANDLER.with(|cell| {
        if let Some(handler) = cell.get() {
            register(handler);
        }
    });
}

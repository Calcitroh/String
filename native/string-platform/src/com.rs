//! COM on the calling thread, started once and left running for the thread's life: a single-threaded apartment, which
//! the shell's dialogs and the taskbar want, and which toolkits' drag and drop (OleInitialize) agrees with. A thread
//! already in COM (either kind) is left as it is.

use std::cell::Cell;
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx,
};

thread_local! {
    static STARTED: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn init() {
    STARTED.with(|started| {
        if !started.get() {
            // S_OK, S_FALSE (already) and RPC_E_CHANGED_MODE (already, the other kind) all leave COM usable here
            let _ =
                unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) };
            started.set(true);
        }
    });
}

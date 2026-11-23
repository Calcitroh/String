//! Windows creates ButtonPressed's read-only arguments and has no public constructor for them.
//! Invoke the registered WinRT delegate with an immutable local argument on the disabled session.
use super::MediaControls;
use std::{ffi::c_void, io, ptr};
use windows::{
    Media::{
        ISystemMediaTransportControlsButtonPressedEventArgs_Vtbl as ArgsVtable,
        SystemMediaTransportControlsButton as Button,
        SystemMediaTransportControlsButtonPressedEventArgs as Args,
    },
    Win32::Foundation::{E_NOINTERFACE, E_POINTER, S_OK},
    core::{GUID, HRESULT, IInspectable, IInspectable_Vtbl, IUnknown, IUnknown_Vtbl, Interface},
};

#[repr(C)]
struct ButtonArgs {
    vtable: &'static ArgsVtable,
    value: Button,
}

// These objects live for the lifetime of the process, including any references taken by Invoke.
static ARGS: [ButtonArgs; 4] = [
    ButtonArgs::new(Button::Play),
    ButtonArgs::new(Button::Pause),
    ButtonArgs::new(Button::Next),
    ButtonArgs::new(Button::Previous),
];

impl ButtonArgs {
    const fn new(value: Button) -> Self {
        Self {
            vtable: &VTABLE,
            value,
        }
    }
}

impl MediaControls {
    #[doc(hidden)]
    pub fn smoke_press(&self, button: Button) -> io::Result<()> {
        if self.smtc.IsEnabled()? {
            return Err(io::Error::other(
                "The self-test needs a disabled media session",
            ));
        }
        let object = ARGS
            .iter()
            .find(|a| a.value == button)
            .ok_or_else(|| io::Error::other("Unsupported test button"))?;
        // ButtonArgs has the ABI and interface identity of the projected argument class.
        let args = unsafe { Args::from_raw(ptr::from_ref(object).cast_mut().cast()) };
        self.button_handler.Invoke(&self.smtc, &args)?;
        Ok(())
    }
}

unsafe extern "system" fn query(
    this: *mut c_void,
    iid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    if iid.is_null() || out.is_null() {
        return E_POINTER;
    }
    unsafe {
        if [Args::IID, IInspectable::IID, IUnknown::IID].contains(&*iid) {
            *out = this;
            S_OK
        } else {
            *out = ptr::null_mut();
            E_NOINTERFACE
        }
    }
}
unsafe extern "system" fn retain(_: *mut c_void) -> u32 {
    1
}
unsafe extern "system" fn iids(_: *mut c_void, count: *mut u32, ids: *mut *mut GUID) -> HRESULT {
    if count.is_null() || ids.is_null() {
        return E_POINTER;
    }
    unsafe {
        *count = 0;
        *ids = ptr::null_mut();
    }
    S_OK
}
unsafe extern "system" fn class_name(_: *mut c_void, out: *mut *mut c_void) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    unsafe {
        *out = ptr::null_mut();
    }
    S_OK
}
unsafe extern "system" fn trust(_: *mut c_void, out: *mut i32) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    unsafe {
        *out = 0;
    }
    S_OK
}
unsafe extern "system" fn button(this: *mut c_void, out: *mut Button) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    unsafe {
        *out = (*this.cast::<ButtonArgs>()).value;
    }
    S_OK
}
static VTABLE: ArgsVtable = ArgsVtable {
    base__: IInspectable_Vtbl {
        base: IUnknown_Vtbl {
            QueryInterface: query,
            AddRef: retain,
            Release: retain,
        },
        GetIids: iids,
        GetRuntimeClassName: class_name,
        GetTrustLevel: trust,
    },
    Button: button,
};

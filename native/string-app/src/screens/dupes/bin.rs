//! Ask the Recycle Bin itself where Restore will put the item identified by the file handle.
use std::{
    ffi::OsString,
    io,
    os::windows::ffi::OsStringExt,
    path::{Path, PathBuf},
};
use windows::{
    Win32::{
        Foundation::PROPERTYKEY,
        System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize},
        UI::Shell::{
            FOLDERID_RecycleBinFolder, IShellItem, IShellItem2, KF_FLAG_DEFAULT,
            PropertiesSystem::PSGetPropertyKeyFromName, SHCreateItemFromRelativeName,
            SHGetKnownFolderItem,
        },
    },
    core::{HSTRING, PCWSTR, w},
};

struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() }
    }
}

pub(super) struct Bin {
    folder: IShellItem,
    directory: PROPERTYKEY,
    name: PROPERTYKEY,
    _apartment: Apartment,
}
impl Bin {
    pub fn open() -> io::Result<Self> {
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
        let apartment = Apartment;
        let key = |name: PCWSTR| -> windows::core::Result<PROPERTYKEY> {
            let mut key = PROPERTYKEY::default();
            unsafe { PSGetPropertyKeyFromName(name, &mut key)? };
            Ok(key)
        };
        Ok(Self {
            folder: unsafe {
                SHGetKnownFolderItem(&FOLDERID_RecycleBinFolder, KF_FLAG_DEFAULT, None)?
            },
            directory: key(w!("System.Recycle.DeletedFrom"))?,
            name: key(w!("System.FileName"))?,
            _apartment: apartment,
        })
    }
    pub fn restore_path(&self, physical: &Path) -> io::Result<PathBuf> {
        let name = HSTRING::from(super::files::dos_path(physical).as_os_str());
        let item: IShellItem2 = unsafe { SHCreateItemFromRelativeName(&self.folder, &name, None)? };
        let property = |key: &PROPERTYKEY| -> io::Result<OsString> {
            let raw = unsafe { item.GetString(key)? };
            if raw.is_null() {
                return Err(io::Error::other("The restore location is unavailable"));
            }
            let text = OsString::from_wide(unsafe { raw.as_wide() });
            unsafe { CoTaskMemFree(Some(raw.0.cast())) };
            Ok(text)
        };
        Ok(PathBuf::from(property(&self.directory)?).join(property(&self.name)?))
    }
}

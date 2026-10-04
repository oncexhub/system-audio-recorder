//! "SAR" shortcuts on the desktop and in the Start menu.

use std::path::PathBuf;

use windows::core::{Interface, GUID, HSTRING};
use windows::Win32::System::Com::*;
use windows::Win32::UI::Shell::*;

const NAME: &str = "SAR.lnk";

fn known_folder(id: &GUID) -> Option<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as _));
        s.map(PathBuf::from)
    }
}

pub fn desktop_exists() -> bool {
    known_folder(&FOLDERID_Desktop).is_some_and(|d| d.join(NAME).exists())
}

/// Creates SAR shortcuts on the desktop and in the Start menu.
pub fn create() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut targets = Vec::new();
    targets.extend(known_folder(&FOLDERID_Desktop));
    targets.extend(known_folder(&FOLDERID_Programs));
    if targets.is_empty() {
        return Err("Could not find the desktop folder.".into());
    }
    for dir in targets {
        unsafe { save_link(&exe, &dir.join(NAME)) }.map_err(|e| format!("Could not create the shortcut: {}", e.message()))?;
    }
    Ok(())
}

unsafe fn save_link(exe: &std::path::Path, at: &std::path::Path) -> windows::core::Result<()> {
    let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
    link.SetPath(&HSTRING::from(exe.as_os_str()))?;
    if let Some(dir) = exe.parent() {
        link.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))?;
    }
    link.SetIconLocation(&HSTRING::from(exe.as_os_str()), 0)?;
    link.SetDescription(&HSTRING::from("System Audio Recorder"))?;
    let file: IPersistFile = link.cast()?;
    file.Save(&HSTRING::from(at.as_os_str()), true)
}

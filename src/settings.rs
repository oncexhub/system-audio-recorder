//! Settings in a tiny key=value file under %APPDATA%\SystemAudioRecorder.

use std::path::PathBuf;

use crate::sink::Format;

pub struct Settings {
    /// Hotkey as stored by the Win32 hotkey control: low byte = virtual key,
    /// high byte = HOTKEYF_* modifiers.
    pub hotkey: u16,
    pub folder: PathBuf,
    pub format: Format,
    /// Bitrate for MP3 and M4A.
    pub bitrate: u32,
    pub close_to_tray: bool,
    /// The first-run "Add SAR to your desktop?" question was answered.
    pub shortcut_asked: bool,
}

const HOTKEYF_CONTROL: u16 = 0x02;
const HOTKEYF_ALT: u16 = 0x04;

impl Settings {
    pub fn kbps(&self) -> u32 {
        self.bitrate
    }

    pub fn load() -> Settings {
        let mut s = Settings {
            hotkey: ((HOTKEYF_CONTROL | HOTKEYF_ALT) << 8) | b'R' as u16,
            folder: default_folder(),
            format: Format::Wav,
            bitrate: 192,
            close_to_tray: true,
            shortcut_asked: false,
        };
        if let Ok(text) = std::fs::read_to_string(file()) {
            for line in text.lines() {
                let Some((k, v)) = line.split_once('=') else { continue };
                let v = v.trim();
                match k.trim() {
                    "hotkey" => s.hotkey = v.parse().unwrap_or(s.hotkey),
                    "folder" if !v.is_empty() => s.folder = PathBuf::from(v),
                    "format" => {
                        s.format = match v {
                            "mp3" => Format::Mp3,
                            "m4a" => Format::M4a,
                            _ => Format::Wav,
                        }
                    }
                    "bitrate" => s.bitrate = v.parse().ok().filter(|b| crate::ui::BITRATES.contains(b)).unwrap_or(s.bitrate),
                    "close_to_tray" => s.close_to_tray = v != "0",
                    "shortcut_asked" => s.shortcut_asked = v == "1",
                    _ => {}
                }
            }
        }
        s
    }

    pub fn save(&self) {
        let path = file();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let text = format!(
            "hotkey={}\nfolder={}\nformat={}\nbitrate={}\nclose_to_tray={}\nshortcut_asked={}\n",
            self.hotkey,
            self.folder.display(),
            self.format.ext(),
            self.bitrate,
            self.close_to_tray as u8,
            self.shortcut_asked as u8
        );
        let _ = std::fs::write(path, text);
    }
}

fn file() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    base.join("SystemAudioRecorder").join("settings.ini")
}

/// The user's real Music folder (follows OneDrive or other redirection), plus "Recordings".
fn default_folder() -> PathBuf {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_Music, SHGetKnownFolderPath, KF_FLAG_DEFAULT};
    let music = unsafe {
        SHGetKnownFolderPath(&FOLDERID_Music, KF_FLAG_DEFAULT, None).ok().and_then(|p| {
            let s = p.to_string().ok();
            CoTaskMemFree(Some(p.0 as _));
            s
        })
    };
    let music = music.map(PathBuf::from).unwrap_or_else(|| {
        let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        home.join("Music")
    });
    music.join("Recordings")
}

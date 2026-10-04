#![windows_subsystem = "windows"]

mod capture;
mod editor;
mod mp4;
mod settings;
mod shortcut;
mod sink;
mod ui;

use std::collections::VecDeque;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::*;
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use capture::{Recorder, WM_RECORDER_FAILED};
use settings::Settings;
use sink::Format;
use editor::{Editor, Handle};
use ui::{EditorView, FooterKind, Gfx, Hit, View};

const APP_NAME: PCWSTR = w!("System Audio Recorder");
const CLASS_NAME: PCWSTR = w!("SystemAudioRecorderWnd");
const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");

const WM_TRAY: u32 = WM_APP + 1;
const WM_SHOW_EXISTING: u32 = WM_APP + 2;
const WM_OPEN_PENDING: u32 = WM_APP + 3;

thread_local! {
    static PENDING_OPEN: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}
const WM_MOUSELEAVE: u32 = 0x02A3;
const HOTKEY_ID: i32 = 1;
const TIMER_ID: usize = 1;
const TIMER_EDIT: usize = 2;

// Tray menu ids
const IDM_TOGGLE: usize = 1;
const IDM_OPEN: usize = 2;
const IDM_FOLDER: usize = 3;
const IDM_EXIT: usize = 4;

// Hotkey modifier flags (same layout as the Win32 hotkey control)
const HOTKEYF_SHIFT: u8 = 0x01;
const HOTKEYF_CONTROL: u8 = 0x02;
const HOTKEYF_ALT: u8 = 0x04;

struct App {
    hwnd: HWND,
    gfx: Option<Gfx>,
    settings: Settings,
    startup: bool,
    recorder: Option<Recorder>,
    waves: VecDeque<f32>,
    capturing_hotkey: bool,
    hotkey_ok: bool,
    shown_secs: u64,
    footer: String,
    footer_kind: FooterKind,
    last_saved: Option<PathBuf>,
    hover: Option<Hit>,
    pressed: Option<Hit>,
    editor: Option<Editor>,
    dragging: Option<Handle>,
    shortcut_prompt: bool,
    icon_idle: HICON,
    icon_rec: HICON,
    taskbar_created: u32,
}

static mut APP: *mut App = std::ptr::null_mut();

#[allow(static_mut_refs)]
fn app() -> &'static mut App {
    unsafe { &mut *APP }
}

fn main() {
    unsafe {
        let args: Vec<String> = std::env::args().collect();
        // Only one instance: bring the existing window forward instead.
        let _mutex = CreateMutexW(None, true, w!("Local\\SystemAudioRecorder.Instance"));
        // Read the error right away: any later API call overwrites it.
        let already_running = GetLastError() == ERROR_ALREADY_EXISTS;
        let open_file = args.get(1).map(PathBuf::from).filter(|p| editor::Kind::of(p).is_some() && p.exists());
        if already_running {
            if let Ok(h) = FindWindowW(CLASS_NAME, PCWSTR::null()) {
                match &open_file {
                    // Hand the file to the running instance.
                    Some(p) => {
                        let wide: Vec<u16> = p.as_os_str().encode_wide().collect();
                        let cds = COPYDATASTRUCT { dwData: 1, cbData: (wide.len() * 2) as u32, lpData: wide.as_ptr() as _ };
                        SendMessageW(h, WM_COPYDATA, Some(WPARAM(0)), Some(LPARAM(&cds as *const _ as isize)));
                    }
                    None => {
                        let _ = PostMessageW(Some(h), WM_SHOW_EXISTING, WPARAM(0), LPARAM(0));
                    }
                }
            }
            return;
        }

        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let hinst = GetModuleHandleW(None).unwrap();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            hIcon: LoadIconW(Some(hinst.into()), PCWSTR(1 as _)).unwrap_or_default(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: CreateSolidBrush(COLORREF(0x000D0B0B)),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        RegisterClassExW(&wc);

        APP = Box::into_raw(Box::new(App {
            hwnd: HWND::default(),
            gfx: Gfx::new().ok(),
            settings: Settings::load(),
            startup: startup_enabled(),
            recorder: None,
            waves: VecDeque::with_capacity(ui::WAVE_BARS),
            capturing_hotkey: false,
            hotkey_ok: false,
            shown_secs: u64::MAX,
            footer: String::new(),
            footer_kind: FooterKind::None,
            last_saved: None,
            hover: None,
            pressed: None,
            editor: None,
            dragging: None,
            shortcut_prompt: false,
            // Idle tray icon = the app logo; recording = red dot.
            icon_idle: LoadImageW(Some(hinst.into()), PCWSTR(1 as _), IMAGE_ICON, GetSystemMetrics(SM_CXSMICON), GetSystemMetrics(SM_CYSMICON), LR_DEFAULTCOLOR)
                .map(|h| HICON(h.0))
                .unwrap_or_else(|_| make_dot_icon(false)),
            icon_rec: make_dot_icon(true),
            taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
        }));

        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            CLASS_NAME,
            APP_NAME,
            style,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            480,
            640,
            None,
            None,
            Some(hinst.into()),
            None,
        )
        .unwrap();

        if !std::env::args().any(|a| a == "--tray") {
            let _ = ShowWindow(hwnd, SW_SHOW);
        }
        if let Some(p) = open_file {
            open_editor(p);
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

// ------------------------------------------------------------- window ----

fn dpi() -> u32 {
    unsafe { GetDpiForWindow(app().hwnd).max(96) }
}

fn scale() -> f32 {
    dpi() as f32 / 96.0
}

unsafe fn size_window(hwnd: HWND, dpi: u32) {
    let s = dpi as f32 / 96.0;
    let mut r = RECT { left: 0, top: 0, right: (ui::WIDTH * s).round() as i32, bottom: (ui::HEIGHT * s).round() as i32 };
    let style = WINDOW_STYLE(GetWindowLongW(hwnd, GWL_STYLE) as u32);
    let _ = AdjustWindowRectExForDpi(&mut r, style, false, WINDOW_EX_STYLE::default(), dpi);
    let _ = SetWindowPos(hwnd, None, 0, 0, r.right - r.left, r.bottom - r.top, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
}

/// Dark title bar and border so the window matches the UI (Windows 11).
unsafe fn dark_frame(hwnd: HWND) {
    use windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE;
    let set = |attr: i32, v: u32| {
        let _ = DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(attr), &v as *const u32 as _, 4);
    };
    set(20, 1); // immersive dark mode
    set(35, 0x000D0B0B); // caption color (COLORREF = 0x00BBGGRR)
    set(34, 0x00282222); // border color
    set(36, 0x00EFEDED); // caption text color
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            app().hwnd = hwnd;
            dark_frame(hwnd);
            size_window(hwnd, GetDpiForWindow(hwnd));
            add_tray_icon();
            register_hotkey(true);
            DragAcceptFiles(hwnd, true);
            // Ask once about the SAR shortcut (skip if one is already there).
            let a = app();
            if !a.settings.shortcut_asked {
                if shortcut::desktop_exists() {
                    a.settings.shortcut_asked = true;
                    a.settings.save();
                } else {
                    a.shortcut_prompt = true;
                }
            }
            LRESULT(0)
        }
        WM_DPICHANGED => {
            let r = &*(lp.0 as *const RECT);
            let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            size_window(hwnd, (wp.0 & 0xffff) as u32);
            LRESULT(0)
        }
        WM_SIZE => {
            if let Some(g) = app().gfx.as_mut() {
                g.resize((lp.0 & 0xffff) as u32, ((lp.0 >> 16) & 0xffff) as u32, dpi() as f32);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            BeginPaint(hwnd, &mut ps);
            paint();
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let a = app();
            if let (Some(h), Some(ed)) = (a.dragging, a.editor.as_mut()) {
                let (x, _) = dip(lp);
                ed.set_handle(h, ui::wave_time(x, ed.duration()));
                ed.playhead = if h == Handle::Start { ed.start } else { ed.end };
                redraw();
                return LRESULT(0);
            }
            let h = hit_at(lp);
            if h != a.hover {
                a.hover = h;
                redraw();
            }
            let mut tme = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            let _ = TrackMouseEvent(&mut tme);
            LRESULT(0)
        }
        WM_MOUSELEAVE => {
            if app().hover.take().is_some() {
                redraw();
            }
            LRESULT(0)
        }
        WM_SETCURSOR if (lp.0 & 0xffff) as u32 == HTCLIENT => {
            let cursor = if app().hover.is_some() { IDC_HAND } else { IDC_ARROW };
            SetCursor(LoadCursorW(None, cursor).ok());
            LRESULT(1)
        }
        WM_LBUTTONDOWN => {
            let a = app();
            a.pressed = hit_at(lp);
            if a.capturing_hotkey && a.pressed != Some(Hit::Hotkey) {
                cancel_hotkey_capture();
            }
            if a.pressed == Some(Hit::Wave) {
                wave_press(lp);
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let a = app();
            if a.dragging.take().is_some() {
                let _ = ReleaseCapture();
                a.pressed = None;
                redraw();
                return LRESULT(0);
            }
            let h = hit_at(lp);
            if h.is_some() && h == a.pressed.take() {
                on_click(h.unwrap());
            }
            LRESULT(0)
        }
        WM_KEYDOWN | WM_SYSKEYDOWN if app().capturing_hotkey => {
            on_capture_key(wp.0 as u32);
            LRESULT(0)
        }
        WM_CHAR | WM_SYSCHAR if app().capturing_hotkey => LRESULT(0),
        WM_KEYDOWN if app().editor.is_some() => {
            editor_key(wp.0 as u32);
            LRESULT(0)
        }
        WM_DROPFILES => {
            let drop = HDROP(wp.0 as _);
            let mut buf = [0u16; 1024];
            let n = DragQueryFileW(drop, 0, Some(&mut buf));
            DragFinish(drop);
            if n > 0 {
                open_editor(PathBuf::from(String::from_utf16_lossy(&buf[..n as usize])));
            }
            LRESULT(0)
        }
        WM_KILLFOCUS => {
            if app().capturing_hotkey {
                cancel_hotkey_capture();
            }
            LRESULT(0)
        }
        WM_HOTKEY if wp.0 as i32 == HOTKEY_ID => {
            toggle_recording();
            LRESULT(0)
        }
        WM_TIMER => {
            if wp.0 == TIMER_EDIT {
                editor_tick();
            } else {
                tick();
            }
            LRESULT(0)
        }
        WM_TRAY => {
            match (lp.0 & 0xffff) as u32 {
                WM_LBUTTONUP | WM_LBUTTONDBLCLK => show_window(),
                WM_RBUTTONUP | WM_CONTEXTMENU => tray_menu(),
                _ => {}
            }
            LRESULT(0)
        }
        WM_SHOW_EXISTING => {
            show_window();
            LRESULT(0)
        }
        WM_COPYDATA => {
            let cds = &*(lp.0 as *const COPYDATASTRUCT);
            if cds.dwData == 1 && !cds.lpData.is_null() {
                let wide = std::slice::from_raw_parts(cds.lpData as *const u16, cds.cbData as usize / 2);
                let path = PathBuf::from(String::from_utf16_lossy(wide));
                // Open after returning so the sending process isn't kept waiting.
                PENDING_OPEN.with(|p| *p.borrow_mut() = Some(path));
                let _ = PostMessageW(Some(hwnd), WM_OPEN_PENDING, WPARAM(0), LPARAM(0));
            }
            LRESULT(1)
        }
        WM_OPEN_PENDING => {
            if let Some(p) = PENDING_OPEN.with(|p| p.borrow_mut().take()) {
                open_editor(p);
            }
            LRESULT(0)
        }
        WM_RECORDER_FAILED => {
            stop_recording();
            LRESULT(0)
        }
        WM_CLOSE => {
            if app().settings.close_to_tray {
                hide_window();
            } else {
                exit_app();
            }
            LRESULT(0)
        }
        WM_QUERYENDSESSION => LRESULT(1),
        WM_ENDSESSION => {
            if wp.0 != 0 {
                if let Some(r) = app().recorder.take() {
                    let _ = r.stop();
                }
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            remove_tray_icon();
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ if msg == app().taskbar_created && msg != 0 => {
            add_tray_icon();
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn view_data(a: &App) -> (String, String, Vec<f32>) {
    let hk = hotkey_text(a.settings.hotkey);
    let folder = a.settings.folder.display().to_string();
    // Newest level on the right; pad on the left with silence.
    let mut waves = vec![0.0; ui::WAVE_BARS - a.waves.len().min(ui::WAVE_BARS)];
    waves.extend(a.waves.iter().copied());
    (hk, folder, waves)
}

fn make_view<'a>(a: &App, hk: &'a str, folder: &'a str, waves: &'a [f32], footer: &'a str) -> View<'a> {
    View {
        shortcut_prompt: a.shortcut_prompt && a.editor.is_none(),
        can_trim: a.last_saved.as_ref().is_some_and(|p| p.exists() && editor::Kind::of(p).is_some()),
        editor: None,
        recording: a.recorder.is_some(),
        secs: a.recorder.as_ref().map_or(0, |r| r.seconds()),
        hotkey: hk,
        hotkey_ok: a.hotkey_ok,
        capturing_hotkey: a.capturing_hotkey,
        folder,
        format: match a.settings.format {
            Format::Wav => 0,
            Format::Mp3 => 1,
            Format::M4a => 2,
        },
        kbps: a.settings.bitrate,
        tray: a.settings.close_to_tray,
        startup: a.startup,
        footer,
        footer_kind: a.footer_kind,
        hover: a.hover,
        waves,
    }
}

struct EditorData {
    name: String,
    columns: Vec<f32>,
}

fn editor_data(a: &App) -> Option<EditorData> {
    let ed = a.editor.as_ref()?;
    Some(EditorData {
        name: ed.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        columns: ed.columns(ui::EDIT_COLS),
    })
}

fn editor_view<'a>(a: &App, d: &'a Option<EditorData>) -> Option<EditorView<'a>> {
    let (ed, d) = (a.editor.as_ref()?, d.as_ref()?);
    Some(EditorView {
        name: &d.name,
        columns: &d.columns,
        duration: ed.duration(),
        start: ed.start,
        end: ed.end,
        playhead: ed.playhead,
        playing: ed.is_playing(),
        loading: ed.loading(),
        active_start: ed.active == Handle::Start,
    })
}

unsafe fn paint() {
    let a = app();
    let (hk, folder, waves) = view_data(a);
    let footer = a.footer.clone();
    let ed = editor_data(a);
    let mut rc = RECT::default();
    let _ = GetClientRect(a.hwnd, &mut rc);
    let mut v = make_view(a, &hk, &folder, &waves, &footer);
    v.editor = editor_view(a, &ed);
    let hwnd = a.hwnd;
    let d = dpi() as f32;
    if let Some(g) = a.gfx.as_mut() {
        g.draw(hwnd, rc.right as u32, rc.bottom as u32, d, &v);
    }
}

/// Mouse position in 96-DPI layout units.
fn dip(lp: LPARAM) -> (f32, f32) {
    let s = scale();
    ((lp.0 & 0xffff) as i16 as f32 / s, ((lp.0 >> 16) & 0xffff) as i16 as f32 / s)
}

fn hit_at(lp: LPARAM) -> Option<Hit> {
    let a = app();
    let (x, y) = dip(lp);
    if let Some(ed) = &a.editor {
        let ev = EditorView {
            name: "",
            columns: &[],
            duration: ed.duration(),
            start: ed.start,
            end: ed.end,
            playhead: ed.playhead,
            playing: ed.is_playing(),
            loading: ed.loading(),
            active_start: ed.active == Handle::Start,
        };
        return ui::hit_test_editor(x, y, &ev);
    }
    if a.shortcut_prompt {
        return ui::hit_test_prompt(x, y);
    }
    let (hk, folder, waves) = view_data(a);
    let v = make_view(a, &hk, &folder, &waves, "");
    let mut v = v;
    v.footer_kind = a.footer_kind;
    ui::hit_test(x, y, &v)
}

fn redraw() {
    unsafe {
        let _ = InvalidateRect(Some(app().hwnd), None, false);
    }
}

unsafe fn on_click(h: Hit) {
    let a = app();
    match h {
        Hit::Record => toggle_recording(),
        Hit::Hotkey => {
            if a.capturing_hotkey {
                cancel_hotkey_capture();
            } else {
                a.capturing_hotkey = true;
                register_hotkey(false);
                let _ = SetFocus(Some(a.hwnd));
                clear_footer();
            }
        }
        Hit::FolderChange => {
            if let Some(p) = pick_folder() {
                a.settings.folder = p;
                a.settings.save();
            }
        }
        Hit::FolderOpen => open_folder(),
        Hit::Bitrate(i) => {
            a.settings.bitrate = ui::BITRATES[i as usize];
            a.settings.save();
        }
        Hit::FormatWav | Hit::FormatMp3 | Hit::FormatM4a => {
            a.settings.format = match h {
                Hit::FormatMp3 => Format::Mp3,
                Hit::FormatM4a => Format::M4a,
                _ => Format::Wav,
            };
            a.settings.save();
        }
        Hit::Tray => {
            a.settings.close_to_tray = !a.settings.close_to_tray;
            a.settings.save();
        }
        Hit::Startup => {
            set_startup(!a.startup);
            a.startup = startup_enabled();
        }
        Hit::Footer => open_folder(),
        Hit::FooterTrim => {
            if let Some(p) = a.last_saved.clone() {
                open_editor(p);
            }
        }
        Hit::OpenEditor => {
            if let Some(p) = pick_audio_file() {
                open_editor(p);
            }
        }
        Hit::Back => close_editor(),
        Hit::Wave => {}
        Hit::StartMinus | Hit::StartPlus | Hit::EndMinus | Hit::EndPlus => {
            if let Some(ed) = a.editor.as_mut() {
                let big = GetKeyState(VK_SHIFT.0 as i32) < 0;
                let step = if big { 1.0 } else { 0.1 };
                let (h, d) = match h {
                    Hit::StartMinus => (Handle::Start, -step),
                    Hit::StartPlus => (Handle::Start, step),
                    Hit::EndMinus => (Handle::End, -step),
                    _ => (Handle::End, step),
                };
                ed.nudge(h, d);
                ed.playhead = if h == Handle::Start { ed.start } else { ed.end };
            }
        }
        Hit::PlayStart => editor_play(|ed| (ed.start, ed.end)),
        Hit::PlayEnd => editor_play(|ed| ((ed.end - 3.0).max(ed.start), ed.end)),
        Hit::PlayPause => {
            if let Some(ed) = a.editor.as_mut() {
                if let Err(e) = ed.toggle_play() {
                    set_footer(&e, FooterKind::Error);
                }
            }
        }
        Hit::SaveCopy => save_trim(false),
        Hit::Replace => save_trim(true),
        Hit::ShortcutYes | Hit::ShortcutNo => {
            a.shortcut_prompt = false;
            a.hover = None;
            a.settings.shortcut_asked = true;
            a.settings.save();
            if h == Hit::ShortcutYes {
                match shortcut::create() {
                    Ok(()) => set_footer("SAR shortcut added to your desktop and Start menu", FooterKind::Info),
                    Err(e) => set_footer(&e, FooterKind::Error),
                }
            }
        }
    }
    redraw();
}

// ------------------------------------------------------------ editor ----

unsafe fn open_editor(path: PathBuf) {
    let a = app();
    if a.recorder.as_ref().is_some_and(|r| r.path == path) {
        set_footer("This recording is still in progress. Stop it first, then trim.", FooterKind::Error);
        return;
    }
    close_editor();
    match Editor::open(path) {
        Ok(ed) => {
            a.editor = Some(ed);
            a.hover = None;
            clear_footer();
            SetTimer(Some(a.hwnd), TIMER_EDIT, 33, None);
            show_window();
        }
        Err(e) => {
            show_window();
            set_footer(&e, FooterKind::Error);
        }
    }
    redraw();
}

unsafe fn close_editor() {
    let a = app();
    if a.editor.take().is_some() {
        let _ = KillTimer(Some(a.hwnd), TIMER_EDIT);
        a.dragging = None;
        a.hover = None;
        if a.footer_kind != FooterKind::Success {
            clear_footer();
        }
    }
    redraw();
}

unsafe fn editor_tick() {
    let a = app();
    if let Some(ed) = a.editor.as_mut() {
        ed.update();
        if let Some(e) = ed.load_error() {
            set_footer(&e, FooterKind::Error);
        }
    }
    if IsWindowVisible(a.hwnd).as_bool() {
        redraw();
    }
}

unsafe fn editor_play(range: impl Fn(&Editor) -> (f64, f64)) {
    if let Some(ed) = app().editor.as_mut() {
        let (from, to) = range(ed);
        if let Err(e) = ed.play(from, to) {
            set_footer(&e, FooterKind::Error);
        }
    }
}

/// Press inside the waveform: grab a handle if close to one, else move the playhead.
unsafe fn wave_press(lp: LPARAM) {
    let a = app();
    let Some(ed) = a.editor.as_mut() else { return };
    let (x, _) = dip(lp);
    let dur = ed.duration();
    let ds = (x - ui::wave_x(ed.start, dur)).abs();
    let de = (x - ui::wave_x(ed.end, dur)).abs();
    let grab = if ds <= 10.0 && ds <= de {
        Some(Handle::Start)
    } else if de <= 10.0 {
        Some(Handle::End)
    } else {
        None
    };
    if let Some(h) = grab {
        ed.stop();
        ed.active = h;
        a.dragging = Some(h);
        SetCapture(a.hwnd);
    } else {
        let t = ui::wave_time(x, dur);
        let was_playing = ed.is_playing();
        ed.stop();
        ed.playhead = t;
        if was_playing {
            let _ = ed.play(t, ed.end.max(t));
        }
    }
    redraw();
}

unsafe fn editor_key(vk: u32) {
    let a = app();
    let Some(ed) = a.editor.as_mut() else { return };
    let step = if GetKeyState(VK_SHIFT.0 as i32) < 0 { 1.0 } else { 0.1 };
    match VIRTUAL_KEY(vk as u16) {
        VK_SPACE => {
            let _ = ed.toggle_play();
        }
        VK_LEFT => {
            let h = ed.active;
            ed.nudge(h, -step);
            ed.playhead = if h == Handle::Start { ed.start } else { ed.end };
        }
        VK_RIGHT => {
            let h = ed.active;
            ed.nudge(h, step);
            ed.playhead = if h == Handle::Start { ed.start } else { ed.end };
        }
        VK_TAB => ed.active = if ed.active == Handle::Start { Handle::End } else { Handle::Start },
        VK_ESCAPE => close_editor(),
        _ => {}
    }
    redraw();
}

fn trimmed_copy_path(src: &Path) -> PathBuf {
    let stem = src.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = src.extension().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let dir = src.parent().unwrap_or(Path::new("."));
    let mut p = dir.join(format!("{stem} (trimmed).{ext}"));
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{stem} (trimmed {n}).{ext}"));
        n += 1;
    }
    p
}

unsafe fn save_trim(replace: bool) {
    let a = app();
    let Some(ed) = a.editor.as_mut() else { return };
    ed.stop();
    let src = ed.path.clone();
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();

    if !replace {
        let dest = trimmed_copy_path(&src);
        let _ = SetCursor(LoadCursorW(None, IDC_WAIT).ok());
        match ed.save(&dest) {
            Ok(()) => {
                set_footer(&format!("\u{2713}  Saved {}", name(&dest)), FooterKind::Success);
                a.last_saved = Some(dest);
            }
            Err(e) => {
                let _ = std::fs::remove_file(&dest);
                set_footer(&e, FooterKind::Error);
            }
        }
        return;
    }

    let msg = HSTRING::from(format!("Replace \"{}\" with the trimmed version?\n\nThe cut parts will be gone for good.", name(&src)));
    if MessageBoxW(Some(a.hwnd), &msg, APP_NAME, MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2) != IDYES {
        return;
    }
    let tmp = src.with_extension(format!("{}.trimming", src.extension().and_then(|e| e.to_str()).unwrap_or("tmp")));
    let _ = SetCursor(LoadCursorW(None, IDC_WAIT).ok());
    let result = ed.save(&tmp);
    // Close the file (playback and waveform reader) before swapping it.
    a.editor = None;
    let result = result.and_then(|_| std::fs::rename(&tmp, &src).map_err(|e| format!("Could not replace the file: {e}")));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    let _ = KillTimer(Some(a.hwnd), TIMER_EDIT);
    open_editor(src.clone());
    match result {
        Ok(()) => {
            set_footer(&format!("\u{2713}  Trimmed {}", name(&src)), FooterKind::Success);
            a.last_saved = Some(src);
        }
        Err(e) => set_footer(&e, FooterKind::Error),
    }
}

unsafe fn pick_audio_file() -> Option<PathBuf> {
    let dlg: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
    let filters = [
        Common::COMDLG_FILTERSPEC { pszName: w!("Recordings (*.wav, *.mp3, *.m4a)"), pszSpec: w!("*.wav;*.mp3;*.m4a") },
    ];
    let _ = dlg.SetFileTypes(&filters);
    let _ = dlg.SetTitle(w!("Choose a recording to trim"));
    let cur = &app().settings.folder;
    if cur.exists() {
        if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(cur.as_os_str()), None) {
            let _ = dlg.SetFolder(&item);
        }
    }
    dlg.Show(Some(app().hwnd)).ok()?;
    let item = dlg.GetResult().ok()?;
    let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
    let s = p.to_string().ok();
    CoTaskMemFree(Some(p.0 as _));
    s.map(PathBuf::from)
}

fn set_footer(text: &str, kind: FooterKind) {
    let a = app();
    a.footer = text.to_string();
    a.footer_kind = kind;
    redraw();
}

fn clear_footer() {
    set_footer("", FooterKind::None);
}

// ------------------------------------------------------------ hotkey ----

fn hotkey_text(hk: u16) -> String {
    let vk = (hk & 0xff) as u32;
    let mods = (hk >> 8) as u8;
    let mut parts = Vec::new();
    if mods & HOTKEYF_CONTROL != 0 {
        parts.push("Ctrl".to_string());
    }
    if mods & HOTKEYF_ALT != 0 {
        parts.push("Alt".to_string());
    }
    if mods & HOTKEYF_SHIFT != 0 {
        parts.push("Shift".to_string());
    }
    parts.push(key_name(vk));
    parts.join(" + ")
}

fn key_name(vk: u32) -> String {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => (vk as u8 as char).to_string(),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        _ => unsafe {
            let mut sc = MapVirtualKeyW(vk, MAPVK_VK_TO_VSC);
            // Keys that live on the extended part of the keyboard.
            if matches!(vk, 0x21..=0x2E | 0x6F | 0x90) {
                sc |= 0x100;
            }
            let mut buf = [0u16; 64];
            let n = GetKeyNameTextW((sc << 16) as i32, &mut buf);
            if n > 0 {
                String::from_utf16_lossy(&buf[..n as usize])
            } else {
                format!("Key {vk}")
            }
        },
    }
}

fn hotkey_mods(hk: u16) -> HOT_KEY_MODIFIERS {
    let m = (hk >> 8) as u8;
    let mut r = MOD_NOREPEAT;
    if m & HOTKEYF_CONTROL != 0 {
        r |= MOD_CONTROL;
    }
    if m & HOTKEYF_ALT != 0 {
        r |= MOD_ALT;
    }
    if m & HOTKEYF_SHIFT != 0 {
        r |= MOD_SHIFT;
    }
    r
}

/// Registers (or unregisters) the global hotkey.
unsafe fn register_hotkey(on: bool) {
    let a = app();
    let _ = UnregisterHotKey(Some(a.hwnd), HOTKEY_ID);
    a.hotkey_ok = false;
    if !on || a.settings.hotkey == 0 {
        return;
    }
    let hk = a.settings.hotkey;
    a.hotkey_ok = RegisterHotKey(Some(a.hwnd), HOTKEY_ID, hotkey_mods(hk), (hk & 0xff) as u32).is_ok();
    if !a.hotkey_ok {
        set_footer(&format!("{} is already used by another program. Pick another hotkey.", hotkey_text(hk)), FooterKind::Error);
    }
}

unsafe fn cancel_hotkey_capture() {
    app().capturing_hotkey = false;
    register_hotkey(true);
    redraw();
}

/// A key was pressed while the hotkey chip is waiting for a new combination.
unsafe fn on_capture_key(vk: u32) {
    let a = app();
    if vk == VK_ESCAPE.0 as u32 {
        cancel_hotkey_capture();
        return;
    }
    // Ignore lone modifiers; wait for the actual key.
    if matches!(vk, 0x10..=0x12 | 0xA0..=0xA5 | 0x5B | 0x5C | 0x14) {
        return;
    }
    let down = |k: VIRTUAL_KEY| GetKeyState(k.0 as i32) < 0;
    let mut mods = 0u8;
    if down(VK_CONTROL) {
        mods |= HOTKEYF_CONTROL;
    }
    if down(VK_MENU) {
        mods |= HOTKEYF_ALT;
    }
    if down(VK_SHIFT) {
        mods |= HOTKEYF_SHIFT;
    }
    let new = ((mods as u16) << 8) | (vk as u16 & 0xff);
    let is_fkey = (0x70..=0x87).contains(&vk) || matches!(vk, 0x13 | 0x91); // F1-F24, Pause, Scroll Lock

    if !is_fkey && mods & (HOTKEYF_CONTROL | HOTKEYF_ALT) == 0 {
        set_footer("Include Ctrl or Alt (or use a function key like F9) so normal typing isn't blocked.", FooterKind::Error);
        return;
    }
    let ok = RegisterHotKey(Some(a.hwnd), HOTKEY_ID + 1, hotkey_mods(new), vk).is_ok();
    let _ = UnregisterHotKey(Some(a.hwnd), HOTKEY_ID + 1);
    if !ok {
        set_footer(&format!("{} is already used by Windows or another program. Try another.", hotkey_text(new)), FooterKind::Error);
        return;
    }

    a.settings.hotkey = new;
    a.settings.save();
    a.capturing_hotkey = false;
    register_hotkey(true);
    set_footer(&format!("Hotkey set to {}", hotkey_text(new)), FooterKind::Info);
    update_tray();
}

// --------------------------------------------------------- recording ----

unsafe fn toggle_recording() {
    if app().recorder.is_some() {
        stop_recording();
    } else {
        start_recording();
    }
}

unsafe fn start_recording() {
    let a = app();
    let folder = a.settings.folder.clone();
    if let Err(e) = std::fs::create_dir_all(&folder) {
        set_footer(&format!("Cannot use the save folder: {e}"), FooterKind::Error);
        show_window();
        return;
    }
    let path = new_file_path(&folder, a.settings.format.ext());
    match Recorder::start(path, a.settings.format, a.settings.kbps(), a.hwnd) {
        Ok(r) => {
            a.recorder = Some(r);
            a.waves.clear();
            a.shown_secs = u64::MAX;
            clear_footer();
            SetTimer(Some(a.hwnd), TIMER_ID, 33, None);
        }
        Err(e) => {
            set_footer("Recording could not start.", FooterKind::Error);
            show_window();
            MessageBoxW(Some(a.hwnd), &HSTRING::from(e), APP_NAME, MB_ICONERROR);
        }
    }
    update_tray();
    redraw();
}

unsafe fn stop_recording() {
    let a = app();
    let Some(r) = a.recorder.take() else { return };
    let _ = KillTimer(Some(a.hwnd), TIMER_ID);
    let secs = r.seconds();
    match r.stop() {
        Ok(path) => {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            set_footer(&format!("\u{2713}  Saved {}  \u{00B7}  {}", name, fmt_time(secs)), FooterKind::Success);
            if !IsWindowVisible(a.hwnd).as_bool() {
                notify("Recording saved", &format!("{name}\n{}", fmt_time(secs)));
            }
            a.last_saved = Some(path);
        }
        Err(e) => {
            set_footer(&format!("Recording stopped: {e}"), FooterKind::Error);
            show_window();
        }
    }
    a.waves.clear();
    update_tray();
    redraw();
}

/// Animation tick while recording (~30 fps).
unsafe fn tick() {
    let a = app();
    let Some(r) = &a.recorder else { return };
    for l in r.take_levels() {
        if a.waves.len() == ui::WAVE_BARS {
            a.waves.pop_front();
        }
        a.waves.push_back(l);
    }
    let secs = r.seconds();
    if secs != a.shown_secs {
        a.shown_secs = secs;
        update_tray();
    }
    if IsWindowVisible(a.hwnd).as_bool() && !IsIconic(a.hwnd).as_bool() {
        redraw();
    }
}

fn new_file_path(folder: &Path, ext: &str) -> PathBuf {
    let t = unsafe { GetLocalTime() };
    let base = format!(
        "Recording {:04}-{:02}-{:02} {:02}-{:02}-{:02}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    );
    let mut p = folder.join(format!("{base}.{ext}"));
    let mut n = 2;
    while p.exists() {
        p = folder.join(format!("{base} ({n}).{ext}"));
        n += 1;
    }
    p
}

fn fmt_time(secs: u64) -> String {
    format!("{:02}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
}

// ------------------------------------------------------------ window ----

unsafe fn show_window() {
    let h = app().hwnd;
    let _ = ShowWindow(h, SW_SHOW);
    let _ = ShowWindow(h, SW_RESTORE);
    let _ = SetForegroundWindow(h);
    redraw();
}

unsafe fn hide_window() {
    let a = app();
    if a.capturing_hotkey {
        cancel_hotkey_capture();
    }
    if a.footer_kind != FooterKind::Success {
        clear_footer();
    }
    if let Some(ed) = a.editor.as_mut() {
        ed.stop();
    }
    let _ = ShowWindow(a.hwnd, SW_HIDE);
}

unsafe fn exit_app() {
    let a = app();
    a.editor = None;
    if let Some(r) = a.recorder.take() {
        let _ = r.stop();
    }
    let _ = DestroyWindow(a.hwnd);
}

unsafe fn open_folder() {
    let a = app();
    let folder = a.settings.folder.clone();
    let _ = std::fs::create_dir_all(&folder);
    // Select the last recording if there is one, otherwise just open the folder.
    if let Some(p) = a.last_saved.as_ref().filter(|p| p.exists() && p.parent() == Some(folder.as_path())) {
        let args = HSTRING::from(format!("/select,\"{}\"", p.display()));
        ShellExecuteW(None, w!("open"), w!("explorer.exe"), &args, None, SW_SHOWNORMAL);
    } else {
        ShellExecuteW(None, w!("open"), &HSTRING::from(folder.as_os_str()), None, None, SW_SHOWNORMAL);
    }
}

unsafe fn pick_folder() -> Option<PathBuf> {
    let dlg: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
    let opts = dlg.GetOptions().ok()?;
    dlg.SetOptions(opts | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM).ok()?;
    let _ = dlg.SetTitle(w!("Choose where recordings are saved"));
    let cur = &app().settings.folder;
    if cur.exists() {
        if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(cur.as_os_str()), None) {
            let _ = dlg.SetFolder(&item);
        }
    }
    dlg.Show(Some(app().hwnd)).ok()?;
    let item = dlg.GetResult().ok()?;
    let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
    let s = p.to_string().ok();
    CoTaskMemFree(Some(p.0 as _));
    s.map(PathBuf::from)
}

// -------------------------------------------------------------- tray ----

unsafe fn tray_data() -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: app().hwnd,
        uID: 1,
        ..Default::default()
    }
}

fn copy_wstr(dst: &mut [u16], s: &str) {
    let mut i = 0;
    for c in s.encode_utf16() {
        if i + 1 >= dst.len() {
            break;
        }
        dst[i] = c;
        i += 1;
    }
    dst[i] = 0;
}

unsafe fn add_tray_icon() {
    let mut d = tray_data();
    d.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    d.uCallbackMessage = WM_TRAY;
    d.hIcon = app().icon_idle;
    copy_wstr(&mut d.szTip, "System Audio Recorder");
    let _ = Shell_NotifyIconW(NIM_ADD, &d);
    update_tray();
}

unsafe fn update_tray() {
    let a = app();
    let mut d = tray_data();
    d.uFlags = NIF_ICON | NIF_TIP;
    let tip = match &a.recorder {
        Some(r) => {
            d.hIcon = a.icon_rec;
            format!("System Audio Recorder - Recording {}", fmt_time(r.seconds()))
        }
        None => {
            d.hIcon = a.icon_idle;
            format!("System Audio Recorder - Ready ({})", hotkey_text(a.settings.hotkey))
        }
    };
    copy_wstr(&mut d.szTip, &tip);
    let _ = Shell_NotifyIconW(NIM_MODIFY, &d);
}

unsafe fn remove_tray_icon() {
    let d = tray_data();
    let _ = Shell_NotifyIconW(NIM_DELETE, &d);
}

unsafe fn notify(title: &str, body: &str) {
    let mut d = tray_data();
    d.uFlags = NIF_INFO;
    d.dwInfoFlags = NIIF_INFO | NIIF_NOSOUND;
    copy_wstr(&mut d.szInfoTitle, title);
    copy_wstr(&mut d.szInfo, body);
    let _ = Shell_NotifyIconW(NIM_MODIFY, &d);
}

unsafe fn tray_menu() {
    let a = app();
    let Ok(menu) = CreatePopupMenu() else { return };
    let toggle = if a.recorder.is_some() { w!("Stop Recording") } else { w!("Start Recording") };
    let _ = AppendMenuW(menu, MF_STRING, IDM_TOGGLE, toggle);
    let _ = AppendMenuW(menu, MF_STRING, IDM_OPEN, w!("Open App"));
    let _ = AppendMenuW(menu, MF_STRING, IDM_FOLDER, w!("Open Recordings Folder"));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
    let _ = AppendMenuW(menu, MF_STRING, IDM_EXIT, w!("Exit"));
    let _ = SetMenuDefaultItem(menu, IDM_OPEN as u32, 0);

    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let _ = SetForegroundWindow(a.hwnd);
    let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY, pt.x, pt.y, Some(0), a.hwnd, None);
    let _ = DestroyMenu(menu);
    match cmd.0 as usize {
        IDM_TOGGLE => toggle_recording(),
        IDM_OPEN => show_window(),
        IDM_FOLDER => open_folder(),
        IDM_EXIT => exit_app(),
        _ => {}
    }
}

/// Draws a round "record" icon: grey ring when idle, red dot when recording.
unsafe fn make_dot_icon(recording: bool) -> HICON {
    let size = GetSystemMetrics(SM_CXSMICON).max(16) * 2;
    let n = size as usize;
    let mut px = vec![0u32; n * n];
    let c = size as f32 / 2.0;
    let r_outer = c - 0.5;
    let ring = size as f32 * 0.12;
    let r_dot = size as f32 * if recording { 0.30 } else { 0.22 };
    let rgbv: u32 = if recording { 0xEF4444 } else { 0x9A9AA4 };
    for y in 0..n {
        for x in 0..n {
            let d = ((x as f32 + 0.5 - c).powi(2) + (y as f32 + 0.5 - c).powi(2)).sqrt();
            let cov = |inner: f32, outer: f32| ((outer - d + 0.5).clamp(0.0, 1.0)) * ((d - inner + 0.5).clamp(0.0, 1.0));
            let a = cov(r_outer - ring, r_outer).max(cov(-1.0, r_dot));
            if a > 0.0 {
                let al = (a * 255.0) as u32;
                let pm = |ch: u32| ((ch * al) / 255) & 0xff;
                let (rr, gg, bb) = ((rgbv >> 16) & 0xff, (rgbv >> 8) & 0xff, rgbv & 0xff);
                px[y * n + x] = (al << 24) | (pm(rr) << 16) | (pm(gg) << 8) | pm(bb);
            }
        }
    }
    let bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size,
            biHeight: -size,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    let color = CreateDIBSection(None, &bi, DIB_RGB_COLORS, &mut bits, None, 0).unwrap_or_default();
    if !bits.is_null() {
        std::ptr::copy_nonoverlapping(px.as_ptr(), bits as *mut u32, n * n);
    }
    let mask = CreateBitmap(size, size, 1, 1, None);
    let info = ICONINFO { fIcon: true.into(), hbmMask: mask, hbmColor: color, ..Default::default() };
    let icon = CreateIconIndirect(&info).unwrap_or_default();
    let _ = DeleteObject(color.into());
    let _ = DeleteObject(mask.into());
    icon
}

// ----------------------------------------------------------- startup ----

fn startup_command() -> String {
    let exe = std::env::current_exe().unwrap_or_default();
    format!("\"{}\" --tray", exe.display())
}

unsafe fn startup_enabled() -> bool {
    let mut buf = [0u16; 1024];
    let mut len = (buf.len() * 2) as u32;
    let r = RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, APP_NAME, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr() as _), Some(&mut len));
    if r.is_err() {
        return false;
    }
    let s = String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1)]);
    s.eq_ignore_ascii_case(&startup_command())
}

unsafe fn set_startup(on: bool) {
    let mut key = HKEY::default();
    if RegCreateKeyW(HKEY_CURRENT_USER, RUN_KEY, &mut key).is_err() {
        return;
    }
    if on {
        let v: Vec<u16> = startup_command().encode_utf16().chain(Some(0)).collect();
        let bytes = std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 2);
        let _ = RegSetValueExW(key, APP_NAME, None, REG_SZ, Some(bytes));
    } else {
        let _ = RegDeleteValueW(key, APP_NAME);
    }
    let _ = RegCloseKey(key);
}

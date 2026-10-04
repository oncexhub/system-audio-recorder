//! Custom dark UI drawn with Direct2D/DirectWrite (built into Windows).
//! Everything is laid out in 96-DPI units; Direct2D scales it for the
//! monitor, so it stays crisp at any display scaling.

use windows::core::w;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows_numerics::Vector2;

pub const WIDTH: f32 = 460.0;
pub const HEIGHT: f32 = 628.0;

pub const WAVE_BARS: usize = 62;

const PAD: f32 = 24.0;
const INNER: f32 = 44.0; // content inset inside cards
const RIGHT: f32 = WIDTH - INNER;

// Palette
const BG: u32 = 0x0B0B0D;
const CARD: u32 = 0x121215;
const BORDER: u32 = 0x222228;
const DIVIDER: u32 = 0x1C1C21;
const CONTROL: u32 = 0x1B1B20;
const CONTROL_HOVER: u32 = 0x24242B;
const CONTROL_BORDER: u32 = 0x2C2C34;
const TEXT: u32 = 0xEDEDEF;
const MUTED: u32 = 0x8A8A94;
const FAINT: u32 = 0x5C5C66;
const ACCENT: u32 = 0x5B5BF0;
const ACCENT_HOVER: u32 = 0x6E6EF5;
const WAVE_A: u32 = 0x3B82F6;
const WAVE_B: u32 = 0x8B5CF6;
const GREEN: u32 = 0x4ADE80;
const GREEN_BG: u32 = 0x22C55E;
const RED: u32 = 0xF87171;
const RED_FILL: u32 = 0xDC2626;
const RED_HOVER: u32 = 0xEF4444;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    Record,
    Hotkey,
    FolderChange,
    FolderOpen,
    FormatWav,
    FormatMp3,
    Tray,
    Startup,
    Footer,
    FooterTrim,
    OpenEditor,
    // Editor
    Back,
    Wave,
    StartMinus,
    StartPlus,
    EndMinus,
    EndPlus,
    PlayStart,
    PlayEnd,
    PlayPause,
    SaveCopy,
    Replace,
    // First-run shortcut prompt
    ShortcutYes,
    ShortcutNo,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FooterKind {
    None,
    Success,
    Error,
    Info,
}

pub struct View<'a> {
    pub recording: bool,
    pub secs: u64,
    pub hotkey: &'a str,
    pub hotkey_ok: bool,
    pub capturing_hotkey: bool,
    pub folder: &'a str,
    pub mp3: bool,
    pub tray: bool,
    pub startup: bool,
    pub footer: &'a str,
    pub footer_kind: FooterKind,
    pub hover: Option<Hit>,
    pub waves: &'a [f32],
    pub can_trim: bool,
    pub editor: Option<EditorView<'a>>,
    pub shortcut_prompt: bool,
}

pub struct EditorView<'a> {
    pub name: &'a str,
    pub columns: &'a [f32],
    pub duration: f64,
    pub start: f64,
    pub end: f64,
    pub playhead: f64,
    pub playing: bool,
    pub loading: Option<f32>,
    pub active_start: bool,
}

struct Rect {
    l: f32,
    t: f32,
    r: f32,
    b: f32,
}

const fn rc(l: f32, t: f32, r: f32, b: f32) -> Rect {
    Rect { l, t, r, b }
}

impl Rect {
    fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.l && x < self.r && y >= self.t && y < self.b
    }
    fn d2d(&self) -> D2D_RECT_F {
        D2D_RECT_F { left: self.l, top: self.t, right: self.r, bottom: self.b }
    }
}

// ---- Layout -------------------------------------------------------------

const REC_CARD: Rect = rc(PAD, 66.0, WIDTH - PAD, 296.0);
const WAVE_CY: f32 = 118.0;
const WAVE_HALF: f32 = 30.0;
const TIMER: Rect = rc(INNER, 156.0, RIGHT, 200.0);
const TIMER_SUB: Rect = rc(INNER, 200.0, RIGHT, 220.0);
const RECORD_BTN: Rect = rc(INNER, 236.0, RIGHT, 276.0);

const SETTINGS_LABEL_Y: f32 = 314.0;
const SET_CARD_TOP: f32 = 334.0;
const ROW_H: f32 = 50.0;
const ROWS: usize = 5;
const SET_CARD: Rect = rc(PAD, SET_CARD_TOP, WIDTH - PAD, SET_CARD_TOP + ROW_H * ROWS as f32);
const FOOTER: Rect = rc(PAD, SET_CARD_TOP + ROW_H * ROWS as f32 + 8.0, WIDTH - PAD, HEIGHT - 6.0);

fn row_top(i: usize) -> f32 {
    SET_CARD_TOP + ROW_H * i as f32
}

fn control_rect(i: usize, l: f32, r: f32) -> Rect {
    let t = row_top(i) + 11.0;
    rc(l, t, r, t + 28.0)
}

fn hotkey_chip() -> Rect {
    control_rect(0, RIGHT - 150.0, RIGHT)
}
fn folder_change() -> Rect {
    control_rect(1, RIGHT - 150.0, RIGHT - 70.0)
}
fn folder_open() -> Rect {
    control_rect(1, RIGHT - 64.0, RIGHT)
}
fn format_seg() -> Rect {
    control_rect(2, RIGHT - 124.0, RIGHT)
}
fn toggle(i: usize) -> Rect {
    let t = row_top(i) + 15.0;
    rc(RIGHT - 36.0, t, RIGHT, t + 20.0)
}

/// Finds which interactive element is under a point (in 96-DPI units).
pub fn hit_test(x: f32, y: f32, v: &View) -> Option<Hit> {
    let seg = format_seg();
    let mid = (seg.l + seg.r) / 2.0;
    let candidates = [
        (RECORD_BTN, Hit::Record),
        (hotkey_chip(), Hit::Hotkey),
        (folder_change(), Hit::FolderChange),
        (folder_open(), Hit::FolderOpen),
        (rc(seg.l, seg.t, mid, seg.b), Hit::FormatWav),
        (rc(mid, seg.t, seg.r, seg.b), Hit::FormatMp3),
        (rc(INNER, row_top(3), RIGHT, row_top(4)), Hit::Tray),
        (rc(INNER, row_top(4), RIGHT, row_top(5)), Hit::Startup),
    ];
    for (r, h) in candidates {
        if r.contains(x, y) {
            let disabled = v.recording && matches!(h, Hit::FolderChange | Hit::FormatWav | Hit::FormatMp3);
            return if disabled { None } else { Some(h) };
        }
    }
    if open_editor_chip(v.recording).contains(x, y) {
        return Some(Hit::OpenEditor);
    }
    if v.footer_kind == FooterKind::Success {
        if v.can_trim && footer_trim_chip().contains(x, y) {
            return Some(Hit::FooterTrim);
        }
        if FOOTER.contains(x, y) {
            return Some(Hit::Footer);
        }
    }
    None
}

// ---- Header / footer extras ----

fn pill_rect(recording: bool) -> Rect {
    let w = if recording { 112.0 } else { 82.0 };
    rc(WIDTH - PAD - w, 22.0, WIDTH - PAD, 48.0)
}

fn open_editor_chip(recording: bool) -> Rect {
    let p = pill_rect(recording);
    rc(p.l - 8.0 - 64.0, 22.0, p.l - 8.0, 48.0)
}

fn footer_trim_chip() -> Rect {
    let cy = (FOOTER.t + FOOTER.b) / 2.0;
    rc(WIDTH - PAD - 56.0, cy - 13.0, WIDTH - PAD, cy + 13.0)
}

// ---- First-run shortcut prompt ----

const PROMPT: Rect = rc(PAD + 16.0, 190.0, WIDTH - PAD - 16.0, 400.0);
const PROMPT_NO: Rect = rc(PAD + 40.0, 340.0, WIDTH / 2.0 - 5.0, 380.0);
const PROMPT_YES: Rect = rc(WIDTH / 2.0 + 5.0, 340.0, WIDTH - PAD - 40.0, 380.0);

pub fn hit_test_prompt(x: f32, y: f32) -> Option<Hit> {
    if PROMPT_YES.contains(x, y) {
        Some(Hit::ShortcutYes)
    } else if PROMPT_NO.contains(x, y) {
        Some(Hit::ShortcutNo)
    } else {
        None
    }
}

// ---- Editor layout ----

pub const EDIT_COLS: usize = 186;
pub const WAVE_L: f32 = INNER;
pub const WAVE_R: f32 = RIGHT;
const E_WAVE_CARD: Rect = rc(PAD, 66.0, WIDTH - PAD, 300.0);
const E_WAVE: Rect = rc(INNER, 84.0, RIGHT, 236.0);
const E_TRIM_TOP: f32 = 316.0;
const E_TRIM_CARD: Rect = rc(PAD, E_TRIM_TOP, WIDTH - PAD, E_TRIM_TOP + 100.0);
const E_PLAY: Rect = rc(PAD, 432.0, WIDTH - PAD, 472.0);
const E_SAVE: Rect = rc(PAD, 484.0, WIDTH / 2.0 - 4.0, 526.0);
const E_REPLACE: Rect = rc(WIDTH / 2.0 + 4.0, 484.0, WIDTH - PAD, 526.0);
const E_FOOTER: Rect = rc(PAD, 540.0, WIDTH - PAD, 566.0);
const BACK: Rect = rc(PAD, 20.0, PAD + 30.0, 50.0);

fn e_row(i: usize) -> f32 {
    E_TRIM_TOP + ROW_H * i as f32
}
fn e_ctrl(i: usize, l: f32, r: f32) -> Rect {
    let t = e_row(i) + 11.0;
    rc(l, t, r, t + 28.0)
}
fn e_play(i: usize) -> Rect {
    e_ctrl(i, RIGHT - 36.0, RIGHT)
}
fn e_plus(i: usize) -> Rect {
    e_ctrl(i, RIGHT - 74.0, RIGHT - 44.0)
}
fn e_minus(i: usize) -> Rect {
    e_ctrl(i, RIGHT - 108.0, RIGHT - 78.0)
}
fn e_time(i: usize) -> Rect {
    e_ctrl(i, RIGHT - 220.0, RIGHT - 116.0)
}

pub fn wave_x(t: f64, dur: f64) -> f32 {
    if dur <= 0.0 {
        return WAVE_L;
    }
    WAVE_L + ((t / dur).clamp(0.0, 1.0) as f32) * (WAVE_R - WAVE_L)
}

pub fn wave_time(x: f32, dur: f64) -> f64 {
    (((x - WAVE_L) / (WAVE_R - WAVE_L)).clamp(0.0, 1.0) as f64) * dur
}

pub fn hit_test_editor(x: f32, y: f32, e: &EditorView) -> Option<Hit> {
    let wave_zone = rc(E_WAVE.l - 8.0, E_WAVE.t - 6.0, E_WAVE.r + 8.0, E_WAVE.b + 6.0);
    let mut candidates = vec![
        (BACK, Hit::Back),
        (wave_zone, Hit::Wave),
        (e_minus(0), Hit::StartMinus),
        (e_plus(0), Hit::StartPlus),
        (e_play(0), Hit::PlayStart),
        (e_minus(1), Hit::EndMinus),
        (e_plus(1), Hit::EndPlus),
        (e_play(1), Hit::PlayEnd),
        (E_PLAY, Hit::PlayPause),
    ];
    if e.loading.is_none() {
        candidates.push((E_SAVE, Hit::SaveCopy));
        candidates.push((E_REPLACE, Hit::Replace));
    }
    candidates.into_iter().find(|(r, _)| r.contains(x, y)).map(|(_, h)| h)
}

/// "mm:ss.t" (or "h:mm:ss.t" for long files).
pub fn fmt_precise(t: f64) -> String {
    let t = t.max(0.0);
    let tenths = (t * 10.0).round() as u64;
    let (s, d) = (tenths / 10, tenths % 10);
    if s >= 3600 {
        format!("{}:{:02}:{:02}.{}", s / 3600, s / 60 % 60, s % 60, d)
    } else {
        format!("{:02}:{:02}.{}", s / 60, s % 60, d)
    }
}

fn fmt_secs(t: f64) -> String {
    if t < 60.0 {
        format!("{:.1} s", t)
    } else {
        fmt_precise(t)
    }
}

// ---- Renderer -----------------------------------------------------------

pub struct Gfx {
    d2d: ID2D1Factory,
    dw: IDWriteFactory,
    target: Option<Target>,
    title: IDWriteTextFormat,
    timer: IDWriteTextFormat,
    body_bold: IDWriteTextFormat,
    small: IDWriteTextFormat,
    caps: IDWriteTextFormat,
    button: IDWriteTextFormat,
}

struct Target {
    rt: ID2D1HwndRenderTarget,
    brush: ID2D1SolidColorBrush,
    wave: ID2D1LinearGradientBrush,
}

#[derive(Clone, Copy)]
enum Align {
    Left,
    Center,
    Right,
}

fn color(hex: u32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: ((hex >> 16) & 0xff) as f32 / 255.0,
        g: ((hex >> 8) & 0xff) as f32 / 255.0,
        b: (hex & 0xff) as f32 / 255.0,
        a,
    }
}

fn v2(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

impl Gfx {
    pub fn new() -> windows::core::Result<Gfx> {
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let fmt = |size: f32, weight: DWRITE_FONT_WEIGHT| -> windows::core::Result<IDWriteTextFormat> {
                let f = dw.CreateTextFormat(
                    w!("Segoe UI"),
                    None,
                    weight,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    w!("en-us"),
                )?;
                f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
                f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
                let sign = dw.CreateEllipsisTrimmingSign(&f)?;
                let trim = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
                f.SetTrimming(&trim, &sign)?;
                Ok(f)
            };
            Ok(Gfx {
                title: fmt(15.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                timer: fmt(38.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                body_bold: fmt(13.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                small: fmt(11.5, DWRITE_FONT_WEIGHT_NORMAL)?,
                caps: fmt(10.5, DWRITE_FONT_WEIGHT_BOLD)?,
                button: fmt(13.5, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                d2d,
                dw,
                target: None,
            })
        }
    }

    fn ensure_target(&mut self, hwnd: HWND, w: u32, h: u32, dpi: f32) -> windows::core::Result<()> {
        if self.target.is_some() {
            return Ok(());
        }
        unsafe {
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: dpi,
                dpiY: dpi,
                ..Default::default()
            };
            let hprops = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd,
                pixelSize: D2D_SIZE_U { width: w, height: h },
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            };
            let rt = self.d2d.CreateHwndRenderTarget(&props, &hprops)?;
            let brush = rt.CreateSolidColorBrush(&color(TEXT, 1.0), None)?;
            let stops = [
                D2D1_GRADIENT_STOP { position: 0.0, color: color(WAVE_A, 1.0) },
                D2D1_GRADIENT_STOP { position: 1.0, color: color(WAVE_B, 1.0) },
            ];
            let coll = rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)?;
            let lp = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: v2(INNER, 0.0), endPoint: v2(RIGHT, 0.0) };
            let wave = rt.CreateLinearGradientBrush(&lp, None, &coll)?;
            self.target = Some(Target { rt, brush, wave });
        }
        Ok(())
    }

    pub fn resize(&mut self, w: u32, h: u32, dpi: f32) {
        if let Some(t) = &self.target {
            unsafe {
                t.rt.SetDpi(dpi, dpi);
                if t.rt.Resize(&D2D_SIZE_U { width: w, height: h }).is_err() {
                    self.target = None;
                }
            }
        }
    }

    pub fn draw(&mut self, hwnd: HWND, w: u32, h: u32, dpi: f32, v: &View) {
        if self.ensure_target(hwnd, w, h, dpi).is_err() {
            return;
        }
        let ok = unsafe {
            let t = self.target.as_ref().unwrap();
            t.rt.BeginDraw();
            t.rt.Clear(Some(&color(BG, 1.0)));
            self.paint(t, v);
            t.rt.EndDraw(None, None)
        };
        if ok.is_err() {
            // Device lost (driver update, sleep, ...): recreate next frame.
            self.target = None;
        }
    }

    // ---- primitives ----

    unsafe fn fill(&self, t: &Target, r: &Rect, radius: f32, c: D2D1_COLOR_F) {
        t.brush.SetColor(&c);
        let rr = D2D1_ROUNDED_RECT { rect: r.d2d(), radiusX: radius, radiusY: radius };
        t.rt.FillRoundedRectangle(&rr, &t.brush);
    }

    unsafe fn stroke(&self, t: &Target, r: &Rect, radius: f32, c: D2D1_COLOR_F) {
        t.brush.SetColor(&c);
        let inset = rc(r.l + 0.5, r.t + 0.5, r.r - 0.5, r.b - 0.5);
        let rr = D2D1_ROUNDED_RECT { rect: inset.d2d(), radiusX: radius, radiusY: radius };
        t.rt.DrawRoundedRectangle(&rr, &t.brush, 1.0, None);
    }

    unsafe fn circle(&self, t: &Target, x: f32, y: f32, r: f32, c: D2D1_COLOR_F) {
        t.brush.SetColor(&c);
        let e = D2D1_ELLIPSE { point: v2(x, y), radiusX: r, radiusY: r };
        t.rt.FillEllipse(&e, &t.brush);
    }

    unsafe fn text(&self, t: &Target, s: &str, f: &IDWriteTextFormat, r: &Rect, c: D2D1_COLOR_F, align: Align) {
        let a = match align {
            Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
            Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
            Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
        };
        let _ = f.SetTextAlignment(a);
        t.brush.SetColor(&c);
        let s: Vec<u16> = s.encode_utf16().collect();
        t.rt.DrawText(&s, f, &r.d2d(), &t.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
    }

    unsafe fn text_width(&self, s: &str, f: &IDWriteTextFormat) -> f32 {
        let s: Vec<u16> = s.encode_utf16().collect();
        match self.dw.CreateTextLayout(&s, f, 1000.0, 100.0) {
            Ok(l) => {
                let mut m = DWRITE_TEXT_METRICS::default();
                if l.GetMetrics(&mut m).is_ok() {
                    m.widthIncludingTrailingWhitespace
                } else {
                    0.0
                }
            }
            Err(_) => 0.0,
        }
    }

    unsafe fn card(&self, t: &Target, r: &Rect) {
        self.fill(t, r, 12.0, color(CARD, 1.0));
        self.stroke(t, r, 12.0, color(BORDER, 1.0));
    }

    unsafe fn chip_button(&self, t: &Target, r: &Rect, label: &str, hover: bool, enabled: bool) {
        let alpha = if enabled { 1.0 } else { 0.45 };
        self.fill(t, r, 7.0, color(if hover { CONTROL_HOVER } else { CONTROL }, alpha));
        self.stroke(t, r, 7.0, color(CONTROL_BORDER, alpha));
        self.text(t, label, &self.body_bold, r, color(TEXT, alpha), Align::Center);
    }

    // ---- the whole window ----

    unsafe fn paint(&self, t: &Target, v: &View) {
        if let Some(e) = &v.editor {
            self.editor(t, v, e);
            return;
        }
        self.header(t, v);
        self.recorder_card(t, v);
        self.settings(t, v);
        self.footer(t, v);
        if v.shortcut_prompt {
            self.shortcut_prompt(t, v);
        }
    }

    /// Centered card asking once whether to add the SAR shortcut.
    unsafe fn shortcut_prompt(&self, t: &Target, v: &View) {
        // Dim everything behind the card.
        t.brush.SetColor(&color(BG, 0.78));
        t.rt.FillRectangle(&rc(0.0, 0.0, WIDTH, HEIGHT + 200.0).d2d(), &t.brush);

        self.fill(t, &PROMPT, 14.0, color(CARD, 1.0));
        self.stroke(t, &PROMPT, 14.0, color(CONTROL_BORDER, 1.0));

        // App logo tile, as on the desktop shortcut.
        let cx = WIDTH / 2.0;
        let logo = rc(cx - 24.0, PROMPT.t + 24.0, cx + 24.0, PROMPT.t + 72.0);
        self.fill(t, &logo, 12.0, color(0x16161C, 1.0));
        self.stroke(t, &logo, 12.0, color(BORDER, 1.0));
        let heights = [11.0, 22.0, 32.0, 18.0, 26.0];
        for (i, h) in heights.iter().enumerate() {
            let x = logo.l + 11.0 + i as f32 * 5.6;
            let my = (logo.t + logo.b) / 2.0;
            let bar = rc(x, my - h / 2.0, x + 3.4, my + h / 2.0);
            let rr = D2D1_ROUNDED_RECT { rect: bar.d2d(), radiusX: 1.7, radiusY: 1.7 };
            let c = if i < 2 { WAVE_A } else if i < 4 { 0x6C6CF7 } else { WAVE_B };
            t.brush.SetColor(&color(c, 1.0));
            t.rt.FillRoundedRectangle(&rr, &t.brush);
        }

        self.text(t, "Add SAR to your desktop?", &self.title, &rc(PROMPT.l, PROMPT.t + 84.0, PROMPT.r, PROMPT.t + 110.0), color(TEXT, 1.0), Align::Center);
        self.text(t, "One click to open the recorder.", &self.small, &rc(PROMPT.l, PROMPT.t + 110.0, PROMPT.r, PROMPT.t + 128.0), color(MUTED, 1.0), Align::Center);
        self.text(t, "Also adds it to the Start menu.", &self.small, &rc(PROMPT.l, PROMPT.t + 127.0, PROMPT.r, PROMPT.t + 145.0), color(MUTED, 1.0), Align::Center);

        let hov = |h: Hit| v.hover == Some(h);
        self.fill(t, &PROMPT_NO, 10.0, color(if hov(Hit::ShortcutNo) { CONTROL_HOVER } else { CONTROL }, 1.0));
        self.stroke(t, &PROMPT_NO, 10.0, color(CONTROL_BORDER, 1.0));
        self.text(t, "No thanks", &self.button, &PROMPT_NO, color(TEXT, 1.0), Align::Center);
        self.fill(t, &PROMPT_YES, 10.0, color(if hov(Hit::ShortcutYes) { ACCENT_HOVER } else { ACCENT }, 1.0));
        self.text(t, "Add shortcut", &self.button, &PROMPT_YES, color(0xFFFFFF, 1.0), Align::Center);
    }

    unsafe fn status_pill(&self, t: &Target, recording: bool) {
        let (label, fg, bg) = if recording { ("RECORDING", RED, RED_FILL) } else { ("READY", GREEN, GREEN_BG) };
        let pill = pill_rect(recording);
        self.fill(t, &pill, 13.0, color(bg, 0.12));
        self.stroke(t, &pill, 13.0, color(bg, 0.35));
        self.circle(t, pill.l + 15.0, 35.0, 3.5, color(fg, 1.0));
        self.text(t, label, &self.caps, &rc(pill.l + 25.0, pill.t, pill.r, pill.b), color(fg, 1.0), Align::Left);
    }

    /// Filled triangle pointing right (play icon).
    unsafe fn play_icon(&self, t: &Target, cx: f32, cy: f32, size: f32, c: D2D1_COLOR_F) {
        let Ok(geo) = self.d2d.CreatePathGeometry() else { return };
        let Ok(sink) = geo.Open() else { return };
        let h = size / 2.0;
        sink.BeginFigure(v2(cx - h * 0.8, cy - h), D2D1_FIGURE_BEGIN_FILLED);
        sink.AddLine(v2(cx + h, cy));
        sink.AddLine(v2(cx - h * 0.8, cy + h));
        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
        let _ = sink.Close();
        t.brush.SetColor(&c);
        t.rt.FillGeometry(&geo, &t.brush, None);
    }

    unsafe fn pause_icon(&self, t: &Target, cx: f32, cy: f32, size: f32, c: D2D1_COLOR_F) {
        let h = size / 2.0;
        self.fill(t, &rc(cx - h * 0.8, cy - h, cx - h * 0.2, cy + h), 1.0, c);
        self.fill(t, &rc(cx + h * 0.2, cy - h, cx + h * 0.8, cy + h), 1.0, c);
    }

    unsafe fn editor(&self, t: &Target, v: &View, e: &EditorView) {
        let muted = color(MUTED, 1.0);
        let hov = |h: Hit| v.hover == Some(h);

        // Header: back button + file name + status
        self.fill(t, &BACK, 8.0, color(if hov(Hit::Back) { CONTROL_HOVER } else { CONTROL }, 1.0));
        self.stroke(t, &BACK, 8.0, color(CONTROL_BORDER, 1.0));
        t.brush.SetColor(&color(TEXT, 1.0));
        let (cx, cy) = (BACK.l + 14.0, 35.0);
        t.rt.DrawLine(v2(cx + 3.0, cy - 6.0), v2(cx - 3.0, cy), &t.brush, 1.8, None);
        t.rt.DrawLine(v2(cx - 3.0, cy), v2(cx + 3.0, cy + 6.0), &t.brush, 1.8, None);
        let pill = pill_rect(v.recording);
        self.text(t, e.name, &self.title, &rc(PAD + 42.0, 20.0, pill.l - 10.0, 50.0), color(TEXT, 1.0), Align::Left);
        self.status_pill(t, v.recording);

        // Waveform card
        self.card(t, &E_WAVE_CARD);
        let cy = (E_WAVE.t + E_WAVE.b) / 2.0;
        let half = (E_WAVE.b - E_WAVE.t) / 2.0;
        let xs = wave_x(e.start, e.duration);
        let xe = wave_x(e.end, e.duration);
        t.brush.SetColor(&color(ACCENT, 0.07));
        t.rt.FillRectangle(&rc(xs, E_WAVE.t, xe, E_WAVE.b).d2d(), &t.brush);
        let step = (WAVE_R - WAVE_L) / e.columns.len().max(1) as f32;
        for (i, &p) in e.columns.iter().enumerate() {
            let x = WAVE_L + i as f32 * step;
            let h = 2.0 + p.powf(0.7) * (half * 2.0 - 6.0);
            let bar = rc(x, cy - h / 2.0, x + step * 0.7, cy + h / 2.0);
            let mid = x + step * 0.35;
            let rr = D2D1_ROUNDED_RECT { rect: bar.d2d(), radiusX: 0.7, radiusY: 0.7 };
            if mid >= xs && mid <= xe {
                t.rt.FillRoundedRectangle(&rr, &t.wave);
            } else {
                t.brush.SetColor(&color(0x2E2E36, 1.0));
                t.rt.FillRoundedRectangle(&rr, &t.brush);
            }
        }
        // Handles
        for (x, is_start) in [(xs, true), (xe, false)] {
            let active = is_start == e.active_start;
            let c = if active { color(0xFFFFFF, 1.0) } else { color(0xB4B4BC, 1.0) };
            t.brush.SetColor(&c);
            t.rt.FillRectangle(&rc(x - 1.0, E_WAVE.t - 4.0, x + 1.0, E_WAVE.b + 4.0).d2d(), &t.brush);
            let grip = rc(x - 6.0, cy - 15.0, x + 6.0, cy + 15.0);
            self.fill(t, &grip, 5.0, c);
            t.brush.SetColor(&color(0x3A3A44, 1.0));
            for dy in [-4.0f32, 0.0, 4.0] {
                t.rt.FillRectangle(&rc(x - 3.0, cy + dy - 0.6, x + 3.0, cy + dy + 0.6).d2d(), &t.brush);
            }
        }
        // Playhead
        if e.playing || (e.playhead > e.start + 0.01 && e.playhead < e.end - 0.01) {
            let x = wave_x(e.playhead, e.duration);
            t.brush.SetColor(&color(0xFFFFFF, 0.9));
            t.rt.FillRectangle(&rc(x - 0.75, E_WAVE.t, x + 0.75, E_WAVE.b).d2d(), &t.brush);
            self.circle(t, x, E_WAVE.t, 3.5, color(0xFFFFFF, 1.0));
        }
        // Labels under the waveform
        let lab = rc(INNER, 242.0, RIGHT, 260.0);
        self.text(t, &fmt_precise(0.0), &self.small, &lab, color(FAINT, 1.0), Align::Left);
        self.text(t, &fmt_precise(e.duration), &self.small, &lab, color(FAINT, 1.0), Align::Right);
        let len = format!("Length  {}", fmt_precise(e.end - e.start));
        self.text(t, &len, &self.body_bold, &lab, color(TEXT, 1.0), Align::Center);
        let hint = match e.loading {
            Some(p) => format!("Loading waveform\u{2026}  {}%", (p * 100.0) as u32),
            None => "Drag the handles to cut  \u{00B7}  Space to play  \u{00B7}  \u{2190} \u{2192} to fine-tune".to_string(),
        };
        self.text(t, &hint, &self.small, &rc(INNER, 266.0, RIGHT, 284.0), muted, Align::Center);

        // Start / End rows
        self.card(t, &E_TRIM_CARD);
        t.brush.SetColor(&color(DIVIDER, 1.0));
        t.rt.FillRectangle(&rc(INNER, e_row(1), RIGHT, e_row(1) + 1.0).d2d(), &t.brush);
        let rows = [
            ("Start", e.start, e.start, Hit::StartMinus, Hit::StartPlus, Hit::PlayStart, e.active_start),
            ("End", e.end, e.duration - e.end, Hit::EndMinus, Hit::EndPlus, Hit::PlayEnd, !e.active_start),
        ];
        for (i, (label, at, cut, minus, plus, play, active)) in rows.into_iter().enumerate() {
            let top = e_row(i);
            let lc = if active { color(ACCENT_HOVER, 1.0) } else { color(TEXT, 1.0) };
            self.text(t, label, &self.body_bold, &rc(INNER, top + 7.0, e_time(i).l - 6.0, top + 26.0), lc, Align::Left);
            let sub = if cut >= 0.05 { format!("Cuts {}", fmt_secs(cut)) } else { "Nothing cut".to_string() };
            self.text(t, &sub, &self.small, &rc(INNER, top + 26.0, e_time(i).l - 6.0, top + 43.0), muted, Align::Left);
            self.text(t, &fmt_precise(at), &self.body_bold, &e_time(i), color(TEXT, 1.0), Align::Right);
            self.chip_button(t, &e_minus(i), "\u{2212}", hov(minus), true);
            self.chip_button(t, &e_plus(i), "+", hov(plus), true);
            let pr = e_play(i);
            self.fill(t, &pr, 7.0, color(if hov(play) { CONTROL_HOVER } else { CONTROL }, 1.0));
            self.stroke(t, &pr, 7.0, color(CONTROL_BORDER, 1.0));
            self.play_icon(t, (pr.l + pr.r) / 2.0 + 1.0, (pr.t + pr.b) / 2.0, 10.0, color(TEXT, 1.0));
        }

        // Play / pause
        self.fill(t, &E_PLAY, 10.0, color(if hov(Hit::PlayPause) { CONTROL_HOVER } else { CONTROL }, 1.0));
        self.stroke(t, &E_PLAY, 10.0, color(CONTROL_BORDER, 1.0));
        let label = if e.playing { "Pause" } else { "Play selection" };
        let tw = self.text_width(label, &self.button);
        let (cx, cy) = ((E_PLAY.l + E_PLAY.r) / 2.0, (E_PLAY.t + E_PLAY.b) / 2.0);
        let ix = cx - (tw + 20.0) / 2.0;
        if e.playing {
            self.pause_icon(t, ix + 5.0, cy, 11.0, color(TEXT, 1.0));
        } else {
            self.play_icon(t, ix + 5.0, cy, 11.0, color(TEXT, 1.0));
        }
        self.text(t, label, &self.button, &rc(ix + 20.0, E_PLAY.t, E_PLAY.r, E_PLAY.b), color(TEXT, 1.0), Align::Left);

        // Save buttons
        let a = if e.loading.is_none() { 1.0 } else { 0.45 };
        self.fill(t, &E_SAVE, 10.0, color(if hov(Hit::SaveCopy) { ACCENT_HOVER } else { ACCENT }, a));
        self.text(t, "Save as copy", &self.button, &E_SAVE, color(0xFFFFFF, a), Align::Center);
        self.fill(t, &E_REPLACE, 10.0, color(if hov(Hit::Replace) { CONTROL_HOVER } else { CONTROL }, a));
        self.stroke(t, &E_REPLACE, 10.0, color(CONTROL_BORDER, a));
        self.text(t, "Replace original", &self.button, &E_REPLACE, color(TEXT, a), Align::Center);

        // Footer
        let c = match v.footer_kind {
            FooterKind::None => None,
            FooterKind::Success => Some(color(GREEN, 1.0)),
            FooterKind::Error => Some(color(RED, 1.0)),
            FooterKind::Info => Some(muted),
        };
        if let Some(c) = c {
            self.text(t, v.footer, &self.small, &E_FOOTER, c, Align::Center);
        }
    }

    unsafe fn header(&self, t: &Target, v: &View) {
        // Logo: rounded tile with a little gradient waveform.
        let logo = rc(PAD, 20.0, PAD + 30.0, 50.0);
        self.fill(t, &logo, 8.0, color(0x16161C, 1.0));
        self.stroke(t, &logo, 8.0, color(BORDER, 1.0));
        let heights = [6.0, 12.0, 18.0, 10.0, 14.0];
        for (i, h) in heights.iter().enumerate() {
            let x = logo.l + 7.0 + i as f32 * 3.6;
            let bar = rc(x, 35.0 - h / 2.0, x + 2.0, 35.0 + h / 2.0);
            let rr = D2D1_ROUNDED_RECT { rect: bar.d2d(), radiusX: 1.0, radiusY: 1.0 };
            let c = if i < 2 { WAVE_A } else if i < 4 { 0x6C6CF7 } else { WAVE_B };
            t.brush.SetColor(&color(c, 1.0));
            t.rt.FillRoundedRectangle(&rr, &t.brush);
        }
        self.text(t, "System Audio Recorder", &self.title, &rc(PAD + 42.0, 20.0, open_editor_chip(v.recording).l - 6.0, 50.0), color(TEXT, 1.0), Align::Left);

        self.status_pill(t, v.recording);
        let chip = open_editor_chip(v.recording);
        let hover = v.hover == Some(Hit::OpenEditor);
        self.fill(t, &chip, 13.0, color(if hover { CONTROL_HOVER } else { CONTROL }, 1.0));
        self.stroke(t, &chip, 13.0, color(CONTROL_BORDER, 1.0));
        self.text(t, "TRIM", &self.caps, &chip, color(if hover { TEXT } else { MUTED }, 1.0), Align::Center);
    }

    unsafe fn recorder_card(&self, t: &Target, v: &View) {
        self.card(t, &REC_CARD);

        // Waveform
        let bar_w = 3.0;
        let gap = 3.0;
        let total = WAVE_BARS as f32 * (bar_w + gap) - gap;
        let x0 = INNER + ((RIGHT - INNER) - total) / 2.0;
        if !v.recording {
            t.brush.SetColor(&color(0x26262E, 1.0));
        }
        for i in 0..WAVE_BARS {
            let h = if v.recording {
                let lvl = v.waves.get(i).copied().unwrap_or(0.0);
                3.0 + lvl * (WAVE_HALF * 2.0 - 3.0)
            } else {
                // Calm static wave while idle: a soft bell-shaped ripple.
                let p = i as f32 / (WAVE_BARS - 1) as f32;
                let bell = (p * std::f32::consts::PI).sin().powf(1.5);
                let ripple = 0.55 + 0.45 * (p * 23.0).sin() * (p * 7.0 + 1.0).cos();
                3.0 + 22.0 * bell * ripple.abs()
            };
            let x = x0 + i as f32 * (bar_w + gap);
            let bar = rc(x, WAVE_CY - h / 2.0, x + bar_w, WAVE_CY + h / 2.0);
            let rr = D2D1_ROUNDED_RECT { rect: bar.d2d(), radiusX: 1.5, radiusY: 1.5 };
            if v.recording {
                t.rt.FillRoundedRectangle(&rr, &t.wave);
            } else {
                t.rt.FillRoundedRectangle(&rr, &t.brush);
            }
        }

        // Timer
        let s = v.secs;
        let time = format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60);
        let tc = if v.recording { color(TEXT, 1.0) } else { color(FAINT, 1.0) };
        self.text(t, &time, &self.timer, &TIMER, tc, Align::Center);
        let sub = if v.recording {
            "Recording system audio  \u{00B7}  microphone is never recorded".to_string()
        } else if v.hotkey_ok {
            format!("Press {} anywhere to start", v.hotkey)
        } else {
            "Set a hotkey below to record from anywhere".to_string()
        };
        self.text(t, &sub, &self.small, &TIMER_SUB, color(MUTED, 1.0), Align::Center);

        // Big button
        let hover = v.hover == Some(Hit::Record);
        let (fill, label) = if v.recording {
            (if hover { RED_HOVER } else { RED_FILL }, "Stop Recording")
        } else {
            (if hover { ACCENT_HOVER } else { ACCENT }, "Start Recording")
        };
        self.fill(t, &RECORD_BTN, 10.0, color(fill, 1.0));
        let tw = self.text_width(label, &self.button);
        let cx = (RECORD_BTN.l + RECORD_BTN.r) / 2.0;
        let cy = (RECORD_BTN.t + RECORD_BTN.b) / 2.0;
        let icon_x = cx - (tw + 18.0) / 2.0;
        if v.recording {
            self.fill(t, &rc(icon_x, cy - 4.5, icon_x + 9.0, cy + 4.5), 2.0, color(0xFFFFFF, 1.0));
        } else {
            self.circle(t, icon_x + 4.5, cy, 4.5, color(0xFFFFFF, 1.0));
        }
        self.text(t, label, &self.button, &rc(icon_x + 18.0, RECORD_BTN.t, RECORD_BTN.r, RECORD_BTN.b), color(0xFFFFFF, 1.0), Align::Left);
    }

    unsafe fn row_labels(&self, t: &Target, i: usize, label: &str, sub: &str, sub_color: D2D1_COLOR_F, right_limit: f32) {
        let top = row_top(i);
        self.text(t, label, &self.body_bold, &rc(INNER, top + 7.0, right_limit, top + 26.0), color(TEXT, 1.0), Align::Left);
        self.text(t, sub, &self.small, &rc(INNER, top + 26.0, right_limit, top + 43.0), sub_color, Align::Left);
    }

    unsafe fn settings(&self, t: &Target, v: &View) {
        self.text(t, "SETTINGS", &self.caps, &rc(PAD + 2.0, SETTINGS_LABEL_Y - 8.0, 200.0, SETTINGS_LABEL_Y + 8.0), color(MUTED, 1.0), Align::Left);
        self.card(t, &SET_CARD);
        for i in 1..ROWS {
            t.brush.SetColor(&color(DIVIDER, 1.0));
            t.rt.FillRectangle(&rc(INNER, row_top(i), RIGHT, row_top(i) + 1.0).d2d(), &t.brush);
        }
        let muted = color(MUTED, 1.0);

        // 0: hotkey
        let chip = hotkey_chip();
        if v.capturing_hotkey {
            self.row_labels(t, 0, "Recording hotkey", "Press the new keys  \u{00B7}  Esc to cancel", color(ACCENT_HOVER, 1.0), chip.l - 10.0);
            self.fill(t, &chip, 7.0, color(ACCENT, 0.12));
            self.stroke(t, &chip, 7.0, color(ACCENT, 1.0));
            self.text(t, "Press keys\u{2026}", &self.body_bold, &chip, color(ACCENT_HOVER, 1.0), Align::Center);
        } else {
            let sub = if v.hotkey_ok { "Start and stop from anywhere" } else { "Not active  \u{00B7}  click to choose another" };
            let sc = if v.hotkey_ok { muted } else { color(RED, 1.0) };
            self.row_labels(t, 0, "Recording hotkey", sub, sc, chip.l - 10.0);
            self.chip_button(t, &chip, v.hotkey, v.hover == Some(Hit::Hotkey), true);
        }

        // 1: folder
        let ch = folder_change();
        self.row_labels(t, 1, "Save location", v.folder, muted, ch.l - 10.0);
        self.chip_button(t, &ch, "Change", v.hover == Some(Hit::FolderChange), !v.recording);
        self.chip_button(t, &folder_open(), "Open", v.hover == Some(Hit::FolderOpen), true);

        // 2: format
        let seg = format_seg();
        let sub = if v.mp3 { "320 kbps  \u{00B7}  about 140 MB per hour" } else { "Lossless  \u{00B7}  about 690 MB per hour" };
        self.row_labels(t, 2, "Format", sub, muted, seg.l - 10.0);
        let alpha = if v.recording { 0.45 } else { 1.0 };
        self.fill(t, &seg, 7.0, color(CONTROL, alpha));
        self.stroke(t, &seg, 7.0, color(CONTROL_BORDER, alpha));
        let mid = (seg.l + seg.r) / 2.0;
        let halves = [(rc(seg.l, seg.t, mid, seg.b), "WAV", !v.mp3, Hit::FormatWav), (rc(mid, seg.t, seg.r, seg.b), "MP3", v.mp3, Hit::FormatMp3)];
        for (r, label, selected, hit) in halves {
            let inner = rc(r.l + 3.0, r.t + 3.0, r.r - 3.0, r.b - 3.0);
            if selected {
                self.fill(t, &inner, 5.0, color(0x30303A, alpha));
            } else if v.hover == Some(hit) {
                self.fill(t, &inner, 5.0, color(CONTROL_HOVER, alpha));
            }
            let c = if selected { color(TEXT, alpha) } else { color(MUTED, alpha) };
            self.text(t, label, &self.body_bold, &r, c, Align::Center);
        }

        // 3, 4: toggles
        self.row_labels(t, 3, "Keep running in tray", "Closing the window keeps the hotkey active", muted, RIGHT - 50.0);
        self.toggle(t, &toggle(3), v.tray, v.hover == Some(Hit::Tray));
        self.row_labels(t, 4, "Start with Windows", "Launches quietly in the tray", muted, RIGHT - 50.0);
        self.toggle(t, &toggle(4), v.startup, v.hover == Some(Hit::Startup));
    }

    unsafe fn toggle(&self, t: &Target, r: &Rect, on: bool, hover: bool) {
        let track = if on {
            if hover { ACCENT_HOVER } else { ACCENT }
        } else if hover {
            0x34343C
        } else {
            0x2A2A31
        };
        self.fill(t, r, 10.0, color(track, 1.0));
        let cy = (r.t + r.b) / 2.0;
        let (kx, kc) = if on { (r.r - 10.0, 0xFFFFFF) } else { (r.l + 10.0, 0xB4B4BC) };
        self.circle(t, kx, cy, 7.0, color(kc, 1.0));
    }

    unsafe fn footer(&self, t: &Target, v: &View) {
        let c = match v.footer_kind {
            FooterKind::None => return,
            FooterKind::Success => color(if v.hover == Some(Hit::Footer) { 0x86EFAC } else { GREEN }, 1.0),
            FooterKind::Error => color(RED, 1.0),
            FooterKind::Info => color(MUTED, 1.0),
        };
        if v.footer_kind == FooterKind::Success && v.can_trim {
            let chip = footer_trim_chip();
            self.text(t, v.footer, &self.small, &rc(FOOTER.l, FOOTER.t, chip.l - 8.0, FOOTER.b), c, Align::Left);
            self.chip_button(t, &chip, "Trim", v.hover == Some(Hit::FooterTrim), true);
        } else {
            self.text(t, v.footer, &self.small, &FOOTER, c, Align::Center);
        }
    }
}

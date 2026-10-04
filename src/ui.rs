//! Custom dark UI drawn with Direct2D/DirectWrite (built into Windows).
//! Everything is laid out in 96-DPI units; Direct2D scales it for the
//! monitor, so it stays crisp at any display scaling. The layout follows the
//! window size: cards get wider, and waveforms get the extra height.

use std::cell::Cell;

use windows::core::w;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows_numerics::Vector2;

/// Smallest (and default) client size.
pub const MIN_W: f32 = 460.0;
pub const MIN_H: f32 = 678.0;

/// Longest waveform history the main screen can show (bars).
pub const MAX_WAVE_BARS: usize = 400;

const PAD: f32 = 24.0;
const INNER: f32 = 44.0; // content inset inside cards

thread_local! {
    static SIZE: Cell<(f32, f32)> = const { Cell::new((MIN_W, MIN_H)) };
}

/// Sets the current client size (in 96-DPI units).
pub fn set_size(w: f32, h: f32) {
    SIZE.with(|s| s.set((w.max(MIN_W), h.max(MIN_H))));
}

fn ww() -> f32 {
    SIZE.with(|s| s.get().0)
}
fn hh() -> f32 {
    SIZE.with(|s| s.get().1)
}
fn right() -> f32 {
    ww() - INNER
}
/// Extra height beyond the minimum, given to the waveforms.
fn extra() -> f32 {
    hh() - MIN_H
}

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
    FormatM4a,
    /// Index into `BITRATES`.
    Bitrate(u8),
    Tray,
    Startup,
    Footer,
    FooterTrim,
    OpenEditor,
    // Editor
    Back,
    Wave,
    Overview,
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
    /// 0 = WAV, 1 = MP3, 2 = M4A
    pub format: u8,
    pub kbps: u32,
    pub tray: bool,
    pub startup: bool,
    pub footer: &'a str,
    pub footer_kind: FooterKind,
    pub hover: Option<Hit>,
    /// Level history, newest last.
    pub waves: &'a [f32],
    pub can_trim: bool,
    pub editor: Option<EditorView<'a>>,
    pub shortcut_prompt: bool,
}

pub struct EditorView<'a> {
    pub name: &'a str,
    /// Peaks for the visible range (`view_start..view_end`).
    pub columns: &'a [f32],
    /// Peaks for the whole file (overview strip).
    pub overview: &'a [f32],
    pub duration: f64,
    pub view_start: f64,
    pub view_end: f64,
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

// ---- Main screen layout -------------------------------------------------

fn rec_card() -> Rect {
    rc(PAD, 66.0, ww() - PAD, 296.0 + extra())
}
fn wave_cy() -> f32 {
    118.0 + extra() / 2.0
}
fn wave_half() -> f32 {
    30.0 + extra() / 2.0 * 0.85
}
fn timer() -> Rect {
    rc(INNER, 156.0 + extra(), right(), 200.0 + extra())
}
fn timer_sub() -> Rect {
    rc(INNER, 200.0 + extra(), right(), 220.0 + extra())
}
fn record_btn() -> Rect {
    rc(INNER, 236.0 + extra(), right(), 276.0 + extra())
}

const ROW_H: f32 = 50.0;
const ROWS: usize = 6;

/// Bitrates offered for MP3 and M4A (kbps).
pub const BITRATES: [u32; 5] = [96, 128, 192, 256, 320];

fn settings_label_y() -> f32 {
    314.0 + extra()
}
fn set_card_top() -> f32 {
    334.0 + extra()
}
fn set_card() -> Rect {
    rc(PAD, set_card_top(), ww() - PAD, set_card_top() + ROW_H * ROWS as f32)
}
fn footer_rect() -> Rect {
    rc(PAD, set_card_top() + ROW_H * ROWS as f32 + 8.0, ww() - PAD, hh() - 6.0)
}

fn row_top(i: usize) -> f32 {
    set_card_top() + ROW_H * i as f32
}

fn control_rect(i: usize, l: f32, r: f32) -> Rect {
    let t = row_top(i) + 11.0;
    rc(l, t, r, t + 28.0)
}

fn hotkey_chip() -> Rect {
    control_rect(0, right() - 150.0, right())
}
fn folder_change() -> Rect {
    control_rect(1, right() - 150.0, right() - 70.0)
}
fn folder_open() -> Rect {
    control_rect(1, right() - 64.0, right())
}
fn format_seg() -> Rect {
    control_rect(2, right() - 156.0, right())
}
fn quality_seg() -> Rect {
    control_rect(3, right() - 200.0, right())
}

/// Splits a segmented control into `n` equal parts.
fn segments(r: &Rect, n: usize) -> Vec<Rect> {
    let w = (r.r - r.l) / n as f32;
    (0..n).map(|i| rc(r.l + w * i as f32, r.t, r.l + w * (i + 1) as f32, r.b)).collect()
}
fn toggle(i: usize) -> Rect {
    let t = row_top(i) + 15.0;
    rc(right() - 36.0, t, right(), t + 20.0)
}

/// Number of bars the main-screen waveform shows at the current width.
fn wave_bars() -> usize {
    (((right() - INNER) + 3.0) / 6.0) as usize
}

/// Finds which interactive element is under a point (in 96-DPI units).
pub fn hit_test(x: f32, y: f32, v: &View) -> Option<Hit> {
    let fmt = segments(&format_seg(), 3);
    let mut candidates = vec![
        (record_btn(), Hit::Record),
        (hotkey_chip(), Hit::Hotkey),
        (folder_change(), Hit::FolderChange),
        (folder_open(), Hit::FolderOpen),
        (rc(fmt[0].l, fmt[0].t, fmt[0].r, fmt[0].b), Hit::FormatWav),
        (rc(fmt[1].l, fmt[1].t, fmt[1].r, fmt[1].b), Hit::FormatMp3),
        (rc(fmt[2].l, fmt[2].t, fmt[2].r, fmt[2].b), Hit::FormatM4a),
        (rc(INNER, row_top(4), right(), row_top(5)), Hit::Tray),
        (rc(INNER, row_top(5), right(), row_top(6)), Hit::Startup),
    ];
    if v.format != 0 {
        for (i, r) in segments(&quality_seg(), BITRATES.len()).into_iter().enumerate() {
            candidates.push((r, Hit::Bitrate(i as u8)));
        }
    }
    for (r, h) in candidates {
        if r.contains(x, y) {
            let disabled = v.recording && matches!(h, Hit::FolderChange | Hit::FormatWav | Hit::FormatMp3 | Hit::FormatM4a | Hit::Bitrate(_));
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
        if footer_rect().contains(x, y) {
            return Some(Hit::Footer);
        }
    }
    None
}

// ---- Header / footer extras ----

fn pill_rect(recording: bool) -> Rect {
    let w = if recording { 112.0 } else { 82.0 };
    rc(ww() - PAD - w, 22.0, ww() - PAD, 48.0)
}

fn open_editor_chip(recording: bool) -> Rect {
    let p = pill_rect(recording);
    rc(p.l - 8.0 - 64.0, 22.0, p.l - 8.0, 48.0)
}

fn footer_trim_chip() -> Rect {
    let f = footer_rect();
    let cy = f.t + 13.0;
    rc(ww() - PAD - 56.0, cy - 13.0, ww() - PAD, cy + 13.0)
}

// ---- First-run shortcut prompt ----

fn prompt() -> Rect {
    let (cx, top) = (ww() / 2.0, (hh() - 210.0) / 2.0 - 40.0);
    rc(cx - 190.0, top, cx + 190.0, top + 210.0)
}
fn prompt_no() -> Rect {
    let p = prompt();
    rc(p.l + 24.0, p.b - 60.0, ww() / 2.0 - 5.0, p.b - 20.0)
}
fn prompt_yes() -> Rect {
    let p = prompt();
    rc(ww() / 2.0 + 5.0, p.b - 60.0, p.r - 24.0, p.b - 20.0)
}

pub fn hit_test_prompt(x: f32, y: f32) -> Option<Hit> {
    if prompt_yes().contains(x, y) {
        Some(Hit::ShortcutYes)
    } else if prompt_no().contains(x, y) {
        Some(Hit::ShortcutNo)
    } else {
        None
    }
}

// ---- Editor layout ----
// Bottom controls are anchored to the bottom of the window; the waveform
// card takes all remaining height.

const BACK: Rect = rc(PAD, 20.0, PAD + 30.0, 50.0);

fn bottom() -> f32 {
    hh() - 14.0
}
fn e_footer() -> Rect {
    rc(PAD, bottom() - 26.0, ww() - PAD, bottom())
}
fn e_save() -> Rect {
    rc(PAD, bottom() - 78.0, ww() / 2.0 - 4.0, bottom() - 36.0)
}
fn e_replace() -> Rect {
    rc(ww() / 2.0 + 4.0, bottom() - 78.0, ww() - PAD, bottom() - 36.0)
}
fn e_play_btn() -> Rect {
    rc(PAD, bottom() - 130.0, ww() - PAD, bottom() - 90.0)
}
fn e_trim_top() -> f32 {
    bottom() - 246.0
}
fn e_trim_card() -> Rect {
    rc(PAD, e_trim_top(), ww() - PAD, e_trim_top() + 100.0)
}
fn e_wave_card() -> Rect {
    rc(PAD, 66.0, ww() - PAD, e_trim_top() - 16.0)
}
/// The zoomable waveform.
fn e_wave() -> Rect {
    rc(INNER, 84.0, right(), e_wave_card().b - 100.0)
}
/// Whole-file overview strip with the visible range highlighted.
fn e_overview() -> Rect {
    let b = e_wave_card().b;
    rc(INNER, b - 86.0, right(), b - 64.0)
}
fn e_labels() -> Rect {
    let b = e_wave_card().b;
    rc(INNER, b - 58.0, right(), b - 40.0)
}
fn e_hint() -> Rect {
    let b = e_wave_card().b;
    rc(INNER, b - 34.0, right(), b - 16.0)
}

fn e_row(i: usize) -> f32 {
    e_trim_top() + ROW_H * i as f32
}
fn e_ctrl(i: usize, l: f32, r: f32) -> Rect {
    let t = e_row(i) + 11.0;
    rc(l, t, r, t + 28.0)
}
fn e_play(i: usize) -> Rect {
    e_ctrl(i, right() - 36.0, right())
}
fn e_plus(i: usize) -> Rect {
    e_ctrl(i, right() - 74.0, right() - 44.0)
}
fn e_minus(i: usize) -> Rect {
    e_ctrl(i, right() - 108.0, right() - 78.0)
}
fn e_time(i: usize) -> Rect {
    e_ctrl(i, right() - 230.0, right() - 116.0)
}

/// Number of waveform columns at the current width (one per 2 units).
pub fn edit_cols() -> usize {
    ((right() - INNER) / 2.0) as usize
}

/// Time -> x inside the zoomable waveform.
pub fn wave_x(t: f64, a: f64, b: f64) -> f32 {
    if b <= a {
        return INNER;
    }
    INNER + (((t - a) / (b - a)) as f32) * (right() - INNER)
}

/// x inside the zoomable waveform -> time.
pub fn wave_time(x: f32, a: f64, b: f64) -> f64 {
    a + (((x - INNER) / (right() - INNER)).clamp(0.0, 1.0) as f64) * (b - a)
}

/// x inside the overview strip -> time.
pub fn overview_time(x: f32, dur: f64) -> f64 {
    wave_time(x, 0.0, dur)
}

pub fn hit_test_editor(x: f32, y: f32, e: &EditorView) -> Option<Hit> {
    let w = e_wave();
    let o = e_overview();
    let mut candidates = vec![
        (BACK, Hit::Back),
        (rc(w.l - 8.0, w.t - 6.0, w.r + 8.0, w.b + 6.0), Hit::Wave),
        (rc(o.l, o.t - 4.0, o.r, o.b + 4.0), Hit::Overview),
        (e_minus(0), Hit::StartMinus),
        (e_plus(0), Hit::StartPlus),
        (e_play(0), Hit::PlayStart),
        (e_minus(1), Hit::EndMinus),
        (e_plus(1), Hit::EndPlus),
        (e_play(1), Hit::PlayEnd),
        (e_play_btn(), Hit::PlayPause),
    ];
    if e.loading.is_none() {
        candidates.push((e_save(), Hit::SaveCopy));
        candidates.push((e_replace(), Hit::Replace));
    }
    candidates.into_iter().find(|(r, _)| r.contains(x, y)).map(|(_, h)| h)
}

/// "mm:ss.cc" (or "h:mm:ss.cc" for long files).
pub fn fmt_precise(t: f64) -> String {
    let t = t.max(0.0);
    let cs = (t * 100.0).round() as u64;
    let (s, c) = (cs / 100, cs % 100);
    if s >= 3600 {
        format!("{}:{:02}:{:02}.{:02}", s / 3600, s / 60 % 60, s % 60, c)
    } else {
        format!("{:02}:{:02}.{:02}", s / 60, s % 60, c)
    }
}

fn fmt_secs(t: f64) -> String {
    if t < 60.0 {
        format!("{:.2} s", t)
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
            let lp = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: v2(INNER, 0.0), endPoint: v2(right(), 0.0) };
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
            // The gradient spans the content width, which follows the window.
            t.wave.SetStartPoint(v2(INNER, 0.0));
            t.wave.SetEndPoint(v2(right(), 0.0));
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
        t.rt.FillRectangle(&rc(0.0, 0.0, ww(), hh()).d2d(), &t.brush);

        let p = prompt();
        self.fill(t, &p, 14.0, color(CARD, 1.0));
        self.stroke(t, &p, 14.0, color(CONTROL_BORDER, 1.0));

        // App logo tile, as on the desktop shortcut.
        let cx = ww() / 2.0;
        let logo = rc(cx - 24.0, p.t + 24.0, cx + 24.0, p.t + 72.0);
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

        self.text(t, "Add SAR to your desktop?", &self.title, &rc(p.l, p.t + 84.0, p.r, p.t + 110.0), color(TEXT, 1.0), Align::Center);
        self.text(t, "One click to open the recorder.", &self.small, &rc(p.l, p.t + 110.0, p.r, p.t + 128.0), color(MUTED, 1.0), Align::Center);
        self.text(t, "Also adds it to the Start menu.", &self.small, &rc(p.l, p.t + 127.0, p.r, p.t + 145.0), color(MUTED, 1.0), Align::Center);

        let hov = |h: Hit| v.hover == Some(h);
        let (no, yes) = (prompt_no(), prompt_yes());
        self.fill(t, &no, 10.0, color(if hov(Hit::ShortcutNo) { CONTROL_HOVER } else { CONTROL }, 1.0));
        self.stroke(t, &no, 10.0, color(CONTROL_BORDER, 1.0));
        self.text(t, "No thanks", &self.button, &no, color(TEXT, 1.0), Align::Center);
        self.fill(t, &yes, 10.0, color(if hov(Hit::ShortcutYes) { ACCENT_HOVER } else { ACCENT }, 1.0));
        self.text(t, "Add shortcut", &self.button, &yes, color(0xFFFFFF, 1.0), Align::Center);
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
        let (va, vb) = (e.view_start, e.view_end);

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
        self.card(t, &e_wave_card());
        let wv = e_wave();
        let cy = (wv.t + wv.b) / 2.0;
        let half = (wv.b - wv.t) / 2.0;
        let xs = wave_x(e.start, va, vb);
        let xe = wave_x(e.end, va, vb);
        // Everything inside the waveform is clipped to it (handles may be off-screen when zoomed).
        t.rt.PushAxisAlignedClip(&rc(wv.l - 7.0, wv.t - 5.0, wv.r + 7.0, wv.b + 5.0).d2d(), D2D1_ANTIALIAS_MODE_ALIASED);
        t.brush.SetColor(&color(ACCENT, 0.07));
        t.rt.FillRectangle(&rc(xs.max(wv.l), wv.t, xe.min(wv.r), wv.b).d2d(), &t.brush);
        let step = (wv.r - wv.l) / e.columns.len().max(1) as f32;
        for (i, &p) in e.columns.iter().enumerate() {
            let x = wv.l + i as f32 * step;
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
            if x < wv.l - 7.0 || x > wv.r + 7.0 {
                continue;
            }
            let active = is_start == e.active_start;
            let c = if active { color(0xFFFFFF, 1.0) } else { color(0xB4B4BC, 1.0) };
            t.brush.SetColor(&c);
            t.rt.FillRectangle(&rc(x - 1.0, wv.t - 4.0, x + 1.0, wv.b + 4.0).d2d(), &t.brush);
            let grip = rc(x - 6.0, cy - 15.0, x + 6.0, cy + 15.0);
            self.fill(t, &grip, 5.0, c);
            t.brush.SetColor(&color(0x3A3A44, 1.0));
            for dy in [-4.0f32, 0.0, 4.0] {
                t.rt.FillRectangle(&rc(x - 3.0, cy + dy - 0.6, x + 3.0, cy + dy + 0.6).d2d(), &t.brush);
            }
        }
        // Playhead
        if e.playing || (e.playhead > e.start + 0.01 && e.playhead < e.end - 0.01) {
            let x = wave_x(e.playhead, va, vb);
            t.brush.SetColor(&color(0xFFFFFF, 0.9));
            t.rt.FillRectangle(&rc(x - 0.75, wv.t, x + 0.75, wv.b).d2d(), &t.brush);
            self.circle(t, x, wv.t, 3.5, color(0xFFFFFF, 1.0));
        }
        t.rt.PopAxisAlignedClip();

        // Overview strip: the whole file, with the visible part highlighted.
        let ov = e_overview();
        self.fill(t, &ov, 4.0, color(0x18181D, 1.0));
        let ocy = (ov.t + ov.b) / 2.0;
        let ostep = (ov.r - ov.l) / e.overview.len().max(1) as f32;
        // Scale to the loudest part so quiet recordings still show a shape.
        let omax = e.overview.iter().copied().fold(0.0f32, f32::max).max(0.02);
        let (oxs, oxe) = (ov.l + (e.start / e.duration.max(1e-9)) as f32 * (ov.r - ov.l), ov.l + (e.end / e.duration.max(1e-9)) as f32 * (ov.r - ov.l));
        for (i, &p) in e.overview.iter().enumerate() {
            let x = ov.l + i as f32 * ostep;
            let h = 1.0 + (p / omax).powf(0.7) * (ov.b - ov.t - 6.0);
            let inside = x >= oxs && x <= oxe;
            t.brush.SetColor(&color(if inside { 0x6C6CF7 } else { 0x34343C }, if inside { 0.75 } else { 1.0 }));
            t.rt.FillRectangle(&rc(x, ocy - h / 2.0, x + ostep * 0.8, ocy + h / 2.0).d2d(), &t.brush);
        }
        let zoomed = vb - va < e.duration - 1e-6;
        if zoomed {
            let vx1 = ov.l + (va / e.duration.max(1e-9)) as f32 * (ov.r - ov.l);
            let vx2 = (ov.l + (vb / e.duration.max(1e-9)) as f32 * (ov.r - ov.l)).max(vx1 + 4.0);
            let win = rc(vx1, ov.t - 2.0, vx2, ov.b + 2.0);
            self.fill(t, &win, 4.0, color(0xFFFFFF, if hov(Hit::Overview) { 0.14 } else { 0.08 }));
            self.stroke(t, &win, 4.0, color(0xFFFFFF, 0.55));
        }

        // Labels under the waveform: visible range + selection length
        let lab = e_labels();
        self.text(t, &fmt_precise(va), &self.small, &lab, color(FAINT, 1.0), Align::Left);
        self.text(t, &fmt_precise(vb), &self.small, &lab, color(FAINT, 1.0), Align::Right);
        let len = format!("Length  {}", fmt_precise(e.end - e.start));
        self.text(t, &len, &self.body_bold, &lab, color(TEXT, 1.0), Align::Center);
        let hint = match e.loading {
            Some(p) => format!("Loading waveform\u{2026}  {}%", (p * 100.0) as u32),
            None if zoomed => {
                let zoom = e.duration / (vb - va).max(1e-9);
                format!("Zoom {:.0}\u{00D7}  \u{00B7}  Scroll to zoom  \u{00B7}  Shift + scroll to move  \u{00B7}  Drag the strip", zoom)
            }
            None => "Drag the handles to cut  \u{00B7}  Scroll to zoom  \u{00B7}  Space to play".to_string(),
        };
        self.text(t, &hint, &self.small, &e_hint(), muted, Align::Center);

        // Start / End rows
        self.card(t, &e_trim_card());
        t.brush.SetColor(&color(DIVIDER, 1.0));
        t.rt.FillRectangle(&rc(INNER, e_row(1), right(), e_row(1) + 1.0).d2d(), &t.brush);
        let rows = [
            ("Start", e.start, e.start, Hit::StartMinus, Hit::StartPlus, Hit::PlayStart, e.active_start),
            ("End", e.end, e.duration - e.end, Hit::EndMinus, Hit::EndPlus, Hit::PlayEnd, !e.active_start),
        ];
        for (i, (label, at, cut, minus, plus, play, active)) in rows.into_iter().enumerate() {
            let top = e_row(i);
            let lc = if active { color(ACCENT_HOVER, 1.0) } else { color(TEXT, 1.0) };
            self.text(t, label, &self.body_bold, &rc(INNER, top + 7.0, e_time(i).l - 6.0, top + 26.0), lc, Align::Left);
            let sub = if cut >= 0.005 { format!("Cuts {}", fmt_secs(cut)) } else { "Nothing cut".to_string() };
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
        let pb = e_play_btn();
        self.fill(t, &pb, 10.0, color(if hov(Hit::PlayPause) { CONTROL_HOVER } else { CONTROL }, 1.0));
        self.stroke(t, &pb, 10.0, color(CONTROL_BORDER, 1.0));
        let label = if e.playing { "Pause" } else { "Play selection" };
        let tw = self.text_width(label, &self.button);
        let (cx, cy) = ((pb.l + pb.r) / 2.0, (pb.t + pb.b) / 2.0);
        let ix = cx - (tw + 20.0) / 2.0;
        if e.playing {
            self.pause_icon(t, ix + 5.0, cy, 11.0, color(TEXT, 1.0));
        } else {
            self.play_icon(t, ix + 5.0, cy, 11.0, color(TEXT, 1.0));
        }
        self.text(t, label, &self.button, &rc(ix + 20.0, pb.t, pb.r, pb.b), color(TEXT, 1.0), Align::Left);

        // Save buttons
        let a = if e.loading.is_none() { 1.0 } else { 0.45 };
        let (sv, rp) = (e_save(), e_replace());
        self.fill(t, &sv, 10.0, color(if hov(Hit::SaveCopy) { ACCENT_HOVER } else { ACCENT }, a));
        self.text(t, "Save as copy", &self.button, &sv, color(0xFFFFFF, a), Align::Center);
        self.fill(t, &rp, 10.0, color(if hov(Hit::Replace) { CONTROL_HOVER } else { CONTROL }, a));
        self.stroke(t, &rp, 10.0, color(CONTROL_BORDER, a));
        self.text(t, "Replace original", &self.button, &rp, color(TEXT, a), Align::Center);

        // Footer
        let c = match v.footer_kind {
            FooterKind::None => None,
            FooterKind::Success => Some(color(GREEN, 1.0)),
            FooterKind::Error => Some(color(RED, 1.0)),
            FooterKind::Info => Some(muted),
        };
        if let Some(c) = c {
            self.text(t, v.footer, &self.small, &e_footer(), c, Align::Center);
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
        self.card(t, &rec_card());

        // Waveform: as many bars as fit; newest level on the right.
        let bars = wave_bars();
        let bar_w = 3.0;
        let gap = 3.0;
        let total = bars as f32 * (bar_w + gap) - gap;
        let x0 = INNER + ((right() - INNER) - total) / 2.0;
        let (wcy, whalf) = (wave_cy(), wave_half());
        if !v.recording {
            t.brush.SetColor(&color(0x26262E, 1.0));
        }
        let skip = v.waves.len().saturating_sub(bars);
        let shown = &v.waves[skip..];
        let pad = bars - shown.len();
        for i in 0..bars {
            let h = if v.recording {
                let lvl = if i < pad { 0.0 } else { shown[i - pad] };
                3.0 + lvl * (whalf * 2.0 - 3.0)
            } else {
                // Calm static wave while idle: a soft bell-shaped ripple.
                let p = i as f32 / (bars - 1).max(1) as f32;
                let bell = (p * std::f32::consts::PI).sin().powf(1.5);
                let ripple = 0.55 + 0.45 * (p * 23.0).sin() * (p * 7.0 + 1.0).cos();
                3.0 + (22.0 + extra() * 0.3) * bell * ripple.abs()
            };
            let x = x0 + i as f32 * (bar_w + gap);
            let bar = rc(x, wcy - h / 2.0, x + bar_w, wcy + h / 2.0);
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
        self.text(t, &time, &self.timer, &timer(), tc, Align::Center);
        let sub = if v.recording {
            "Recording system audio  \u{00B7}  microphone is never recorded".to_string()
        } else if v.hotkey_ok {
            format!("Press {} anywhere to start", v.hotkey)
        } else {
            "Set a hotkey below to record from anywhere".to_string()
        };
        self.text(t, &sub, &self.small, &timer_sub(), color(MUTED, 1.0), Align::Center);

        // Big button
        let btn = record_btn();
        let hover = v.hover == Some(Hit::Record);
        let (fill, label) = if v.recording {
            (if hover { RED_HOVER } else { RED_FILL }, "Stop Recording")
        } else {
            (if hover { ACCENT_HOVER } else { ACCENT }, "Start Recording")
        };
        self.fill(t, &btn, 10.0, color(fill, 1.0));
        let tw = self.text_width(label, &self.button);
        let cx = (btn.l + btn.r) / 2.0;
        let cy = (btn.t + btn.b) / 2.0;
        let icon_x = cx - (tw + 18.0) / 2.0;
        if v.recording {
            self.fill(t, &rc(icon_x, cy - 4.5, icon_x + 9.0, cy + 4.5), 2.0, color(0xFFFFFF, 1.0));
        } else {
            self.circle(t, icon_x + 4.5, cy, 4.5, color(0xFFFFFF, 1.0));
        }
        self.text(t, label, &self.button, &rc(icon_x + 18.0, btn.t, btn.r, btn.b), color(0xFFFFFF, 1.0), Align::Left);
    }

    unsafe fn row_labels(&self, t: &Target, i: usize, label: &str, sub: &str, sub_color: D2D1_COLOR_F, right_limit: f32) {
        let top = row_top(i);
        self.text(t, label, &self.body_bold, &rc(INNER, top + 7.0, right_limit, top + 26.0), color(TEXT, 1.0), Align::Left);
        self.text(t, sub, &self.small, &rc(INNER, top + 26.0, right_limit, top + 43.0), sub_color, Align::Left);
    }

    unsafe fn settings(&self, t: &Target, v: &View) {
        let ly = settings_label_y();
        self.text(t, "SETTINGS", &self.caps, &rc(PAD + 2.0, ly - 8.0, 200.0, ly + 8.0), color(MUTED, 1.0), Align::Left);
        self.card(t, &set_card());
        for i in 1..ROWS {
            t.brush.SetColor(&color(DIVIDER, 1.0));
            t.rt.FillRectangle(&rc(INNER, row_top(i), right(), row_top(i) + 1.0).d2d(), &t.brush);
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
        let sub = match v.format {
            1 => "Plays everywhere",
            2 => "Smallest files for the same quality",
            _ => "Lossless, largest files",
        };
        self.row_labels(t, 2, "Format", sub, muted, seg.l - 10.0);
        let items = [("WAV", Hit::FormatWav), ("MP3", Hit::FormatMp3), ("M4A", Hit::FormatM4a)];
        let fmt: Vec<_> = items.iter().enumerate().map(|(i, (l, h))| (*l, i as u8 == v.format, *h)).collect();
        self.segmented(t, &seg, &fmt, v.hover, !v.recording);

        // 3: quality
        let seg = quality_seg();
        let sub = if v.format == 0 {
            "Lossless  \u{00B7}  ~690 MB/hour".to_string()
        } else {
            let mb = (v.kbps as f32 * 0.45).round() as u32;
            if v.kbps == 192 { format!("Recommended  \u{00B7}  ~{mb} MB/hour") } else { format!("~{mb} MB per hour") }
        };
        self.row_labels(t, 3, "Quality", &sub, muted, seg.l - 10.0);
        let labels: Vec<String> = BITRATES.iter().map(|b| b.to_string()).collect();
        let q: Vec<_> = labels
            .iter()
            .enumerate()
            .map(|(i, l)| (l.as_str(), v.format != 0 && BITRATES[i] == v.kbps, Hit::Bitrate(i as u8)))
            .collect();
        self.segmented(t, &seg, &q, v.hover, !v.recording && v.format != 0);

        // 4, 5: toggles
        self.row_labels(t, 4, "Keep running in tray", "Closing the window keeps the hotkey active", muted, right() - 50.0);
        self.toggle(t, &toggle(4), v.tray, v.hover == Some(Hit::Tray));
        self.row_labels(t, 5, "Start with Windows", "Launches quietly in the tray", muted, right() - 50.0);
        self.toggle(t, &toggle(5), v.startup, v.hover == Some(Hit::Startup));
    }

    /// Pill-shaped segmented control: (label, selected, hit) per segment.
    unsafe fn segmented(&self, t: &Target, r: &Rect, items: &[(&str, bool, Hit)], hover: Option<Hit>, enabled: bool) {
        let alpha = if enabled { 1.0 } else { 0.4 };
        self.fill(t, r, 7.0, color(CONTROL, alpha));
        self.stroke(t, r, 7.0, color(CONTROL_BORDER, alpha));
        for (part, (label, selected, hit)) in segments(r, items.len()).iter().zip(items) {
            let inner = rc(part.l + 3.0, part.t + 3.0, part.r - 3.0, part.b - 3.0);
            if *selected {
                self.fill(t, &inner, 5.0, color(0x30303A, alpha));
            } else if enabled && hover == Some(*hit) {
                self.fill(t, &inner, 5.0, color(CONTROL_HOVER, alpha));
            }
            let c = if *selected { color(TEXT, alpha) } else { color(MUTED, alpha) };
            self.text(t, label, &self.body_bold, part, c, Align::Center);
        }
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
        let f = footer_rect();
        let line = rc(f.l, f.t, f.r, f.t + 26.0);
        if v.footer_kind == FooterKind::Success && v.can_trim {
            let chip = footer_trim_chip();
            self.text(t, v.footer, &self.small, &rc(line.l, line.t, chip.l - 8.0, line.b), c, Align::Left);
            self.chip_button(t, &chip, "Trim", v.hover == Some(Hit::FooterTrim), true);
        } else {
            self.text(t, v.footer, &self.small, &line, c, Align::Center);
        }
    }
}

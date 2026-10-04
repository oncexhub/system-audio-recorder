<div align="center">

<img src="docs/logo.png" width="96" alt="SAR logo">

# System Audio Recorder

**Record what your PC plays. Never your microphone.**<br>
One hotkey to start, the same hotkey to stop. Tiny, native and free.

[![Download](https://img.shields.io/badge/Download-SystemAudioRecorder.exe-5B5BF0?style=for-the-badge&logo=windows&logoColor=white)](https://github.com/oncexhub/system-audio-recorder/releases/latest/download/SystemAudioRecorder.exe)

![Release](https://img.shields.io/github/v/release/oncexhub/system-audio-recorder?style=flat-square&color=5B5BF0&label=version)
![Windows](https://img.shields.io/badge/Windows-10%20%7C%2011-3B82F6?style=flat-square)
![Size](https://img.shields.io/badge/size-~0.5%20MB-8B5CF6?style=flat-square)
![License](https://img.shields.io/github/license/oncexhub/system-audio-recorder?style=flat-square&color=4ADE80)

<br>

<img src="docs/demo.gif" width="476" alt="Recording with live sound waves">

</div>

## Features

- 🎧 **System audio only.** Records exactly what you hear (browser, Spotify, games, Discord), never the microphone.
- ⌨️ **One global hotkey.** `Ctrl + Alt + R` starts and stops from anywhere, even in a game. You can change it.
- ✂️ **Built-in trimming.** Cut off a late start or an early stop in seconds, with no quality loss.
- ⏱️ **Long recordings.** No length limit, and files stay playable even after a crash.
- 🪶 **Tiny and light.** About 0.5 MB, no installer, about 1% CPU. Runs quietly in the tray.

## Screenshots

<div align="center">
<table>
<tr>
<td align="center"><img src="docs/main.png" width="300" alt="Main window"><br><sub><b>Ready to record</b></sub></td>
<td align="center"><img src="docs/trim.png" width="300" alt="Trim editor"><br><sub><b>Trim the start and end</b></sub></td>
</tr>
</table>
</div>

## Download

1. **[Download `SystemAudioRecorder.exe`](https://github.com/oncexhub/system-audio-recorder/releases/latest/download/SystemAudioRecorder.exe)**. You can also get `SAR-windows.zip` from the [Releases](../../releases/latest) page.
2. Put it anywhere you like and double-click it. You don't need to install anything.
3. On first start, the app asks once whether to add a **SAR** shortcut to your desktop and Start menu.

> [!NOTE]
> The green **Code → Download ZIP** button only contains the source code, not the app.

> [!TIP]
> **"Windows protected your PC"?** The app isn't signed with a paid certificate, so SmartScreen may warn the first time. Click **More info → Run anyway**. You only need to do this once.

**Requirements:** Windows 10 or 11 (64-bit). You don't need Rust or any other runtime.

## How to use

| | |
|---|---|
| **Record** | Press `Ctrl + Alt + R` anywhere. Press it again to stop. The file is saved automatically. |
| **Find your files** | `Music\Recordings`, named `Recording YYYY-MM-DD HH-MM-SS`. Click **Open** in the app. |
| **Change the hotkey** | Click the hotkey in the app and press a new combination. Press Esc to cancel. |
| **Tray** | Closing the window keeps the app in the tray. The icon turns red while recording. Right-click it for options or **Exit**. |

The app records whatever device Windows is playing through: speakers, headphones and so on. If you switch devices mid-recording, it simply carries on.

## Trimming

Started a little too early or stopped too late? Cut it off:

1. Click **Trim** after a recording, or click **TRIM** at the top and pick a file. You can also drag a `.wav` or `.mp3` onto the window.
2. Drag the two handles. Fine-tune with **− / +** or the arrow keys (0.1 s, or 1 s with Shift).
3. Press **▶** to check the start or the end, and Space to play the selection.
4. Click **Save as copy** to keep the original, or **Replace original** to overwrite it (the app asks first).

Cutting never re-encodes, so quality stays identical and even hours-long recordings save in seconds.

## Formats

| Format | Quality | Size |
|---|---|---|
| **WAV** (default) | Lossless, 48 kHz / 16-bit stereo | about 690 MB per hour |
| **MP3** | 320 kbps | about 140 MB per hour |

## Settings and uninstalling

Settings are stored in `%APPDATA%\SystemAudioRecorder\settings.ini`. To remove the app:
1. Turn off **Start with Windows** in the app, if you turned it on.
2. Exit the app from the tray.
3. Delete the `.exe`, the SAR shortcuts and the settings folder.

<details>
<summary><b>How it works</b></summary>

- WASAPI **loopback** on the default playback device captures exactly what you hear.
- A silent playback stream keeps the audio engine running, so silences stay in the recording and its length is exact.
- Audio is streamed straight to disk. WAV headers are refreshed every few seconds, and files over 4 GB become RF64.
- MP3 encoding uses the encoder built into Windows (Media Foundation).
- The UI is drawn with Direct2D, which is also built into Windows. There is no web engine and no framework.

</details>

<details>
<summary><b>Building from source</b></summary>

Only needed if you want to change the code. Install [Rust](https://rustup.rs) and the Visual Studio Build Tools ("Desktop development with C++"), then run:

```
cargo build --release
```

The output is `target\release\SystemAudioRecorder.exe`. Pushing a tag such as `v1.2.0` makes GitHub Actions build the exe and the zip and publish them as a release.

</details>

## License

[MIT](LICENSE)

# System Audio Recorder

A tiny native Windows app (about 0.5 MB, no Electron, no installer) that records **only what your PC plays**: browser, Spotify, games, Discord and so on. It **never** records your microphone.

Start and stop recording with one global hotkey, from anywhere, even in a game.

## Download
1. Go to [**Releases**](../../releases/latest) and download `SystemAudioRecorder.exe`.
2. Put it anywhere you like, for example in a `Tools` folder, and double-click it. You don't need to install anything.
3. On first start, the app asks once whether to add a **SAR** shortcut to your desktop and Start menu.

**Requirements:** Windows 10 or 11 (64-bit). You don't need Rust or any other runtime.

> **"Windows protected your PC"?** The app isn't signed with a paid certificate, so SmartScreen may warn the first time. Click **More info → Run anyway**. You only need to do this once.

## Usage
1. Run `SystemAudioRecorder.exe`.
2. Press **Ctrl + Alt + R** anywhere to start recording, and press it again to stop. Click the hotkey in the app to choose a different one.
3. Recordings are saved automatically to your `Music\Recordings` folder, named `Recording YYYY-MM-DD HH-MM-SS`. You can change the folder in the app.

Closing the window keeps the app in the tray, where the icon turns red while recording. Right-click the tray icon to start/stop, open the folder or exit.

The app records whatever device Windows is currently playing through (speakers, headphones, ...). Nothing needs to be set up.

## Trimming
Cut off the start and/or end of a recording, for example when you started a little too early or stopped too late.

- Open a file in one of these ways:
  - Click **Trim** next to the "Saved" message after recording.
  - Click **TRIM** at the top and pick a file.
  - Drag a `.wav` or `.mp3` file onto the window.
- Drag the two handles, or fine-tune with **− / +** or the arrow keys (0.1 s, or 1 s with Shift). Tab switches handles.
- The ▶ buttons preview the start, or the last 3 seconds before the end. Space plays or pauses the selection.
- **Save as copy** writes `name (trimmed).wav`. **Replace original** overwrites the file after asking first.
- Cutting never re-encodes, so quality stays identical and even hours-long recordings save in seconds.

## Formats
- **WAV**: lossless, 48 kHz / 16-bit stereo, about 690 MB per hour. There is no length limit (files above 4 GB automatically become RF64). The file stays playable even after a crash or power loss.
- **MP3**: 320 kbps, about 140 MB per hour, using the encoder built into Windows.

## Settings and uninstalling
Settings are stored in `%APPDATA%\SystemAudioRecorder\settings.ini`. To remove the app:
1. Turn off **Start with Windows** in the app, if you turned it on.
2. Exit the app from the tray.
3. Delete the `.exe` and the folder above.

## How it works
- WASAPI **loopback** on the default playback device captures exactly what you hear.
- A silent playback stream keeps the audio engine running, so silences stay in the recording and its length is exact.
- If you switch speakers or headphones mid-recording, recording carries on with the new device.
- Audio is streamed straight to disk. The UI is drawn with Direct2D (built into Windows): about 1% CPU while the window is open, close to 0% in the tray.

## Building from source
Only needed if you want to change the code. Install [Rust](https://rustup.rs) and the Visual Studio Build Tools ("Desktop development with C++"), then run:
```
cargo build --release
```
The output is `target\release\SystemAudioRecorder.exe`.

Pushing a tag such as `v1.0.0` makes GitHub Actions build the exe and attach it to a new release automatically.

## License
[MIT](LICENSE)

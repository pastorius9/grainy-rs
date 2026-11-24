# Grainy RS: what's new

The notes Grainy RS shows the first time it opens after an update (Korean: ko.md). Each new version adds a `## version — title` section to this file and to ko.md. The GitHub release notes are in docs/releases.

## 0.5.3 — No more quitting over the window size

- **Grainy RS no longer quits when another program makes its window very large.** A script or a window manager or screen capture tool that made the window larger than the graphics card can draw (usually 16384 to 32768 pixels) or larger than all the monitors together closed Grainy RS without a message. Now the window goes back to the size of its monitor.
- After a moment the window could not be drawn, it draws again instead of staying dark.

## 0.5.2 — Import the Python Grainy's library

- **Grainy continues as Grainy RS.** The Python Grainy ends with 0.6.9; new features and fixes come only in Grainy RS.
- **The Python Grainy's library comes over.** On the first start with an empty library, Grainy RS offers once to bring over the Python Grainy's library (photos, edits, history, collections, thumbnails, camera profiles). The Python library is only read, never changed.
  - File management › Import the Python Grainy Library… does it at any time. A library that already has photos is asked about and backed up first (File management › Restore catalog brings it back).
- **AI connection:** the Claude Code and Codex install lines replace an earlier grainy entry (such as the Python Grainy's) with Grainy RS.
- After an update, Grainy RS shows its own notes instead of the Python Grainy's.

## 0.5.1 — Auto Color strength slider, model choice in Ask AI

- **The Auto color strength slider is back.** After Auto Color (Ctrl+Shift+U), an Auto color strength slider (0–100%) appears right under the Basic panel's button row.
  - The colours change live while you drag; in its number box, Up/Down (Shift: 10×) fine-tune it.
  - The strength you pick is used for other photos and batch runs too, and one Undo takes it back.
- **Ask AI lets you pick the model.** Each program (Claude Code, Codex, Gemini CLI) has a list of recommended models; you can also type a model ID or use the program's default. Switching to another AI program clears a model that does not fit it.
- Settings → AI Connection (MCP)… has a new Ask AI (Sign In · Model)… button, so sign-in and model choice are one click away.

## 0.5.0 — The whole edit on the GPU, fast RAW import, AI Denoise, multi-selection

- **Every step of an edit now runs on the graphics card (GPU):** all 82 film looks with halation, the display rendering transform (DRT), the photo filter, local adjustments, and straighten, perspective, lens distortion and crop. Sliders respond faster.
- **RAW files import quickly.** Unedited RAWs get their thumbnail from the preview the camera stored, several thumbnails are made at once, and unused thumbnails are cleaned up.
- **Batch export is faster:** up to 8 photos at once, developed on the GPU.
- **New**
  - View / filters → Soft Proofing: preview with an output colour profile and mark the colours it cannot show (Black Point Compensation optional).
  - Color Grading wheels: shadows, midtones and highlights with wheels and number sliders side by side.
  - An 8-colour quick swatch row in the Color Mixer, and the R/G/B % under the pointer in the histogram.
  - The second screen opens as a real window of its own.
  - Apple ProRAW (DNG 1.6) gain maps and monochrome-sensor RAWs (Leica M Monochrom and others).
  - AI Denoise in the Detail panel, run on this computer's GPU without the internet.
- **Familiar controls:** Up/Down in any slider's number box (Enter applies, Esc cancels). In the filmstrip and the library grid, Shift- and Ctrl(⌘)-click, Shift+arrow keys, Ctrl+A (select all) and Ctrl+D (select none) pick several photos.
- Many fixes across the GPU, RAW, colour management, file formats and the interface, among them a graphics-driver stop on narrow crops and a possible quit on macOS 11.

## 0.4.1 — Grainy's own icon in the taskbar and the Dock

- Fixed: the Windows taskbar, title bar and Alt+Tab, and the macOS Dock, showed a generic icon instead of Grainy's Gr icon.
- Fixed: after an update from inside the app, Windows Explorer kept showing the old icon.

## 0.4.0 — Much faster previews, Process 4, large libraries, better AI selection

- **Sliders respond much faster.** The preview is drawn straight from the graphics card's memory, and the monitor's colour profile is applied on the GPU. On an RTX 3070 with a Display P3 monitor, a 4000×2667 photo went from 414 ms to 11 ms.
- **At 1:1, only the visible part is computed.** On a 45 MP photo, a 1:1 slider drag went from 930 ms to under 10 ms.
- **Process 4 · Grainy RS native**, chosen in the Calibration panel: clipped-highlight recovery, a wider highlights and shadows range, halo-free clarity, texture, dehaze and noise reduction. Edits made with Process 3 stay as they are.
- **Large libraries are faster:** indexing and searching 100,000 photos went from 73 s to 1.5 s, and switching folders is much quicker.
- **Better AI selection:** finer edges on hair and fur, and a new Portrait Skin selection. Windows uses the graphics card or NPU; macOS uses its built-in features with nothing to download.
- **HDR monitor window on macOS** for HDR screens such as the MacBook Pro XDR.
- More reliable colour management, shooting details kept on export (Canon CR3, Fujifilm RAF), faster AVIF export and sturdier updates.

## 0.3.0 — RAW Basic panel, panorama and HDR merge, AVIF and JPEG XL, faster X-Trans

- The RAW Basic panel's 15 sliders fit on a 1920×1080 Windows screen at 125% scaling. Arrow keys move a slider finely, with Shift in larger steps.
- After cropping or rotating, the fitted view is sharp enough to judge detail.
- Large libraries search quickly by file name or keyword. If moving photos to the recycle bin stops partway, the library sorts out the originals and their virtual copies the next time it opens.
- Merge overlapping photos into a flat panorama, or bracketed exposures into HDR. The result is saved as a new 32-bit TIFF.
- Open AVIF and JPEG XL files for further editing; JPEG XL exports are 16-bit lossless. HDR export (32-bit TIFF, 10-bit PQ AVIF) is available.
- On a Windows HDR monitor, a separate HDR preview window shows the parts brighter than SDR white.
- Fujifilm X-Trans RAWs are developed on several CPU cores (X-T3 demosaicing 16.4 s → 4.8 s); JPEG export and colour noise reduction are faster too.
- Subject and background selections. Tethered capture needs a camera and driver with remote shutter support.

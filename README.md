# Grainy RS

Grainy RS is the Rust rewrite of [Grainy](https://github.com/pastorius9/grainy), a photo editor for film scans
and digital photos. Shared development stages are tested against reference data from Python Grainy 0.6.8.

**This is an early release.** The Python Grainy at <https://github.com/pastorius9/grainy> stays the main product
for now; use Grainy RS to try it out and report what you find.

## Download and run

### Windows

1. Download `GrainyRS-<version>-windows-x64.zip` from the
   [releases](https://github.com/pastorius9/grainy-rs/releases) page.
2. Unzip the whole `GrainyRS` folder anywhere and start `GrainyRS.exe`. Nothing is installed, and the Microsoft
   Visual C++ Redistributable is **not** needed: the C runtime is built into the program, and the one Microsoft
   file it uses (`vcomp140.dll`) is in the folder. Keep the files of the folder together.

`grainy-cli.exe` in the same folder renders a photo from the command line with the same pixels as an export
(`grainy-cli.exe --help`). Windows 10 or 11, 64-bit.

### macOS

1. Download `GrainyRS-<version>-macos-universal.dmg` from the
   [releases](https://github.com/pastorius9/grainy-rs/releases) page. It runs natively on Apple Silicon and on
   Intel Macs, macOS 11 or newer.
2. Open the disk image and drag **Grainy RS** to the Applications folder.
3. First start: the app is signed with a self-signed certificate and is not notarized by Apple, so macOS blocks
   it once. Right-click (or Control-click) Grainy RS in the Applications folder, choose **Open**, then **Open**
   again. Or try to open it, then go to System Settings > Privacy & Security and press **Open Anyway**.
   After that it starts normally, and updates keep the same signature, so macOS keeps the permissions it gave
   the app.

The data folder is `~/Library/Application Support/GrainyRS` (separate from the Python Grainy's). The
command-line program is inside the app: `/Applications/Grainy RS.app/Contents/MacOS/grainy-cli --help`. To
uninstall, move the app to the Trash (delete the data folder as well to remove the edits).

## Develop and library changes in 0.3.0

- The RAW Basic panel fits its 15 sliders at 1920 × 1080 with 125% Windows scaling. Use the arrow keys to adjust the active Basic slider and Shift for larger steps. Cropping and rotating a photo preserve the resolution needed for a fit-to-window preview.
- Filename search uses a catalog index. When a batch recycle-bin operation stops partway through, Grainy updates the originals and virtual copies that moved. It reconciles interrupted catalog writes when the library reopens.
- JPEG XL export is labeled 16-bit lossless. Lightroom preset and catalog import reports include the Adobe process version when available.
- Subject and background masks run locally. On Windows an optional U²-Net ONNX model can supply the foreground mask. Set `GRAINY_U2NET_MODEL` to a local model file. Without a model, Grainy estimates the selection from image contrast.
- Tethered capture saves new shots to a chosen folder and imports them into the originating library. It requires a camera that supports remote capture through gphoto2, or through a WIA driver on Windows.

## Merge selected photographs

Select two or more originals in the library, then choose **Library → Merge HDR Exposures** for a bracket of a static scene or **Library → Merge Planar Panorama** for a horizontal or vertical sequence with overlap. Arrange panorama frames in shooting order. Both commands align translations, work from the original exposures, and save a new linear 32-bit TIFF. Grainy adds the result to the originating library or session. For a rotating camera sweep requiring a cylindrical or spherical projection, use a dedicated panorama stitcher.

The same processing is available as `grainy-cli merge hdr dark.dng light.dng -o result.tif` and `grainy-cli merge panorama left.tif right.tif -o panorama.tif`.

## Where it keeps its data

- Original photo files are never written, moved or deleted. Edits are stored in the library.
- Grainy RS has its own data folder, `%LOCALAPPDATA%\GrainyRS` on Windows and
  `~/Library/Application Support/GrainyRS` on macOS (library, previews, settings), separate from the Python
  Grainy's. `GrainyRS.exe --library <folder>` opens another library; the environment variable
  `GRAINY_RS_HOME` moves the whole data folder.
- **A Grainy library can be opened read-only** (File menu, *Open the Grainy Library Read-Only*): the Python
  Grainy's library is read from a snapshot copy, nothing in it is changed, and edits made there stay in that
  window.
- The network is used only to send feedback and to check for updates (from this repository's releases; an
  update is installed only after its size and SHA-256 match). `GrainyRS.exe --offline` or `GRAINY_OFFLINE=1`
  switches the network off completely.

## What is not done yet

- HDR edits can be exported as a 32-bit floating-point TIFF or a 10-bit PQ AVIF. Windows has a separate HDR display window on compatible hardware. The standard editing canvas uses an SDR preview.
- Video files are recognised and refused (as in the Python Grainy, which removed video in 0.5.74).
- A few English texts of the interface are still Korean, as in the Python Grainy.
- The automatic lens-correction scale is the least exact part of the engine (within 4e-4 of the Python Grainy;
  everything else is equal or within float rounding).
- Simple RAW develops are slower than in the Python Grainy for some cameras (Fujifilm X-Trans in particular).
- macOS: the first macOS release. It is built and tested on GitHub's macOS runners (the same tests as on
  Windows, about 950), but few real Macs have run it. There is no OpenMP on macOS (the serial code path, which
  gives the same pixels, only slower for some RAW files). macOS's maths library rounds a few functions
  differently from Windows', so some stages (noise reduction, Lab conversion, local adjustments, 16-bit exports)
  match the Python Grainy within small stated tolerances instead of exactly (8-bit exports are equal). Pictures
  of the window are compared with the reference pictures on Windows only (fonts render differently). The
  automatic monitor profile (ColorSync) and the self-update from the disk image have not yet been tried on a
  real Mac.
- Linux is not packaged or tested.

## Licence

Grainy RS is MIT licensed (see [LICENSE](LICENSE)). It includes LibRaw (used under the CDDL-1.0), Little CMS,
libjpeg-turbo, zlib, the Lensfun lens database (CC BY-SA 3.0) and other third-party code and data under their
own licences: see [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md), the `NOTICE` files of the crates, and
`THIRD_PARTY_NOTICES.txt` in the release package (generated by `tools/release/notices.py`).

## Build and test

Rust 1.90 or newer. A C/C++ compiler is needed: LibRaw, Little CMS and libjpeg-turbo are compiled from the
vendored sources. On Windows use the MSVC toolchain (Visual Studio Build Tools). On macOS install the Xcode
command line tools (`xcode-select --install`); Homebrew packages are not needed.

```
cargo build --release -p grainy-app -p grainy-cli
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

The application is `target/release/grainy-app` (`grainy-app.exe` on Windows; `GrainyRS.exe` in the Windows
package). The release packages are made by `tools/release/make_release.py` (Windows zip) and
`tools/release/make_macos.py` (macOS app and disk image, on a Mac: `rustup target add aarch64-apple-darwin
x86_64-apple-darwin` first); GitHub Actions builds both (`.github/workflows/build.yml`). How the reference
data in `golden/` is made, and how to contribute: [CONTRIBUTING.md](CONTRIBUTING.md). The workspace is one crate
per part of the engine (`crates/grainy-*`); the application's structure is described in
[crates/grainy-app/ARCHITECTURE.md](crates/grainy-app/ARCHITECTURE.md).

## 한국어

Grainy RS는 사진 편집기 [Grainy](https://github.com/pastorius9/grainy)를 Rust로 새로 만든 버전입니다. Grainy 0.6.8과
같은 보정, 같은 픽셀 결과를 냅니다. **아직 초기 버전**이며, 당분간은 Python으로 된 Grainy가 주 제품입니다.

- 설치: [Releases](https://github.com/pastorius9/grainy-rs/releases)에서 `GrainyRS-<버전>-windows-x64.zip`을 받아 압축을
  풀고 `GrainyRS.exe`를 실행합니다. Microsoft Visual C++ 재배포 패키지는 필요 없습니다.
- macOS: `GrainyRS-<버전>-macos-universal.dmg`를 열고 Grainy RS를 Applications 폴더로 끌어다 놓습니다. 처음 한 번은
  자체 서명 인증서라 막히므로, 앱을 우클릭(또는 Control-클릭) > 열기 > 열기를 하거나 시스템 설정 > 개인정보 보호 및
  보안에서 "그래도 열기"를 누르세요. 데이터는 `~/Library/Application Support/GrainyRS`에 저장됩니다.
- 데이터: 원본 사진은 절대 수정·이동·삭제하지 않습니다. 라이브러리와 설정은 `%LOCALAPPDATA%\GrainyRS`(Windows) 또는
  `~/Library/Application Support/GrainyRS`(macOS)에 따로 저장됩니다. 기존 Grainy 라이브러리는 읽기 전용으로 열 수 있습니다(파일 메뉴).
- 네트워크는 피드백 보내기와 업데이트 확인에만 쓰며, `--offline`으로 완전히 끌 수 있습니다.
- 라이선스: MIT. 함께 들어 있는 외부 라이브러리와 데이터의 라이선스는 `THIRD_PARTY_NOTICES.md`를 보세요.

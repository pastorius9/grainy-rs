# Grainy RS

Grainy RS is the Rust rewrite of [Grainy](https://github.com/pastorius9/grainy), a photo editor for film scans
and digital photos. Shared development stages are tested against reference data from Python Grainy 0.6.8.

**Grainy continues as Grainy RS.** New features and fixes come only here; the Python Grainy at
<https://github.com/pastorius9/grainy> gets no more updates after its last version, 0.6.9. Project page:
<https://pastorius9.github.io/grainy/>.

## Moving from the Python Grainy

Grainy RS is a separate program with its own data folder; it is installed beside the Python Grainy and does not
change it.

- **The library comes over on first start.** When Grainy RS starts with an empty library and finds the Python
  Grainy's library, it asks once whether to bring it over: photos, edits, history, collections, thumbnails and
  camera profiles. The Python Grainy's library is only read (from a snapshot copy) and never changed. Close the
  Python Grainy first.
- **Later:** *File management* menu › *Import the Python Grainy Library…* does the same at any time. If the Grainy RS
  library already has photos, it asks first and keeps a backup of it (*File management* › *Restore catalog* brings it
  back).
- **AI connection:** set it up again from Grainy RS (*AI Connection (MCP)…*). Its install line replaces an
  earlier `grainy` entry in Claude Code or Codex, so the AI programs talk to Grainy RS instead.

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
같은 보정, 같은 픽셀 결과를 냅니다. **Grainy는 이제 Grainy RS로 이어집니다.** 새 기능과 수정은 여기에만 나오고,
파이썬판 Grainy는 마지막 버전 0.6.9 이후 업데이트가 없습니다. 웹페이지: <https://pastorius9.github.io/grainy/>

- 파이썬판을 쓰던 분: Grainy RS는 따로 설치되는 별도 프로그램이라 파이썬판을 바꾸지 않습니다. 라이브러리가 빈 상태로 처음
  켜면 파이썬판 라이브러리(사진, 보정, 기록, 컬렉션, 썸네일, 카메라 프로필)를 가져올지 한 번 묻습니다. 파이썬판 라이브러리는
  읽기만 하고 바꾸지 않습니다(가져오기 전에 파이썬판을 닫아 주세요). 나중에 파일 관리 메뉴 › "파이썬 Grainy 라이브러리 가져오기…"로도
  가져올 수 있고, 이미 사진이 있으면 먼저 묻고 기존 라이브러리를 백업해 둡니다. AI 연결은 Grainy RS의 "AI 연결 (MCP)…" 창에서
  다시 설정하세요(예전 grainy 연결이 이 연결로 바뀝니다).

- 설치: [Releases](https://github.com/pastorius9/grainy-rs/releases)에서 `GrainyRS-<버전>-windows-x64.zip`을 받아 압축을
  풀고 `GrainyRS.exe`를 실행합니다. Microsoft Visual C++ 재배포 패키지는 필요 없습니다.
- macOS: `GrainyRS-<버전>-macos-universal.dmg`를 열고 Grainy RS를 Applications 폴더로 끌어다 놓습니다. 처음 한 번은
  자체 서명 인증서라 막히므로, 앱을 우클릭(또는 Control-클릭) > 열기 > 열기를 하거나 시스템 설정 > 개인정보 보호 및
  보안에서 "그래도 열기"를 누르세요. 데이터는 `~/Library/Application Support/GrainyRS`에 저장됩니다.
- 데이터: 원본 사진은 절대 수정·이동·삭제하지 않습니다. 라이브러리와 설정은 `%LOCALAPPDATA%\GrainyRS`(Windows) 또는
  `~/Library/Application Support/GrainyRS`(macOS)에 따로 저장됩니다. 기존 Grainy 라이브러리는 읽기 전용으로 열 수 있습니다(파일 메뉴).
- 네트워크는 피드백 보내기와 업데이트 확인에만 쓰며, `--offline`으로 완전히 끌 수 있습니다.
- 라이선스: MIT. 함께 들어 있는 외부 라이브러리와 데이터의 라이선스는 `THIRD_PARTY_NOTICES.md`를 보세요.

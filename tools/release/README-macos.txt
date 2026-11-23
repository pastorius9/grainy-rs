Grainy RS {version} for macOS (Apple Silicon and Intel)
=====================================================

Grainy RS is a photo editor for film scans and digital photos. Develop RAW files, adjust colour,
and export individual photos or batches. The earlier Python edition is available at
https://github.com/pastorius9/grainy. This build requires macOS 11 or newer.

Install and first start
-----------------------
Open the disk image and drag "Grainy RS" to the Applications folder.

The app is signed with a self-signed certificate and is not notarized by Apple, so macOS asks once:
  - Right-click (or Control-click) "Grainy RS" in the Applications folder, choose Open, then Open again; or
  - try to open it, then go to System Settings > Privacy & Security and press "Open Anyway".
After that it starts normally. Updates keep the same signature, so macOS keeps the permissions it gave the app
(folders, external disks).

The command-line program is inside the app:
    "/Applications/Grainy RS.app/Contents/MacOS/grainy-cli" render photo.jpg -o out.jpg --settings settings.json
    "/Applications/Grainy RS.app/Contents/MacOS/grainy-cli" --help

Your photos and your data
-------------------------
Photo edits are stored in the library. The Library menu also includes explicit commands
to move, rename or send selected originals to the system trash.
Grainy RS keeps its library and settings in ~/Library/Application Support/GrainyRS (its own folder, separate
from the Python Grainy). Other places:
    "Grainy RS.app/Contents/MacOS/GrainyRS" --library <folder>    use another library folder
    GRAINY_RS_HOME=<folder>                                        use another data folder
Moving from the Python Grainy: on the first start with an empty library, Grainy RS offers once to bring
over the Python Grainy's library (photos, edits, history, collections, thumbnails, camera profiles);
later the same is in File menu > Import the Python Grainy Library... The Python Grainy's library is
only read, never changed. It can also be opened read-only (File menu > Open the Grainy Library
Read-Only).
To uninstall: move the app to the Trash. To remove the edits as well, delete the data folder above.

Network
-------
Grainy RS uses the network only when you send feedback or check for updates (from
https://github.com/pastorius9/grainy-rs releases). To switch the network off completely, start it with
    "Grainy RS.app/Contents/MacOS/GrainyRS" --offline              (or set GRAINY_OFFLINE=1)

Licence
-------
Grainy RS is MIT licensed (see LICENSE). It includes LibRaw, Little CMS, libjpeg-turbo, zlib, the Lensfun
lens database and other third-party software and data under their own licences: see
THIRD_PARTY_NOTICES.txt. Source code: https://github.com/pastorius9/grainy-rs

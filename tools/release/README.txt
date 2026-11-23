Grainy RS {version} for Windows (64-bit)
=====================================

Grainy RS is a photo editor for film scans and digital photos. Develop RAW files, adjust colour,
and export individual photos or batches. The earlier Python edition is available at
https://github.com/pastorius9/grainy.

Start
-----
Unzip the whole GrainyRS folder anywhere (for example into your Documents folder) and double-click
GrainyRS.exe. No installation is needed (the Visual C++ Redistributable is not needed either: the one
Microsoft file the program uses, vcomp140.dll, is in the folder). Windows may warn about an unknown
publisher the first time. Keep all the files of the folder together.

grainy-cli.exe renders a photo from the command line with the same pixels as an export:
    grainy-cli.exe render photo.jpg -o out.jpg --settings settings.json
    grainy-cli.exe --help

Your photos and your data
-------------------------
Photo edits are stored in the library. The Library menu also includes explicit commands
to move, rename or send selected originals to the system trash.
Grainy RS keeps its library and settings in %LOCALAPPDATA%\GrainyRS (its own folder, separate from the
Python Grainy). Other places:
    GrainyRS.exe --library <folder>    use another library folder
    set GRAINY_RS_HOME=<folder>         use another data folder
Moving from the Python Grainy: on the first start with an empty library, Grainy RS offers once to bring
over the Python Grainy's library (photos, edits, history, collections, thumbnails, camera profiles);
later the same is in File menu > Import the Python Grainy Library... The Python Grainy's library is
only read, never changed. It can also be opened read-only (File menu > Open the Grainy Library
Read-Only).

Network
-------
Grainy RS uses the network only when you send feedback or check for updates (from
https://github.com/pastorius9/grainy-rs releases). To switch the network off completely:
    GrainyRS.exe --offline              (or set GRAINY_OFFLINE=1)

Licence
-------
Grainy RS is MIT licensed (see LICENSE). It includes LibRaw, Little CMS, libjpeg-turbo, zlib, the Lensfun
lens database and other third-party software and data under their own licences: see
THIRD_PARTY_NOTICES.txt. Source code: https://github.com/pastorius9/grainy-rs

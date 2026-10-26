Skywave for Linux (x86_64)
==========================

A chill, single-player driving game. Built on Ubuntu 22.04, so it runs on most
distributions from that era on, and on SteamOS (Steam Deck).

Run
---
    ./skywave.sh                  (or ./skywave)
    ./skywave.sh --seed 42        another town
    ./skywave.sh --version        print the version
    ./skywave.sh --help           all options

Everything the game needs (cars, shaders, the town generator) is built into the
`skywave` binary; the only file it reads or writes is its settings file (below).
It needs a GPU with Vulkan (or OpenGL ES 3) drivers and libudev, which every
desktop distribution ships.
`skywave.desktop` is a menu entry: edit its Exec= and Path= lines to where you
unpacked this folder and copy it to ~/.local/share/applications/.

Controls
--------
Keyboard: W/S or arrows to drive, A/D to steer, Space handbrake, C camera,
hold the right mouse button to look around, [ and ] radio, M map, Esc menu.
A gamepad works too and is picked up when plugged in: right/left trigger to
drive and brake, left stick to steer, Start for the menu. Every control can be
rebound in the menu.

AppImage
--------
Each release also has Skywave-x86_64.AppImage: the same game as one file.
Make it executable and run it:
    chmod +x Skywave-x86_64.AppImage
    ./Skywave-x86_64.AppImage --version
It needs FUSE to mount itself (SteamOS and most desktops have it); without it,
run it with APPIMAGE_EXTRACT_AND_RUN=1 set. The rolling nightly build is at
    https://github.com/adammathes/skywave/releases/download/nightly/Skywave-x86_64.AppImage

Steam Deck
----------
Easiest: in Desktop Mode open Konsole and run tools/install-deck.sh from the
Skywave repository (the README has the one-line command). It installs the
AppImage into ~/Applications/Skywave/, adds a menu entry and prints these steps.
Re-run it to update. By hand:
1. Switch to Desktop Mode. Put Skywave-x86_64.AppImage somewhere in your home
   folder (e.g. ~/Applications/Skywave/) and make it executable (above, or
   Properties > Permissions > Is executable in the file manager).
   With this archive instead: unpack it, e.g. into ~/Games/skywave, and use
   skywave.sh in the next step.
2. In Steam (desktop), choose Games > Add a Non-Steam Game to My Library,
   click Browse, pick the AppImage (or skywave.sh; set the file type filter
   to All Files to see it) and add it. Leave Launch Options empty.
3. Back in Gaming Mode, start Skywave from your library. The built-in controls
   act as a standard gamepad and the game is gamepad-first; no extra
   controller layout is needed. It runs at the Deck's native 1280x800.

Settings
--------
Settings and key bindings are saved to
    $XDG_CONFIG_HOME/skywave/settings.json
or, when XDG_CONFIG_HOME is not set,
    ~/.config/skywave/settings.json
Delete that file to go back to the defaults.

The game makes no network requests except to music sources you choose.

License: BSD-3-Clause, see LICENSE.

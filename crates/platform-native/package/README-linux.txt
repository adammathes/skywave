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

Steam Deck
----------
1. Switch to Desktop Mode and unpack this archive, e.g. into ~/Games/skywave.
2. In Steam (desktop), choose Games > Add a Non-Steam Game to My Library,
   browse to skywave.sh and add it (show all file types to see it).
3. Back in Gaming Mode, start Skywave from your library. The built-in controls
   act as a standard gamepad; no extra controller layout is needed.

Settings
--------
Settings and key bindings are saved to
    $XDG_CONFIG_HOME/skywave/settings.json
or, when XDG_CONFIG_HOME is not set,
    ~/.config/skywave/settings.json
Delete that file to go back to the defaults.

The game makes no network requests except to music sources you choose.

License: BSD-3-Clause, see LICENSE.

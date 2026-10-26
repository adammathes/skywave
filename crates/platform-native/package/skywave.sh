#!/bin/sh
# Starts Skywave from wherever this folder was unpacked (for desktop launchers and the
# Steam Deck, which start programs from another working directory).
cd "$(dirname "$0")" || exit 1
exec ./skywave "$@"

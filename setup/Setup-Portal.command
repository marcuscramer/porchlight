#!/bin/bash
# macOS: double-click in Finder to set up a connected Portal for Porchlight.
cd "$(dirname "$0")" || exit 1
bash ./setup.sh
echo
read -n 1 -s -r -p "Press any key to close this window…"
echo

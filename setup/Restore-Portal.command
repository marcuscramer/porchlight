#!/bin/bash
# macOS: double-click in Finder to undo Porchlight's system changes on a connected Portal.
cd "$(dirname "$0")" || exit 1
bash ./setup.sh --restore
echo
read -n 1 -s -r -p "Press any key to close this window…"
echo

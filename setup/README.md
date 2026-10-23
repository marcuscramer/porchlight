# Porchlight setup for Meta Portal

Sets up a Portal for Porchlight from a computer, with nothing to type: it installs the latest Porchlight and makes
the one-time settings from the main README's "Convenience" section (updates from inside the app, call wake-up,
camera and microphone allowed, readable system dialogs on the Portal TV). It asks before switching off Meta's own
system updates.

## Steps

1. On the Portal: **Settings > Debug > ADB Enabled**.
2. Connect the Portal to the computer with a **USB-C cable** that carries data.
3. Double-click:
   - **macOS:** `Setup-Portal.command`
   - **Windows:** `Setup-Portal.bat`
   - **Linux:** run `./setup.sh` in a terminal
4. When the Portal asks **"Allow USB debugging?"**, tick "Always allow" and choose **Allow**.
5. Wait for **Done.** Then pick a name on the Portal and pair with someone.

The computer needs an internet connection: the setup downloads the latest Porchlight, and, if the computer doesn't
have Google's `adb` already, Google's Android platform-tools into this folder (nothing is installed system-wide).

You can run it again at any time, for example after a factory reset; it changes only what isn't set up yet.

## Undo

Double-click `Restore-Portal` (`.command` / `.bat`, or `./setup.sh --restore`). It switches call wake-up off, takes
back the permissions, and puts Meta's install verifier, system updates and dialog overlays back exactly as the setup
found them. It leaves Porchlight installed unless you say otherwise.

## If something gets in the way

- **macOS says the file is from an unidentified developer.** Right-click `Setup-Portal.command` > Open, then Open.
  On newer macOS: System Settings > Privacy & Security > "Open Anyway".
- **Windows blocks the script.** Right-click the folder > Properties > tick **Unblock** > OK, then double-click
  again. If Windows doesn't recognise the Portal at all, it may need Google's USB driver.
- **"No Portal found yet"** stays on screen: try another cable or USB port, and check that ADB Enabled is on.
- **More than one device connected:** unplug the others.

The Windows version has been checked on macOS with PowerShell 7 against an emulator, not yet on a Windows PC.

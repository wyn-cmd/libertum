# Libertum 0.4.1

## Changes

- **Smoother, lighter trackpad scrolling.** Trackpad movement is now collected between frames and applied once per displayed frame, instead of moving the page for every event the pad sends. A fast gesture no longer does several times the work between two frames, and a still window asks the GPU for nothing.
- **`.deb` and `.rpm` installs update themselves.** On Linux, **Update now** in a packaged install downloads the new package, checks it against `SHA256SUMS` and installs it through the desktop's own administrator prompt (`pkexec`); Libertum then restarts the new version by itself. There is no file to download, no command to copy and no package to open. If the password prompt is dismissed or the install fails, the update window says why and the installed version is left as it was. On a system without `pkexec`, the package is still downloaded and its command shown, as before.
- **The AppImage and the macOS app start themselves after an update,** as before; the restart step is now shared with the package install.
- The Settings hint and the update window describe the package install.

## Packaging

- Windows: `Libertum-Setup.exe` and `Libertum-Setup-0.4.1.exe`.
- Linux: `.deb`, `.rpm` and `.AppImage`.
- macOS: `.dmg` and `.zip` for Apple Silicon and Intel.
- `SHA256SUMS` covers every attached file.

## Notes

- Builds are unsigned. Windows SmartScreen may warn on first run; on macOS, right-click the app and choose **Open**.
- Installing a package always needs the administrator password. Libertum never asks for it itself: the prompt is the desktop's (polkit), and the install runs only after it is accepted.
- Updating an AppImage or a macOS bundle needs no password, but the file must be in a folder the user can write to.

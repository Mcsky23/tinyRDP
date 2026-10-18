# FreeRDP aspect preservation

The stock SDL3 frontend stretches the desktop independently along X and Y when smart sizing is enabled. tinyRDP builds the SDL frontend from the verified FreeRDP 3.32.2 archive against the installed public Homebrew SDK. It does not rebuild or replace the protocol libraries.

`scripts/build_backend.py` applies the small integration changes to the cached source:

- Local scaling calls `SdlWindow::drawAspectFit`, implemented in `aspect_fit.cpp`.
- SDL's letterboxed logical presentation applies one viewport to drawing, Retina conversion, mouse input, and cursor positions. The original additional X/Y scaling is disabled for this path.
- Cursor dimensions are computed from transformed rectangle endpoints so the bars do not inflate a cursor's size.
- Cursor images and hotspots use only the viewport's scale and Retina conversion. The viewport origin is subtracted before rounding, so black bars cannot shift the visible pointer away from the click location. The same geometry helper is used in production and in the rendering diagnostic.
- Button presses and wheel events in the bars are ignored. Motion and releases are clamped to the desktop edge so releasing a drag outside the desktop does not leave a button held remotely.
- Scrolling follows the macOS deltas delivered by SDL without undoing natural scrolling. Mouse utility changes therefore carry through when that utility applies to the remote desktop window. The per-device `invert_scroll` setting is passed explicitly as `MINIRDP_INVERT_SCROLL=0` or `1` to each helper process, then reverses both scroll axes after those local adjustments. Global FreeRDP scroll-direction preferences are not used by this path.
- `scroll_input.hpp` is shared by the production wheel handler and `--minirdp-scroll-test`. The diagnostic covers both natural-scroll flags, positive and negative utility-adjusted deltas, inversion, horizontal scrolling, diagonal fractional gestures, and send failures. Vertical and horizontal events use independent RDP flags.
- Per-device shared folders are passed as a JSON array in `MINIRDP_SHARED_FOLDERS`. `shared_folders.cpp` registers each name/path pair with `freerdp_device_new` and `freerdp_device_collection_add`, then enables device redirection before connecting. This is the same drive representation used by `/drive:name,path`, with paths preserved without CSV parsing. Each connection explicitly receives its own list, including an empty list when sharing is off.
- `--minirdp-drive-test` checks the real SDK drive structures and JSON handoff, including Unicode and punctuation. `scripts/check_folder_io.py` separately compiles the pinned upstream `drive_file.c` with `shared_folder_io_test.cpp` and checks file operations on temporary Mac fixtures. That diagnostic binary is not bundled with the app and does not connect to a server.
- The SDK build is adjusted to find the upstream embedded font and omit upstream installer targets.
- A local rendering diagnostic is added as `--minirdp-render-test`.

The renderer keeps a texture at the remote desktop's resolution, uploads only changed rectangles, clears the output to black, and presents the entire texture through SDL's aspect-preserving viewport. Resizing does not stretch or discard the stored desktop pixels.

The archive stays under `target/freerdp-backend`, pinned by SHA-256. `scripts/bundle_runtime.py` copies the frontend and its complete non-system library dependency graph into the app. Each dependency uses a relative `@loader_path` reference; external runtime search paths are removed. OpenSSL's dynamically loaded providers and CA roots are included too. Each Mach-O file is signed, and the packaging check rejects missing or external runtime dependencies. Runtime files are replaced atomically so active sessions retain their existing mapped inodes.

`FreeRDP-LICENSE.txt` and `OpenSans-OFL.txt` are included in the app bundle. `Resources/Licenses/Runtime` contains dependency notices, package versions, and Homebrew recipes identifying their upstream sources and build options. New tinyRDP code is covered by the repository's MIT license; dependencies retain their own licenses, including the codec libraries linked by the installed FreeRDP SDK.

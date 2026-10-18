#!/usr/bin/env python3
"""Build only FreeRDP's SDL frontend against the installed Homebrew SDK."""
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
VERSION = "3.32.2"
SHA256 = "9ccb8357bf0188044a460de775c7ea289ca7d2a2a917eca0f0a7c4dc9c8b08ea"
CACHE = ROOT / "target" / "freerdp-backend"
SOURCE = CACHE / ("FreeRDP-" + VERSION)
BUILD = CACHE / "build"


def run(*args, **kwargs):
    subprocess.run([str(arg) for arg in args], check=True, **kwargs)


def replace(path, old, new):
    text = path.read_text()
    if text.count(old) != 1:
        raise RuntimeError("Unexpected FreeRDP source at " + str(path))
    path.write_text(text.replace(old, new, 1))


def prepare():
    CACHE.mkdir(parents=True, exist_ok=True)
    archive = CACHE / ("FreeRDP-" + VERSION + ".tar.gz")
    if not archive.exists():
        print("Downloading FreeRDP " + VERSION, flush=True)
        url = "https://codeload.github.com/FreeRDP/FreeRDP/tar.gz/refs/tags/" + VERSION
        with urllib.request.urlopen(url, timeout=60) as response:
            data = response.read()
        if hashlib.sha256(data).hexdigest() != SHA256:
            raise RuntimeError("FreeRDP source checksum did not match")
        archive.write_bytes(data)
    if hashlib.sha256(archive.read_bytes()).hexdigest() != SHA256:
        raise RuntimeError("Cached FreeRDP source checksum did not match")

    # Copy regular files from the verified archive. Never follow archive links
    # or accept paths escaping our build directory; works with macOS Python 3.9.
    with tarfile.open(archive) as tar:
        for member in tar:
            destination = CACHE / member.name
            if not destination.resolve().is_relative_to(CACHE.resolve()):
                raise RuntimeError("Invalid source archive path")
            if member.isdir():
                destination.mkdir(parents=True, exist_ok=True)
            elif member.isfile():
                data = tar.extractfile(member).read()
                destination.parent.mkdir(parents=True, exist_ok=True)
                # Preserve timestamps of unchanged files for incremental builds.
                if not destination.exists() or destination.read_bytes() != data:
                    destination.write_bytes(data)

    sdl = SOURCE / "client" / "SDL"
    # The upstream SDK build omits installation helpers and assumes resources
    # live at the project root. Neither assumption applies to this frontend build.
    resource_cmake = sdl / "common" / "res" / "CMakeLists.txt"
    text = resource_cmake.read_text().replace("${CMAKE_SOURCE_DIR}/resources", str(SOURCE / "resources"))
    resource_cmake.write_text(text)
    client_cmake = sdl / "SDL3" / "CMakeLists.txt"
    text = client_cmake.read_text()
    text = text.replace('install_freerdp_desktop("${MODULE_NAME}" "${SDL_CLIENT_UUID}")', "")
    text = text.replace("add_subdirectory(man)", "")
    text = text.replace("add_win_console_manifest(SRCS)",
                        'include_directories("${CMAKE_CURRENT_SOURCE_DIR}" "${MINIRDP_NATIVE_DIR}")\nlist(APPEND SRCS "${MINIRDP_NATIVE_DIR}/aspect_fit.cpp" "${MINIRDP_NATIVE_DIR}/aspect_fit_test.cpp" "${MINIRDP_NATIVE_DIR}/scroll_input_test.cpp" "${MINIRDP_NATIVE_DIR}/shared_folders.cpp")\nadd_win_console_manifest(SRCS)')
    client_cmake.write_text(text)

    window = sdl / "SDL3" / "sdl_window.hpp"
    replace(window, "\tvoid updateSurface();", "\tvoid updateSurface();\n\t[[nodiscard]] bool drawAspectFit(SDL_Surface* surface, const std::vector<SDL_Rect>& rects, bool present = true);")
    context = sdl / "SDL3" / "sdl_context.cpp"
    replace(context,
            "SDL_SetAppMetadataProperty(SDL_PROP_APP_METADATA_NAME_STRING, SDL_CLIENT_NAME);",
            'SDL_SetAppMetadataProperty(SDL_PROP_APP_METADATA_NAME_STRING, "tinyRDP");')
    text = context.read_text()
    start = text.index("\t\twindow.setOffsetX(0);", text.index("bool SdlContext::drawToWindow("))
    end = text.index("\n\t}\n\telse", start)
    text = text[:start] + """        // SDL's letterboxed presentation handles both axes and pointer mapping.
        _localScale = { 1.0f, 1.0f };
        return window.drawAspectFit(surface, rects);""" + text[end:]
    context.write_text(text)
    replace(context,
            "const auto size = pixelToScreen(id, SDL_FPoint{ pos.w, pos.h });\n\tSDL_FRect r{ fpos.x, fpos.y, size.x, size.y };",
            "const auto end = pixelToScreen(id, SDL_FPoint{ pos.x + pos.w, pos.y + pos.h });\n\tSDL_FRect r{ fpos.x, fpos.y, end.x - fpos.x, end.y - fpos.y };")
    pointer = sdl / "SDL3" / "sdl_pointer.cpp"
    replace(pointer, '#include "sdl_pointer.hpp"', '#include "sdl_pointer.hpp"\n#include "cursor_geometry.hpp"')
    replace(pointer,
            "const auto pos = sdl->pixelToScreen(id, orig, true);",
            """const auto pos = minirdp::cursorToWindow(orig, [sdl, id](const SDL_FPoint& point) {
        return sdl->pixelToScreen(id, point);
    });""")
    touch = sdl / "SDL3" / "sdl_touch.cpp"
    replace(touch, '#include "sdl_touch.hpp"', '#include "sdl_touch.hpp"\n#include "scroll_input.hpp"')
    text = touch.read_text()
    start = text.index('\tconst bool flipped =', text.index('bool SdlTouch::handleEvent(SdlContext* sdl, const SDL_MouseWheelEvent& ev)'))
    end = text.index('\n}\n', start)
    text = text[:start] + """    return minirdp::sendScroll(ev, minirdp::invertScrollEnabled(), [sdl](Uint16 flags, Sint32 value) {
        return send_mouse_wheel(sdl, flags, value);
    });""" + text[end:]
    touch.write_text(text)
    # Ignore clicks and scrolling in the bars. Clamp motion and button releases
    # at the desktop edge so a drag released outside cannot leave a button stuck.
    replace(context,
            "return SDL_ConvertEventToRenderCoordinates(renderer, &ev);",
            """if (!SDL_ConvertEventToRenderCoordinates(renderer, &ev))
        return false;
    if (!useLocalScale() || !context()->gdi)
        return true;
    float* x = nullptr;
    float* y = nullptr;
    switch (ev.type)
    {
        case SDL_EVENT_MOUSE_MOTION: x = &ev.motion.x; y = &ev.motion.y; break;
        case SDL_EVENT_MOUSE_BUTTON_DOWN:
        case SDL_EVENT_MOUSE_BUTTON_UP: x = &ev.button.x; y = &ev.button.y; break;
        case SDL_EVENT_MOUSE_WHEEL: x = &ev.wheel.mouse_x; y = &ev.wheel.mouse_y; break;
        default: return true;
    }
    const float width = static_cast<float>(context()->gdi->width);
    const float height = static_cast<float>(context()->gdi->height);
    const bool inside = *x >= 0 && *x < width && *y >= 0 && *y < height;
    if (!inside && ev.type != SDL_EVENT_MOUSE_MOTION && ev.type != SDL_EVENT_MOUSE_BUTTON_UP)
        return false;
    *x = std::clamp(*x, 0.0f, width - 1.0f);
    *y = std::clamp(*y, 0.0f, height - 1.0f);
    return true;""")
    main = sdl / "SDL3" / "sdl_freerdp.cpp"
    replace(main, '#include "sdl_utils.hpp"', '#include "sdl_utils.hpp"\nextern int minirdp_render_test();\nextern int minirdp_scroll_test();\nextern int minirdp_drive_test();\nextern bool minirdp_configure_shared_folders(rdpSettings* settings);')
    replace(main, '\tsdl_rdp->sdl->setMetadata();', '''    if (status == 0 && !minirdp_configure_shared_folders(settings))
    {
        std::cerr << "Could not configure shared folders: " << SDL_GetError() << std::endl;
        return -1;
    }
\tsdl_rdp->sdl->setMetadata();''')
    text = main.read_text()
    # Keep a small local rendering diagnostic that needs no RDP server.
    import re
    text, count = re.subn(r"(int main\(int argc, char\* argv\[\]\)\s*\{)",
                         r'\1\n    if (argc == 2 && strcmp(argv[1], "--minirdp-render-test") == 0)\n        return minirdp_render_test();\n    if (argc == 2 && strcmp(argv[1], "--minirdp-scroll-test") == 0)\n        return minirdp_scroll_test();\n    if (argc == 2 && strcmp(argv[1], "--minirdp-drive-test") == 0)\n        return minirdp_drive_test();', text)
    if count != 1:
        raise RuntimeError("Could not locate the SDL frontend entry point")
    main.write_text(text)


def main():
    if not shutil.which("cmake"):
        raise RuntimeError("Install the backend build dependencies: brew install cmake freerdp")
    prefix = subprocess.check_output(["brew", "--prefix"], text=True).strip()
    prepare()
    run("cmake", "-S", SOURCE / "client/SDL", "-B", BUILD,
        "-DCMAKE_PREFIX_PATH=" + prefix, "-DCMAKE_BUILD_TYPE=MinSizeRel",
        "-DFREERDP_DEFAULT_PROJECT_VERSION=" + VERSION,
        "-DFREERDP_VERSION_FULL=" + VERSION + "-minirdp",
        "-DWITH_CLIENT_SDL=ON", "-DWITH_CLIENT_SDL3=ON", "-DWITH_CLIENT_SDL2=OFF",
        "-DWITH_SDL_LINK_SHARED=ON", "-DBUILD_TESTING=OFF",
        "-DCMAKE_OSX_DEPLOYMENT_TARGET=13.0", "-DCMAKE_VERBOSE_MAKEFILE=OFF",
        "-DCMAKE_RUNTIME_OUTPUT_DIRECTORY=" + str(BUILD / "bin"),
        "-DMINIRDP_NATIVE_DIR=" + str(ROOT / "native"))
    run("cmake", "--build", BUILD, "--target", "sdl3-freerdp", "--parallel", str(min(os.cpu_count() or 2, 8)))
    executable = BUILD / "bin" / "sdl-freerdp"
    # Keep using Homebrew's public SDK libraries after a formula upgrade.
    dependencies = subprocess.check_output(["otool", "-L", str(executable)], text=True)
    for line in dependencies.splitlines()[1:]:
        library = line.strip().split(" (")[0]
        if library.startswith(prefix + "/Cellar/freerdp/"):
            stable = prefix + "/opt/freerdp/lib/" + Path(library).name
            run("install_name_tool", "-change", library, stable, executable)
    run("codesign", "--force", "--sign", "-", executable)
    print("Built aspect-preserving FreeRDP frontend:", executable, flush=True)


if __name__ == "__main__":
    main()

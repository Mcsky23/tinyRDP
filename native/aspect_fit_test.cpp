// Local SDL rendering regression checks. No RDP connection or credentials.
// Copyright (c) 2026 tinyRDP contributors. MIT License.
#include "sdl_window.hpp"
#include "cursor_geometry.hpp"

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <memory>
#include <stdexcept>

namespace {
using Surface = std::unique_ptr<SDL_Surface, decltype(&SDL_DestroySurface)>;

void require(bool condition, const char* message)
{
    if (!condition)
        throw std::runtime_error(message);
}

void near(float actual, float expected, float tolerance, const char* message)
{
    require(std::abs(actual - expected) <= tolerance, message);
}

Surface snapshot(SDL_Renderer* renderer, SDL_Surface* desktop)
{
    // Read the whole output, including bars outside the logical viewport,
    // before presentation (GPU backbuffers are undefined after Present).
    require(SDL_SetRenderLogicalPresentation(renderer, 0, 0, SDL_LOGICAL_PRESENTATION_DISABLED), SDL_GetError());
    Surface pixels(SDL_RenderReadPixels(renderer, nullptr), SDL_DestroySurface);
    require(pixels != nullptr, SDL_GetError());
    require(SDL_SetRenderLogicalPresentation(renderer, desktop->w, desktop->h, SDL_LOGICAL_PRESENTATION_LETTERBOX), SDL_GetError());
    return pixels;
}

void check(SdlWindow& window, SDL_Surface* desktop)
{
    require(window.drawAspectFit(desktop, {}, false), SDL_GetError());
    auto renderer = window.renderer();
    int width = 0;
    int height = 0;
    require(SDL_GetRenderOutputSize(renderer, &width, &height), SDL_GetError());
    const float scale = std::min(static_cast<float>(width) / desktop->w,
                                 static_cast<float>(height) / desktop->h);
    const float expectedWidth = desktop->w * scale;
    const float expectedHeight = desktop->h * scale;
    const float expectedX = (width - expectedWidth) / 2.0f;
    const float expectedY = (height - expectedHeight) / 2.0f;
    SDL_FRect viewport{};
    require(SDL_GetRenderLogicalPresentationRect(renderer, &viewport), SDL_GetError());
    near(viewport.w, expectedWidth, 1.1f, "Desktop width is stretched");
    near(viewport.h, expectedHeight, 1.1f, "Desktop height is stretched");
    near(viewport.x, expectedX, 1.1f, "Desktop is not centered horizontally");
    near(viewport.y, expectedY, 1.1f, "Desktop is not centered vertically");

    auto pixels = snapshot(renderer, desktop);
    require(pixels->w == width && pixels->h == height, "Readback did not include the whole output");
    int left = pixels->w;
    int top = pixels->h;
    int right = -1;
    int bottom = -1;
    for (int y = 0; y < pixels->h; ++y)
    {
        for (int x = 0; x < pixels->w; ++x)
        {
            Uint8 r = 0, g = 0, b = 0, a = 0;
            require(SDL_ReadSurfacePixel(pixels.get(), x, y, &r, &g, &b, &a), SDL_GetError());
            if (r > 220 && g > 220 && b > 220)
            {
                left = std::min(left, x);
                top = std::min(top, y);
                right = std::max(right, x);
                bottom = std::max(bottom, y);
            }
            else
            {
                require(r < 8 && g < 8 && b < 8, "Bars are not black");
            }
        }
    }
    near(static_cast<float>(left), expectedX, 1.1f, "Rendered left bar is incorrect");
    near(static_cast<float>(top), expectedY, 1.1f, "Rendered top bar is incorrect");
    near(static_cast<float>(right + 1), expectedX + expectedWidth, 1.1f, "Rendered right edge is incorrect");
    near(static_cast<float>(bottom + 1), expectedY + expectedHeight, 1.1f, "Rendered bottom edge is incorrect");

    // Verify actual window coordinates, including pixel density, produce the
    // same remote coordinates for clicks and motion as the rendered desktop.
    for (const SDL_FPoint point : {SDL_FPoint{1, 1},
                                   SDL_FPoint{desktop->w / 2.0f, desktop->h / 2.0f},
                                   SDL_FPoint{desktop->w - 2.0f, desktop->h - 2.0f}})
    {
        SDL_FPoint local{};
        require(SDL_RenderCoordinatesToWindow(renderer, point.x, point.y, &local.x, &local.y), SDL_GetError());
        SDL_Event event{};
        event.button.type = SDL_EVENT_MOUSE_BUTTON_DOWN;
        event.button.windowID = window.id();
        event.button.x = local.x;
        event.button.y = local.y;
        require(SDL_ConvertEventToRenderCoordinates(renderer, &event), SDL_GetError());
        near(event.button.x, point.x, 0.01f, "Click x coordinate is offset");
        near(event.button.y, point.y, 0.01f, "Click y coordinate is offset");
    }
    SDL_FPoint a{}, b{};
    require(SDL_RenderCoordinatesToWindow(renderer, 100, 100, &a.x, &a.y), SDL_GetError());
    require(SDL_RenderCoordinatesToWindow(renderer, 132, 132, &b.x, &b.y), SDL_GetError());
    // SDL rounds the fitted viewport to physical pixels. Allow the resulting
    // subpixel difference for a 32-pixel cursor, without allowing stretching.
    near(b.x - a.x, b.y - a.y, 0.05f, "Cursor aspect ratio exceeds viewport pixel rounding");

    // Exercise the exact cursor geometry used by sdl_Pointer_Set_Process.
    // A cursor's hotspot is relative to its image: letterbox offsets must not
    // move its visible tip away from the location sent in the mouse event.
    int windowWidth = 0, windowHeight = 0;
    require(SDL_GetWindowSize(window.window(), &windowWidth, &windowHeight), SDL_GetError());
    const float scaleX = viewport.w / desktop->w * windowWidth / width;
    const float scaleY = viewport.h / desktop->h * windowHeight / height;
    for (const SDL_FPoint hotspot : {SDL_FPoint{0, 0}, SDL_FPoint{7, 11},
                                     SDL_FPoint{15, 15}, SDL_FPoint{31, 31}})
    {
        const auto cursor = minirdp::cursorToWindow(SDL_FRect{hotspot.x, hotspot.y, 32, 32},
            [renderer](const SDL_FPoint& point) {
                SDL_FPoint result{};
                require(SDL_RenderCoordinatesToWindow(renderer, point.x, point.y,
                                                        &result.x, &result.y), SDL_GetError());
                return result;
            });
        // Include the clamp used by SDL cursor creation. Previously the bar
        // margin could push a top-left hotspot all the way to the bottom edge.
        const float actualHotspotX = std::clamp(cursor.x, 0.0f, cursor.w - 1.0f);
        const float actualHotspotY = std::clamp(cursor.y, 0.0f, cursor.h - 1.0f);
        near(actualHotspotX, std::floor(hotspot.x * scaleX), 0.01f, "Cursor hotspot includes the horizontal bar offset");
        near(actualHotspotY, std::floor(hotspot.y * scaleY), 0.01f, "Cursor hotspot includes the vertical bar offset");
        near(cursor.w, std::ceil(32 * scaleX), 0.01f, "Cursor width is incorrect");
        near(cursor.h, std::ceil(32 * scaleY), 0.01f, "Cursor height is incorrect");

        // The visual hotspot should coincide with the click to within the
        // single window-point rounding imposed by SDL_CreateColorCursor.
        const SDL_FPoint target{desktop->w / 2.0f, desktop->h / 2.0f};
        SDL_FPoint click{};
        require(SDL_RenderCoordinatesToWindow(renderer, target.x, target.y, &click.x, &click.y), SDL_GetError());
        near(click.x - actualHotspotX + hotspot.x * scaleX, click.x, 1.01f, "Visible cursor is horizontally offset from click");
        near(click.y - actualHotspotY + hotspot.y * scaleY, click.y, 1.01f, "Visible cursor is vertically offset from click");
    }
    require(SDL_RenderPresent(renderer), SDL_GetError());
    std::printf("Aspect fit verified: %dx%d desktop in %dx%d output\n", desktop->w, desktop->h, width, height);
}
}

int minirdp_render_test()
{
    SDL_SetHint(SDL_HINT_RENDER_DRIVER, "software");
    if (!SDL_Init(SDL_INIT_VIDEO))
    {
        std::fprintf(stderr, "%s\n", SDL_GetError());
        return 1;
    }
    int result = 0;
    try
    {
        auto window = SdlWindow::create(SDL_GetPrimaryDisplay(), "tinyRDP rendering check",
                                        SDL_WINDOW_HIDDEN | SDL_WINDOW_RESIZABLE, 960, 600);
        require(window.window() && window.renderer(), SDL_GetError());
        std::printf("SDL renderer: %s; window pixel density: %.1f\n",
                    SDL_GetRendererName(window.renderer()), SDL_GetWindowPixelDensity(window.window()));
        for (const SDL_Point resolution : {SDL_Point{1920, 1080}, SDL_Point{1024, 768}})
        {
            Surface desktop(SDL_CreateSurface(resolution.x, resolution.y, SDL_PIXELFORMAT_BGRA32), SDL_DestroySurface);
            require(desktop != nullptr, SDL_GetError());
            require(SDL_FillSurfaceRect(desktop.get(), nullptr, SDL_MapSurfaceRGB(desktop.get(), 255, 255, 255)), SDL_GetError());
            for (const SDL_Point size : {SDL_Point{960, 600}, SDL_Point{1200, 400},
                                         SDL_Point{400, 800}, SDL_Point{640, 360}})
            {
                require(window.resize(size), SDL_GetError());
                require(SDL_SyncWindow(window.window()), SDL_GetError());
                SDL_PumpEvents();
                check(window, desktop.get());
            }
            if (std::getenv("MINIRDP_TEST_FULLSCREEN"))
            {
                require(SDL_ShowWindow(window.window()), SDL_GetError());
                window.fullscreen(true, true);
                SDL_PumpEvents();
                require((SDL_GetWindowFlags(window.window()) & SDL_WINDOW_FULLSCREEN) != 0,
                        "Window did not enter fullscreen");
                check(window, desktop.get());
                window.fullscreen(false, true);
            }
            // A partial update must keep the rest of the desktop intact.
            SDL_Rect patch{10, 10, 32, 32};
            require(SDL_FillSurfaceRect(desktop.get(), &patch, SDL_MapSurfaceRGB(desktop.get(), 0, 0, 255)), SDL_GetError());
            require(window.drawAspectFit(desktop.get(), {patch}, false), SDL_GetError());
            auto pixels = snapshot(window.renderer(), desktop.get());
            SDL_FRect viewport{};
            require(SDL_GetRenderLogicalPresentationRect(window.renderer(), &viewport), SDL_GetError());
            const int x = static_cast<int>(viewport.x + viewport.w * 26.0f / desktop->w);
            const int y = static_cast<int>(viewport.y + viewport.h * 26.0f / desktop->h);
            Uint8 r = 0, g = 0, b = 0, a = 0;
            require(SDL_ReadSurfacePixel(pixels.get(), x, y, &r, &g, &b, &a), SDL_GetError());
            require(r < 8 && g < 8 && b > 240, "Partial desktop update was lost");
            require(SDL_ReadSurfacePixel(pixels.get(), pixels->w / 2, pixels->h / 2, &r, &g, &b, &a), SDL_GetError());
            require(r > 240 && g > 240 && b > 240, "Partial update erased unchanged desktop pixels");
            require(SDL_RenderPresent(window.renderer()), SDL_GetError());
            require(SDL_FillSurfaceRect(desktop.get(), &patch, SDL_MapSurfaceRGB(desktop.get(), 255, 255, 255)), SDL_GetError());
            require(window.drawAspectFit(desktop.get(), {patch}), SDL_GetError());
            check(window, desktop.get());
        }
        std::puts("Rendering checks passed: aspect ratio, centering, black bars, pointer mapping, cursor hotspots, and resize.");
    }
    catch (const std::exception& error)
    {
        std::fprintf(stderr, "Rendering check failed: %s (%s)\n", error.what(), SDL_GetError());
        result = 1;
    }
    SDL_Quit();
    return result;
}

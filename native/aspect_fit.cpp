// tinyRDP's aspect-preserving presentation for the FreeRDP SDL3 client.
// Copyright (c) 2026 tinyRDP contributors. MIT License.
#include "sdl_window.hpp"

#include <cstdint>

bool SdlWindow::drawAspectFit(SDL_Surface* surface, const std::vector<SDL_Rect>& rects, bool present)
{
    if (!_renderer || !surface || surface->w <= 0 || surface->h <= 0)
        return false;

    const bool fresh = !_gdiTexture || _gdiTextureW != surface->w || _gdiTextureH != surface->h;
    if (!ensureGdiTexture(surface))
        return false;

    // Keep a desktop-sized texture across window resizes. Only upload damaged
    // pixels; changing the window's shape must not change the remote desktop.
    if (fresh || rects.empty())
    {
        if (!SDL_UpdateTexture(_gdiTexture, nullptr, surface->pixels, surface->pitch))
            return false;
    }
    else
    {
        const auto* format = SDL_GetPixelFormatDetails(surface->format);
        if (!format)
            return false;
        const SDL_Rect bounds{0, 0, surface->w, surface->h};
        for (const auto& damage : rects)
        {
            SDL_Rect clipped{};
            if (!SDL_GetRectIntersection(&damage, &bounds, &clipped))
                continue;
            const auto* pixels = static_cast<const uint8_t*>(surface->pixels)
                + static_cast<int64_t>(clipped.y) * surface->pitch
                + static_cast<int64_t>(clipped.x) * format->bytes_per_pixel;
            if (!SDL_UpdateTexture(_gdiTexture, &clipped, pixels, surface->pitch))
                return false;
        }
    }

    if (!SDL_SetRenderTarget(_renderer, nullptr))
        return false;
    // SDL uses the same centered, uniform transform for rendering and its
    // window/render coordinate conversion APIs (including Retina density).
    if (!SDL_SetRenderLogicalPresentation(_renderer, surface->w, surface->h,
                                          SDL_LOGICAL_PRESENTATION_LETTERBOX))
        return false;
    if (!SDL_SetRenderDrawColor(_renderer, 0, 0, 0, 255) || !SDL_RenderClear(_renderer))
        return false;
    if (!SDL_RenderTexture(_renderer, _gdiTexture, nullptr, nullptr))
        return false;
    return !present || SDL_RenderPresent(_renderer);
}

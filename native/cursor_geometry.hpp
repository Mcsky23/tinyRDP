// Cursor images and hotspots are distances, independent of the desktop origin.
// Copyright (c) 2026 tinyRDP contributors. MIT License.
#pragma once

#include <SDL3/SDL.h>
#include <cmath>

namespace minirdp {
template <typename Transform>
SDL_FRect cursorToWindow(const SDL_FRect& cursor, Transform toWindow)
{
    const auto origin = toWindow(SDL_FPoint{0, 0});
    const auto hotspot = toWindow(SDL_FPoint{cursor.x, cursor.y});
    const auto size = toWindow(SDL_FPoint{cursor.w, cursor.h});
    // Remove translation before rounding. Applying the letterbox margin to a
    // cursor hotspot shifts the image relative to the actual mouse location.
    return {std::floor(hotspot.x - origin.x), std::floor(hotspot.y - origin.y),
            std::ceil(size.x - origin.x), std::ceil(size.y - origin.y)};
}
}

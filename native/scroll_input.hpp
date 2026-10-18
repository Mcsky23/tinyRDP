// Follow the scroll events delivered by macOS, including mouse utilities.
// Copyright (c) 2026 tinyRDP contributors. MIT License.
#pragma once

#include <SDL3/SDL.h>
#include <freerdp/input.h>

namespace minirdp {
inline bool invertScrollEnabled()
{
    const auto* value = SDL_getenv("MINIRDP_INVERT_SCROLL");
    return value && SDL_strcmp(value, "1") == 0;
}

template <typename Send>
bool sendScroll(const SDL_MouseWheelEvent& event, bool invert, Send send)
{
    // SDL's deltas already include macOS natural scrolling and any utility
    // changes. Do not undo SDL_MOUSEWHEEL_FLIPPED: it describes that direction.
    const float scale = invert ? -120.0f : 120.0f;
    const auto x = static_cast<Sint32>(event.x * scale);
    const auto y = static_cast<Sint32>(event.y * scale);
    // Keep axis flags separate when a trackpad gesture moves diagonally.
    return (y == 0 || send(PTR_FLAGS_WHEEL, y))
        && (x == 0 || send(PTR_FLAGS_HWHEEL, x));
}
}

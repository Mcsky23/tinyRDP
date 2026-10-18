// Exercise the scroll handler used by the SDL frontend without an RDP server.
// Copyright (c) 2026 tinyRDP contributors. MIT License.
#include "scroll_input.hpp"

#include <cstdio>
#include <stdexcept>
#include <utility>
#include <vector>

namespace {
using Scroll = std::pair<Uint16, Sint32>;

void require(bool condition, const char* message)
{
    if (!condition)
        throw std::runtime_error(message);
}

std::vector<Scroll> scroll(float x, float y, SDL_MouseWheelDirection direction, bool invert)
{
    SDL_MouseWheelEvent event{};
    event.type = SDL_EVENT_MOUSE_WHEEL;
    event.x = x;
    event.y = y;
    event.direction = direction;
    std::vector<Scroll> result;
    require(minirdp::sendScroll(event, invert, [&result](Uint16 flags, Sint32 value) {
        result.emplace_back(flags, value);
        return true;
    }), "Scroll event was rejected");
    return result;
}
}

int minirdp_scroll_test()
{
    try
    {
        for (const auto direction : {SDL_MOUSEWHEEL_NORMAL, SDL_MOUSEWHEEL_FLIPPED})
        {
            // Changing natural-scroll metadata must not reverse the deltas
            // delivered by macOS or by a utility that reverses only the mouse.
            require(scroll(0, 1, direction, false) == std::vector<Scroll>{{PTR_FLAGS_WHEEL, 120}},
                    "Default scrolling reversed a positive macOS event");
            require(scroll(0, -1, direction, false) == std::vector<Scroll>{{PTR_FLAGS_WHEEL, -120}},
                    "Default scrolling reversed a negative macOS event");
            require(scroll(0, 1, direction, true) == std::vector<Scroll>{{PTR_FLAGS_WHEEL, -120}},
                    "Inversion did not reverse vertical scrolling");
            require(scroll(0, -1, direction, true) == std::vector<Scroll>{{PTR_FLAGS_WHEEL, 120}},
                    "Inversion did not reverse negative vertical scrolling");
            require(scroll(1, 0, direction, false) == std::vector<Scroll>{{PTR_FLAGS_HWHEEL, 120}},
                    "Default horizontal scrolling is reversed");
            require(scroll(1, 0, direction, true) == std::vector<Scroll>{{PTR_FLAGS_HWHEEL, -120}},
                    "Inversion did not reverse horizontal scrolling");
            require(scroll(-0.25f, 0.5f, direction, false)
                        == std::vector<Scroll>{{PTR_FLAGS_WHEEL, 60}, {PTR_FLAGS_HWHEEL, -30}},
                    "Diagonal trackpad gesture mixed its axis flags or lost its precision");
            require(scroll(-0.25f, 0.5f, direction, true)
                        == std::vector<Scroll>{{PTR_FLAGS_WHEEL, -60}, {PTR_FLAGS_HWHEEL, 30}},
                    "Inversion did not reverse both axes of a trackpad gesture");
            require(scroll(0, 0, direction, true).empty(), "Empty gesture sent a scroll event");
        }
        SDL_MouseWheelEvent event{};
        event.x = event.y = 1;
        int calls = 0;
        require(!minirdp::sendScroll(event, false, [&calls](Uint16, Sint32) {
            ++calls;
            return false;
        }) && calls == 1, "Scroll handler did not propagate a send failure");
        std::printf("Scroll checks passed: macOS event direction, utility-reversed events, inversion, both axes, and fractional gestures. Session mode: %s.\n",
                    minirdp::invertScrollEnabled() ? "inverted" : "follow macOS");
        return 0;
    }
    catch (const std::exception& error)
    {
        std::fprintf(stderr, "Scroll check failed: %s\n", error.what());
        return 1;
    }
}

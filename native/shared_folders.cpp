// Register selected folders through FreeRDP's public drive redirection API.
// Copyright (c) 2026 tinyRDP contributors. MIT License.
#include <SDL3/SDL.h>
#include <freerdp/settings.h>
#include <winpr/json.h>

#include <cstdio>
#include <cstring>
#include <memory>
#include <stdexcept>

namespace {
bool addFolders(rdpSettings* settings, const char* manifest)
{
    if (!manifest)
        return true;
    std::unique_ptr<WINPR_JSON, decltype(&WINPR_JSON_Delete)> folders(
        WINPR_JSON_Parse(manifest), WINPR_JSON_Delete);
    if (!folders || !WINPR_JSON_IsArray(folders.get()))
        return SDL_SetError("The shared folder list is invalid");
    const auto count = WINPR_JSON_GetArraySize(folders.get());
    if (count > 64)
        return SDL_SetError("Too many shared folders");
    for (size_t index = 0; index < count; ++index)
    {
        auto folder = WINPR_JSON_GetArrayItem(folders.get(), index);
        const auto* name = WINPR_JSON_GetStringValue(WINPR_JSON_GetObjectItemCaseSensitive(folder, "name"));
        const auto* path = WINPR_JSON_GetStringValue(WINPR_JSON_GetObjectItemCaseSensitive(folder, "path"));
        if (!name || !*name || !path || path[0] != '/')
            return SDL_SetError("A shared folder needs a name and an absolute path");
        if (freerdp_device_collection_find(settings, name))
            return SDL_SetError("Duplicate shared folder name");
        const char* args[] = {name, path};
        auto device = freerdp_device_new(RDPDR_DTYP_FILESYSTEM, 2, args);
        if (!device)
            return SDL_SetError("Could not allocate a redirected folder");
        if (!freerdp_device_collection_add(settings, device))
        {
            freerdp_device_free(device);
            return SDL_SetError("Could not register a redirected folder");
        }
    }
    return count == 0 || freerdp_settings_set_bool(settings, FreeRDP_DeviceRedirection, TRUE);
}

void require(bool condition, const char* message)
{
    if (!condition)
        throw std::runtime_error(message);
}
}

bool minirdp_configure_shared_folders(rdpSettings* settings)
{
    // A process-local JSON list preserves all path characters, including CSV
    // delimiters and quotes, while using the same drives as /drive:name,path.
    return addFolders(settings, SDL_getenv("MINIRDP_SHARED_FOLDERS"));
}

int minirdp_drive_test()
{
    try
    {
        using Settings = std::unique_ptr<rdpSettings, decltype(&freerdp_settings_free)>;
        Settings settings(freerdp_settings_new(0), freerdp_settings_free);
        require(settings != nullptr, "Could not allocate FreeRDP settings");
        require(addFolders(settings.get(), R"([{"name":"Projects","path":"/Users/test/Project, \"quoted\" 'files' ✓"},{"name":"Files","path":"/tmp/another folder"}])"), SDL_GetError());
        require(freerdp_settings_get_uint32(settings.get(), FreeRDP_DeviceCount) == 2,
                "Not all folders were registered");
        require(freerdp_settings_get_bool(settings.get(), FreeRDP_DeviceRedirection),
                "RDP drive redirection is disabled");
        auto device = freerdp_device_collection_find(settings.get(), "Projects");
        require(device && device->Type == RDPDR_DTYP_FILESYSTEM, "Folder was not registered as a filesystem drive");
        auto drive = reinterpret_cast<RDPDR_DRIVE*>(device);
        require(std::strcmp(drive->Path, "/Users/test/Project, \"quoted\" 'files' ✓") == 0,
                "Folder path was changed by parsing");
        Settings empty(freerdp_settings_new(0), freerdp_settings_free);
        require(empty && addFolders(empty.get(), "[]"), "Empty sharing list was rejected");
        require(freerdp_settings_get_uint32(empty.get(), FreeRDP_DeviceCount) == 0,
                "An empty list shared a drive");
        require(!addFolders(empty.get(), "{}"), "Invalid sharing list was accepted");
        require(!addFolders(empty.get(), R"([{"name":"Files"}])"), "Missing folder path was accepted");
        Settings session(freerdp_settings_new(0), freerdp_settings_free);
        require(session && minirdp_configure_shared_folders(session.get()), SDL_GetError());
        std::printf("Drive checks passed: native FreeRDP registration, multiple folders, Unicode, commas, quotes, empty settings, and invalid data. Session folders: %u.\n",
                    freerdp_settings_get_uint32(session.get(), FreeRDP_DeviceCount));
        return 0;
    }
    catch (const std::exception& error)
    {
        std::fprintf(stderr, "Drive check failed: %s\n", error.what());
        return 1;
    }
}

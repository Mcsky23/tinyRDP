// Exercise the pinned FreeRDP drive filesystem implementation on local fixtures.
// Copyright (c) 2026 tinyRDP contributors. MIT License.
#include <winpr/string.h>
#include <freerdp/channels/rdpdr.h>
extern "C" {
#include "drive_file.h"
}
#include <cstdio>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <iterator>
#include <memory>
#include <stdexcept>
#include <string>

namespace {
void require(bool condition, const char* message)
{
    if (!condition)
        throw std::runtime_error(std::string(message) + " (WinPR error " + std::to_string(GetLastError()) + ")");
}
struct FileDeleter { void operator()(DRIVE_FILE* file) const { if (file) (void)drive_file_free(file); } };
using File = std::unique_ptr<DRIVE_FILE, FileDeleter>;
using Wide = std::unique_ptr<WCHAR, decltype(&std::free)>;
struct StreamDeleter { void operator()(wStream* stream) const { Stream_Free(stream, TRUE); } };
using Stream = std::unique_ptr<wStream, StreamDeleter>;

File open(const WCHAR* base, const char* path, UINT32 disposition = FILE_OPEN)
{
    size_t length = 0;
    Wide relative(ConvertUtf8ToWCharAlloc(path, &length), std::free);
    require(relative != nullptr, "Could not convert the relative path");
    return File(drive_file_new(base, relative.get(), static_cast<UINT32>(length), 1,
        GENERIC_READ | GENERIC_WRITE | DELETE, disposition, FILE_NON_DIRECTORY_FILE,
        FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE));
}

std::string readRemote(DRIVE_FILE* file)
{
    Stream stream(Stream_New(nullptr, 1024));
    require(stream != nullptr, "Could not allocate the read stream");
    UINT32 length = 1024;
    require(drive_file_read(file, stream.get(), 0, &length), "RDP read failed");
    return std::string(reinterpret_cast<const char*>(Stream_Buffer(stream.get())), length);
}

void writeRemote(DRIVE_FILE* file, const std::string& text)
{
    require(drive_file_seek(file, 0), "RDP seek failed");
    require(drive_file_write(file, reinterpret_cast<const BYTE*>(text.data()), static_cast<UINT32>(text.size())), "RDP write failed");
}

std::string readMac(const std::filesystem::path& path)
{
    std::ifstream input(path);
    require(input.good(), "Could not read the Mac file");
    return std::string(std::istreambuf_iterator<char>(input), {});
}
}

int main()
{
    char temporary[] = "/tmp/minirdp-folder-io-XXXXXX";
    const auto* created = mkdtemp(temporary);
    if (!created) return 1;
    const std::filesystem::path root(created);
    int result = 0;
    try
    {
        const auto folder = root / "Project, \"quoted\" 'files' ✓";
        std::filesystem::create_directory(folder);
        Wide base(ConvertUtf8ToWCharAlloc(folder.c_str(), nullptr), std::free);
        require(base != nullptr, "Could not convert the shared folder path");
        std::ofstream(folder / "from-mac.txt") << "Created on Mac\n";
        auto file = open(base.get(), "\\from-mac.txt");
        require(file != nullptr, "RDP could not open a Mac-created file");
        require(readRemote(file.get()) == "Created on Mac\n", "Mac data was not visible through RDP");
        std::ofstream(folder / "from-mac.txt", std::ios::trunc) << "Changed on Mac\n";
        require(readRemote(file.get()) == "Changed on Mac\n", "Mac edits were not visible through RDP");
        writeRemote(file.get(), "Changed by RDP\n");
        require(readMac(folder / "from-mac.txt") == "Changed by RDP\n", "RDP edits were not visible on Mac");
        file.reset();
        std::filesystem::rename(folder / "from-mac.txt", folder / "mac-renamed.txt");
        file = open(base.get(), "\\mac-renamed.txt");
        require(file && readRemote(file.get()) == "Changed by RDP\n", "Mac rename was not visible through RDP");
        file.reset();
        std::filesystem::remove(folder / "mac-renamed.txt");
        require(!open(base.get(), "\\mac-renamed.txt"), "Mac deletion was not visible through RDP");

        file = open(base.get(), "\\from-remote.txt", FILE_CREATE);
        require(file != nullptr, "RDP file creation failed");
        writeRemote(file.get(), "Created by RDP\n");
        require(readMac(folder / "from-remote.txt") == "Created by RDP\n", "RDP-created file was not visible on Mac");
        size_t length = 0;
        Wide renamed(ConvertUtf8ToWCharAlloc("\\remote-renamed.txt", &length), std::free);
        require(renamed != nullptr, "Rename path conversion failed");
        const auto bytes = static_cast<UINT32>(length * sizeof(WCHAR));
        Stream rename(Stream_New(nullptr, 6 + bytes));
        require(rename != nullptr, "Could not allocate rename stream");
        Stream_Write_UINT8(rename.get(), 0); // ReplaceIfExists
        Stream_Write_UINT8(rename.get(), 0); // RootDirectory
        Stream_Write_UINT32(rename.get(), bytes);
        Stream_Write(rename.get(), renamed.get(), bytes);
        Stream_SetPosition(rename.get(), 0);
        require(drive_file_set_information(file.get(), FileRenameInformation, 6 + bytes, rename.get()), "RDP rename failed");
        require(!std::filesystem::exists(folder / "from-remote.txt") && std::filesystem::exists(folder / "remote-renamed.txt"), "RDP rename was not visible on Mac");
        Stream disposition(Stream_New(nullptr, 1));
        require(disposition != nullptr, "Could not allocate delete stream");
        Stream_Write_UINT8(disposition.get(), 1);
        Stream_SetPosition(disposition.get(), 0);
        require(drive_file_set_information(file.get(), FileDispositionInformation, 1, disposition.get()), "RDP delete request failed");
        file.reset();
        require(!std::filesystem::exists(folder / "remote-renamed.txt"), "RDP deletion was not visible on Mac");
        std::puts("Folder I/O checks passed: creation, edits, renames, and deletion in both directions through the FreeRDP drive implementation.");
    }
    catch (const std::exception& error)
    {
        std::fprintf(stderr, "Folder I/O check failed: %s\n", error.what());
        result = 1;
    }
    std::error_code ignored;
    std::filesystem::remove_all(root, ignored);
    return result;
}

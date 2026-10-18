#!/usr/bin/env python3
"""Check the pinned drive implementation using temporary Mac files; no server."""
import shlex
import subprocess
from build_backend import ROOT, SOURCE


def run(*args):
    subprocess.run([str(arg) for arg in args], check=True)


build = ROOT / "target/freerdp-backend/folder-io-check"
build.mkdir(parents=True, exist_ok=True)
source = SOURCE / "channels/drive/client"
if not (source / "drive_file.c").is_file():
    raise RuntimeError("Build the backend first: python3 scripts/build_backend.py")
cflags = ["-Wno-deprecated-declarations"] + shlex.split(subprocess.check_output(["pkg-config", "--cflags", "freerdp3", "winpr3"], text=True))
libs = shlex.split(subprocess.check_output(["pkg-config", "--libs", "freerdp3", "winpr3"], text=True))
run("clang", "-std=c11", "-c", source / "drive_file.c", "-o", build / "drive_file.o", *cflags)
run("clang++", "-std=c++17", ROOT / "native/shared_folder_io_test.cpp", build / "drive_file.o",
    "-I" + str(source), "-o", build / "folder-io-test", *cflags, *libs)
run(build / "folder-io-test")

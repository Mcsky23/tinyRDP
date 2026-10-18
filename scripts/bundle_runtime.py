#!/usr/bin/env python3
"""Copy the helper's complete non-system runtime into the macOS app bundle."""
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile


def output(*args):
    return subprocess.check_output([str(arg) for arg in args], text=True)


def run(*args):
    result = subprocess.run([str(arg) for arg in args], capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError(f"{args[0]} failed:\n{result.stdout}{result.stderr}")


def system_library(name):
    return name.startswith(("/usr/lib/", "/System/Library/"))


def load_commands(binary):
    dependencies, rpaths = [], []
    for block in output("otool", "-l", binary).split("Load command ")[1:]:
        command = re.search(r"^\s*cmd (LC_\w+)$", block, re.MULTILINE)
        if not command:
            continue
        kind = command.group(1)
        if kind in {"LC_LOAD_DYLIB", "LC_LOAD_WEAK_DYLIB", "LC_REEXPORT_DYLIB",
                    "LC_LAZY_LOAD_DYLIB", "LC_LOAD_UPWARD_DYLIB"}:
            dependencies.append(re.search(r"^\s*name (.+) \(offset \d+\)$", block, re.MULTILINE).group(1))
        elif kind == "LC_RPATH":
            rpaths.append(re.search(r"^\s*path (.+) \(offset \d+\)$", block, re.MULTILINE).group(1))
    return dependencies, rpaths


def resolve_dependency(name, binary, rpaths, executable):
    def expand(value):
        return value.replace("@loader_path", str(binary.parent)).replace("@executable_path", str(executable.parent))

    if name.startswith("@rpath/"):
        candidates = [Path(expand(path)) / name[len("@rpath/"):] for path in rpaths]
    else:
        candidates = [Path(expand(name))]
    for path in candidates:
        if path.is_absolute() and path.is_file():
            return path.resolve()
    raise RuntimeError(f"Cannot resolve {name} required by {binary}")


def keg_for(path):
    for parent in path.parents:
        if (parent / "INSTALL_RECEIPT.json").is_file():
            return parent
    raise RuntimeError("Cannot locate Homebrew package metadata for " + str(path))


def copy_notices(keg, destination):
    destination.mkdir(parents=True, exist_ok=True)
    patterns = ("LICENSE*", "LICENCE*", "COPYING*", "COPYRIGHT*", "NOTICE*", "AUTHORS*", "OFL*", "*GPL*", "MPL*")
    files = {path for pattern in patterns for path in keg.glob(pattern) if path.is_file()}
    docs = keg / "share/doc"
    if docs.is_dir():
        files.update(path for pattern in patterns for path in docs.rglob(pattern) if path.is_file())
    for source in sorted(files):
        target = destination / source.relative_to(keg)
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)
    # The recipes record upstream sources, versions, patches, and build options.
    for source in (keg / ".brew").glob("*.rb"):
        shutil.copyfile(source, destination / source.name)
    return [str(path.relative_to(keg)) for path in sorted(files)]


def verify(app):
    contents = app.resolve() / "Contents"
    binaries = [contents / "Helpers/sdl-freerdp", contents / "MacOS/tinyrdp"]
    binaries.extend((contents / "Frameworks").rglob("*.dylib"))
    for binary in binaries:
        dependencies, rpaths = load_commands(binary)
        for name in dependencies:
            if system_library(name):
                continue
            if not name.startswith("@loader_path/"):
                raise RuntimeError(f"External runtime dependency in {binary.name}: {name}")
            target = (binary.parent / name[len("@loader_path/"):]).resolve()
            if not target.is_relative_to(contents) or not target.is_file():
                raise RuntimeError(f"Missing bundled dependency in {binary.name}: {name}")
        if any(not system_library(path) and not path.startswith("@loader_path") for path in rpaths):
            raise RuntimeError("External runtime search path in " + str(binary))
    print(f"Verified {len(binaries)} Mach-O files: all dependencies are bundled or provided by macOS.", flush=True)


def bundle(executable, app):
    executable = executable.resolve()
    contents = app.resolve() / "Contents"
    prefix = Path(output("brew", "--prefix").strip())
    graph, kegs = {}, {}
    queue = [executable]
    # OpenSSL can load its legacy provider at runtime, outside the Mach-O graph.
    openssl_kegs = set()
    while queue:
        binary = queue.pop()
        if binary in graph:
            continue
        names, rpaths = load_commands(binary)
        graph[binary] = {}
        for name in names:
            if system_library(name):
                continue
            dependency = resolve_dependency(name, binary, rpaths, executable)
            graph[binary][name] = dependency
            queue.append(dependency)
        if binary != executable:
            keg = keg_for(binary)
            kegs[keg] = None
            if keg.parent.name.startswith("openssl@") and keg not in openssl_kegs:
                openssl_kegs.add(keg)
                queue.extend(path.resolve() for path in (keg / "lib/ossl-modules").glob("*.dylib"))

    targets = {executable: Path("Helpers/sdl-freerdp")}
    used = set()
    for source in graph:
        if source == executable:
            continue
        relative = Path("Frameworks") / source.name
        if source.parent.name == "ossl-modules":
            relative = Path("Frameworks/ossl-modules") / source.name
        if relative in used:
            raise RuntimeError("Conflicting library filenames: " + str(relative))
        used.add(relative)
        targets[source] = relative

    contents.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".runtime-", dir=contents) as directory:
        stage = Path(directory)
        for source, relative in targets.items():
            target = stage / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, target)
            target.chmod(0o755)
            changes = []
            for name, dependency in graph[source].items():
                local = os.path.relpath(targets[dependency], relative.parent)
                changes.extend(["-change", name, "@loader_path/" + local])
            if source != executable:
                changes.extend(["-id", "@loader_path/" + target.name])
            for rpath in load_commands(source)[1]:
                if not system_library(rpath):
                    changes.extend(["-delete_rpath", rpath])
            if changes:
                run("install_name_tool", *changes, target)
            run("codesign", "--force", "--sign", os.environ.get("CODESIGN_IDENTITY", "-"), target)

        packages = []
        for keg in sorted(kegs):
            package = keg.parent.name
            notices = copy_notices(keg, stage / "Resources/Licenses/Runtime" / package)
            packages.append({"name": package, "version": keg.name, "notices": notices,
                             "libraries": [str(targets[path]) for path in targets if path.is_relative_to(keg)]})
        certificates = prefix / "opt/ca-certificates/share/ca-certificates/cacert.pem"
        if not certificates.is_file():
            raise RuntimeError("The Homebrew CA certificate bundle is missing")
        openssl = stage / "Resources/OpenSSL"
        openssl.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(certificates, openssl / "cert.pem")
        (openssl / "openssl.cnf").write_text("# tinyRDP uses the default OpenSSL provider and bundled CA roots.\n")
        ca_keg = keg_for(certificates.resolve())
        copy_notices(ca_keg, stage / "Resources/Licenses/Runtime/ca-certificates")
        packages.append({"name": "ca-certificates", "version": ca_keg.name})
        (stage / "Resources/Licenses/Runtime/packages.json").write_text(json.dumps(packages, indent=2) + "\n")

        # Replace files atomically. Existing sessions keep their mapped inodes.
        for source in sorted(stage.rglob("*")):
            if source.is_file():
                target = contents / source.relative_to(stage)
                target.parent.mkdir(parents=True, exist_ok=True)
                os.replace(source, target)
    verify(app)
    size = sum((contents / relative).stat().st_size for relative in targets.values())
    print(f"Bundled helper and {len(targets) - 1} runtime libraries ({size / 1024 / 1024:.1f} MiB).", flush=True)


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--verify":
        verify(Path(sys.argv[2]))
    elif len(sys.argv) == 3:
        bundle(Path(sys.argv[1]), Path(sys.argv[2]))
    else:
        raise SystemExit("Usage: bundle_runtime.py HELPER tinyRDP.app | --verify tinyRDP.app")

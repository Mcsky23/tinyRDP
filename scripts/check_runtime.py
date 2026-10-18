#!/usr/bin/env python3
"""Exercise a relocated tinyRDP bundle while denying access to Homebrew."""
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

from bundle_runtime import verify


def main():
    app = Path(sys.argv[1] if len(sys.argv) > 1 else "dist/tinyRDP.app").resolve()
    verify(app)
    subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)], check=True)
    profile = '(version 1) (allow default) (deny file-read* (subpath "/opt/homebrew") (subpath "/usr/local"))'
    with tempfile.TemporaryDirectory(prefix="minirdp-standalone-") as directory:
        temporary = Path(directory).resolve()
        relocated = temporary / "Moved tinyRDP.app"
        shutil.copytree(app, relocated)
        contents = relocated / "Contents"
        home = temporary / "home"
        home.mkdir()
        env = {
            "PATH": "/usr/bin:/bin:/usr/sbin:/sbin",
            "HOME": str(home),
            "TMPDIR": str(temporary),
            "LANG": "en_US.UTF-8",
            "SDL_VIDEODRIVER": "dummy",
            "MINIRDP_TEST_FULLSCREEN": "1",
            "MINIRDP_SHARED_FOLDERS": "[]",
            "SSL_CERT_FILE": str(contents / "Resources/OpenSSL/cert.pem"),
            "OPENSSL_CONF": str(contents / "Resources/OpenSSL/openssl.cnf"),
            "OPENSSL_MODULES": str(contents / "Frameworks/ossl-modules"),
            "DYLD_PRINT_LIBRARIES": "1",
        }
        for diagnostic in ("drive", "scroll", "render"):
            command = ["/usr/bin/sandbox-exec", "-p", profile,
                       str(contents / "Helpers/sdl-freerdp"), f"--minirdp-{diagnostic}-test"]
            result = subprocess.run(command, env=env, capture_output=True, text=True, timeout=45)
            if result.returncode:
                raise RuntimeError(f"Standalone {diagnostic} check failed:\n{result.stdout}{result.stderr}")
            loaded = [line for line in result.stderr.splitlines() if line.startswith("dyld[")]
            if any("/opt/homebrew/" in line or "/usr/local/" in line for line in loaded):
                raise RuntimeError("A diagnostic loaded a library outside the app: " + diagnostic)
            print(f"Standalone {diagnostic} check passed with Homebrew access denied.", flush=True)
    print("Relocation and standalone runtime checks passed.", flush=True)


if __name__ == "__main__":
    main()

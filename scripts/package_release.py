"""Package a native release binary. Run from the repository root with Python 3.11+."""
import argparse
import hashlib
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
import zipfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True)
    parser.add_argument("--output", type=Path, default=Path("dist"))
    args = parser.parse_args()
    if subprocess.check_output(["git", "status", "--porcelain", "--untracked-files=no"]):
        raise SystemExit("Commit tracked changes before packaging a release")
    version = tomllib.loads(Path("Cargo.toml").read_text())["package"]["version"]
    windows = "windows" in args.target
    binary = "clicloud.exe" if windows else "clicloud"
    source = Path("target") / args.target / "release" / binary
    # Verify that this is a runnable binary of the version we are packaging.
    result = subprocess.check_output([str(source.resolve()), "--version"], text=True).strip()
    if result != f"clicloud {version}":
        raise SystemExit(f"Unexpected binary version: {result}")
    # The build machine has the Visual C++ runtime; the user's Windows may not.
    if windows and b"vcruntime140" in source.read_bytes().lower():
        raise SystemExit("The Windows binary needs VCRUNTIME140.dll; link the C runtime statically")
    name = f"clicloud-{version}-{args.target}"
    args.output.mkdir(parents=True, exist_ok=True)
    archive = args.output / (name + (".zip" if windows else ".tar.gz"))
    with tempfile.TemporaryDirectory() as temp:
        stage = Path(temp) / name
        stage.mkdir()
        shutil.copy2(source, stage / binary)
        for file in ["README.md", "README.ru.md", "LICENSE", "THIRD_PARTY_NOTICES", "config.example.json"]:
            shutil.copy2(file, stage / file)
        shutil.copy2(Path("docs/releases") / f"v{version}.md", stage / "RELEASE_NOTES.md")
        (stage / "SOURCE_COMMIT").write_text(
            subprocess.check_output(["git", "rev-parse", "HEAD"], text=True), encoding="utf-8"
        )
        if windows:
            with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as output:
                for file in sorted(stage.iterdir()):
                    output.write(file, f"{name}/{file.name}")
        else:
            with tarfile.open(archive, "w:gz") as output:
                output.add(stage, arcname=name)
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    archive.with_name(archive.name + ".sha256").write_text(
        f"{digest}  {archive.name}\n", encoding="ascii"
    )
    print(archive)


if __name__ == "__main__":
    main()

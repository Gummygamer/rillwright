"""Install Rillwright using the Dory-capable LÖVE runtime on a Switch 2 SD."""
import argparse
import hashlib
import io
from pathlib import Path
import struct
from tempfile import TemporaryDirectory
import zipfile

from PIL import Image

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("card", type=Path)
card = parser.parse_args().card
source = Path(__file__).resolve().parent
switch = card / "switch"
runtime = switch / "gen1recomp"
model = runtime / "models/dory1.0-4b-q5_k_m.gguf"
with model.open("rb") as stream:
    if stream.read(8) != b"GGUF\x03\0\0\0":
        raise SystemExit(f"Expected a GGUF v3 model at {model}")
nro = bytearray((runtime / "gen1recomp.nro").read_bytes())
if hashlib.sha256(nro).hexdigest() != "c866c2ba8e5107aac8634837298d79dcbd90fbe847f6796b877c9e9432f55c09":
    raise SystemExit("Expected the Switch 2 Dory runtime that detects the integrated Switch 2 GPU")

# Keep executable segments intact and replace the launcher's icon and title.
base = struct.unpack_from("<I", nro, 24)[0]
magic, version, _, _, offset, size, _, romfs_size = struct.unpack_from("<4sI6Q", nro, base)
assert magic == b"ASET" and version == 0 and size == 0x4000 and romfs_size == 0
metadata = nro[base + offset:base + offset + size]
for language in range(16):
    start = language * 0x300
    metadata[start:start+0x300] = b"Rillwright".ljust(0x200,b"\0") + b"Rillwright".ljust(0x100,b"\0")
icon = io.BytesIO()
with Image.open(source.parents[1] / "crates/rillwright-gui/assets/rillwright.png") as image:
    image.convert("RGB").resize((256,256), Image.Resampling.LANCZOS).save(icon, "JPEG")
jpeg = icon.getvalue()
nro[base:] = struct.pack("<4sI6Q", magic, version, 56, len(jpeg), 56+len(jpeg), size, 0, 0) + jpeg + metadata

destination = switch / "rillwright"
destination.mkdir(exist_ok=True)
with TemporaryDirectory(dir=destination) as temporary:
    stage = Path(temporary)
    with zipfile.ZipFile(stage / "game.love", "w", zipfile.ZIP_DEFLATED) as archive:
        for name in ("conf.lua", "main.lua", "ui.lua", "worker.lua", "tools.lua", "storage.lua"):
            archive.write(source / name, name)
    (stage / "rillwright.nro").write_bytes(nro)
    for name in ("game.love", "rillwright.nro"):
        (stage / name).replace(destination / name)
print(destination)

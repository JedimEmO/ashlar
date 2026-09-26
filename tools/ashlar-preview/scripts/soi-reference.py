#!/usr/bin/env python3
"""Render SOI's author-provided CobbleStone_01 maps under the Ashlar studio rig.

Requires Pillow and the freely downloadable Soi_CobblesStone_Metallic.zip from
https://soi.itch.io/substance-designer-material-pack-1 (CC0). No source pixels are
used by the Ashlar procedural graph. Run from any directory with the ZIP path.
"""
import hashlib
import io
import json
from pathlib import Path
import subprocess
import sys
import zipfile
from PIL import Image

ROOT = Path(__file__).resolve().parents[3]
archive = Path(sys.argv[1]).resolve()
assets = ROOT / 'target/soi-reference/assets'
out = assets / 'materials/library/soi-cobblestone'
out.mkdir(parents=True, exist_ok=True)
names = {suffix: f'CobbleStone_01_{suffix}.tga'
         for suffix in ('BC', 'H', 'NOpenGL', 'R', 'M', 'AO')}
with zipfile.ZipFile(archive) as package:
    data = {suffix: package.read(name) for suffix, name in names.items()}
images = {suffix: Image.open(io.BytesIO(value)) for suffix, value in data.items()}
assert all(image.size == (2048, 2048) for image in images.values())
images['BC'].save(out / 'base.png')
images['NOpenGL'].save(out / 'normal.png')
Image.merge('RGB', (images['AO'], images['R'], images['M'])).save(out / 'orm.png')
images['H'].point(lambda value: value * 257, 'I').save(out / 'height.png')
subprocess.run(['cargo', 'build', '-p', 'ashlar-preview', '--example',
                'material-swatch', '--offline'], cwd=ROOT, check=True)
capture = ROOT / 'target/soi-reference/soi-original.png'
subprocess.run([str(ROOT / 'target/debug/examples/material-swatch'),
                'library:soi-cobblestone', str(capture), 'sphere', str(assets), 'opengl'],
               cwd=ROOT, check=True, timeout=120)
metadata = {
    'source': 'https://soi.itch.io/substance-designer-material-pack-1',
    'archive': 'Soi_CobblesStone_Metallic.zip',
    'archive_sha256': hashlib.sha256(archive.read_bytes()).hexdigest(),
    'variant': 'CobbleStone_01',
    'source_files_sha256': {names[s]: hashlib.sha256(d).hexdigest()
                            for s, d in data.items()},
    'conversion': 'BC is sRGB; NOpenGL is linear; ORM packs AO/R/M. '
                  '8-bit height expands to 16-bit via multiplication by 257.',
    'height_scale_metres': 0.025,
    'capture_sha256': hashlib.sha256(capture.read_bytes()).hexdigest(),
    'note': 'Author exports, not executed locally. Designer build and export '
            'settings unknown. Display displacement scale matches Ashlar.'}
capture.with_name('soi-reference.json').write_text(json.dumps(metadata, indent=2) + '\n')

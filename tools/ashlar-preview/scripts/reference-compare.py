#!/usr/bin/env python3
"""Side-by-side crops of a reference material's maps against an Ashlar bake.

The reference is always on the left. This is the tool a fidelity pass is run
with: bake, run this, look at the crops, write down what differs, edit.

    reference-compare.py soi-cobblestone [bake-dir] [out-dir] [--publish]
    reference-compare.py brick           [bake-dir] [out-dir] [--publish]

bake-dir defaults to the content step's export at 2048 with PNGs:

    cargo run --release -p ashlar-showcase --example export-materials -- assets 2048 <name> png

which writes assets/materials/library/<name>/ (ignored by git). The reference
maps are not committed either; docs/material-references/<name>/README.md says
where they come from and where this script expects them. --publish also writes
the full-view base and height pairs as JPEGs beside the crops, for a write-up.
Requires Pillow and numpy.
"""
import argparse
from pathlib import Path

import numpy as np
from PIL import Image

Image.MAX_IMAGE_PIXELS = None
ROOT = Path(__file__).resolve().parents[3]
EXPORT = ROOT / 'assets/materials/library'
GAP = 8


def eight_bit(image):
    """Height PNGs are 16-bit; view them normalised to their own maximum."""
    data = np.asarray(image)
    if data.dtype == np.uint8:
        return image
    data = data.astype(np.float64)
    return Image.fromarray((data / data.max() * 255).astype(np.uint8))


def soi_reference():
    """SOI's exported 2048 maps, as soi-reference.py converts them."""
    root = ROOT / 'target/soi-reference/assets/materials/library/soi-cobblestone'
    orm = Image.open(root / 'orm.png').convert('RGB')
    return {'base': Image.open(root / 'base.png').convert('RGB'),
            'rough': orm.split()[1],
            'height': eight_bit(Image.open(root / 'height.png'))}


def brick_reference():
    """The left 1024 square of ambientCG's 2048 x 1024 Bricks097 2K-PNG maps."""
    root = ROOT / 'target/material-research/ambientcg'
    square = (0, 0, 1024, 1024)
    height = Image.open(root / 'Bricks097_2K-PNG_Displacement.png')
    if height.mode not in ('L', 'I;16', 'I'):
        height = height.convert('L')
    return {'base': Image.open(root / 'Bricks097_2K-PNG_Color.png').convert('RGB').crop(square),
            'rough': Image.open(root / 'Bricks097_2K-PNG_Roughness.png').convert('L').crop(square),
            'height': eight_bit(height).crop(square)}


# name -> (reference loader, crops as (suffix, box, scale))
REFERENCES = {
    'soi-cobblestone': (soi_reference, [
        ('full', (0, 0, 2048, 2048), 0.4),
        ('crop-a', (300, 300, 940, 940), 1),
        ('crop-b', (1200, 1000, 1840, 1640), 1),
        ('zoom2', (700, 1300, 1020, 1620), 2),
        ('zoom3', (560, 420, 770, 630), 3)]),
    'brick': (brick_reference, [
        ('full', (0, 0, 1024, 1024), 1),
        ('crop', (200, 300, 712, 812), 1),
        ('zoom3', (300, 400, 556, 656), 3)]),
}


def ours(directory, size):
    orm = Image.open(directory / 'orm.png').convert('RGB')
    maps = {'base': Image.open(directory / 'base.png').convert('RGB'),
            'rough': orm.split()[1],
            'height': eight_bit(Image.open(directory / 'height.png'))}
    return {k: v if v.size == size else v.resize(size, Image.LANCZOS) for k, v in maps.items()}


def pair(left, right, box, scale):
    a, b = left.crop(box), right.crop(box)
    sheet = Image.new(a.mode, (a.width * 2 + GAP, a.height), 255 if a.mode == 'L' else (255,) * 3)
    sheet.paste(a, (0, 0))
    sheet.paste(b, (a.width + GAP, 0))
    if scale != 1:
        sheet = sheet.resize((round(sheet.width * scale), round(sheet.height * scale)), Image.LANCZOS)
    return sheet


def main():
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('reference', choices=sorted(REFERENCES))
    parser.add_argument('bake', nargs='?', type=Path)
    parser.add_argument('out', nargs='?', type=Path)
    parser.add_argument('--publish', action='store_true')
    args = parser.parse_args()
    load, crops = REFERENCES[args.reference]
    reference = load()
    bake = args.bake or EXPORT / args.reference
    out = args.out or ROOT / 'target/reference-compare' / args.reference
    out.mkdir(parents=True, exist_ok=True)
    mine = ours(bake, reference['base'].size)
    for name in reference:
        for suffix, box, scale in crops:
            pair(reference[name], mine[name], box, scale).save(out / f'{name}-{suffix}.png')
    if args.publish:
        for name in ('base', 'height'):
            _, box, _ = crops[0]
            sheet = pair(reference[name], mine[name], box, 1)
            sheet.thumbnail((2056, 2056), Image.LANCZOS)
            sheet.convert('RGB').save(
                out / f'compare-{args.reference}-{name}.jpg', quality=88)
    print(out)


if __name__ == '__main__':
    main()

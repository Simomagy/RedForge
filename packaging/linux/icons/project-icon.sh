#!/bin/sh
# Renders the project icon (redforge-project.svg) into the PNGs next to it and
# the Windows ICO; run it after changing the SVG. Needs rsvg-convert, magick
# and python3. Below 32 px the "PROJ" label is only a smudge, so it's left out.
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
ico="$here/../../../crates/rf-app/assets/redforge-project.ico"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

sed '/<path id="label"/,/\/>/d' "$here/redforge-project.svg" >"$tmp/small.svg"
for size in 16 24 32 48 64 96 128 256 512; do
  svg="$here/redforge-project.svg"
  [ "$size" -lt 32 ] && svg="$tmp/small.svg"
  rsvg-convert -w "$size" -h "$size" "$svg" -o "$here/redforge-project-$size.png"
done

# Up to 128 px as bitmaps, 256 px as a PNG (as Windows wants it, and smaller).
magick $(for size in 16 24 32 48 64 96 128; do echo "$here/redforge-project-$size.png"; done) "$tmp/small.ico"
python3 -I - "$tmp/small.ico" "$here/redforge-project-256.png" "$ico" <<'EOF'
import struct, sys
small, png, out = sys.argv[1:]
data = open(small, 'rb').read()
count = struct.unpack_from('<H', data, 4)[0]
images = []
for i in range(count):
    w, h, colors, _, planes, bpp, size, offset = struct.unpack_from('<BBBBHHII', data, 6 + 16 * i)
    images.append((w, h, colors, planes, bpp, data[offset:offset + size]))
images.append((0, 0, 0, 1, 32, open(png, 'rb').read()))
head = struct.pack('<HHH', 0, 1, len(images))
body = b''
for w, h, colors, planes, bpp, image in images:
    head += struct.pack('<BBBBHHII', w, h, colors, 0, planes, bpp, len(image), 6 + 16 * len(images) + len(body))
    body += image
open(out, 'wb').write(head + body)
EOF

#!/usr/bin/env python3
"""Trace a METAFONT font into a Type 1 outline font (.pfb).

Minimal mftrace replacement with the CLI subset bundle_packages.py uses:

    mf_trace.py [--formats=pfb] [--noround] [--no-afm] FONT

It runs `mf` at 4000 dpi, reads the
generic-font file, traces every glyph bitmap with `potrace`, and writes
FONT.pfb into the current directory. The font is found through MFINPUTS.
Glyphs are named after their ASCII character (Adobe glyph names) where there
is one, `gNNN` otherwise, and the built-in encoding maps each code to its glyph.
"""
import os
import re
import struct
import subprocess
import sys
import tempfile

from fontTools.agl import UV2AGL

# METAFONT numerics stay below 4096, which caps the resolution.
PIXELS_PER_INCH = 4000
HIRES_MODE = (
    "mode_def hires = mode_param(pixels_per_inch, {ppi}); "
    "mode_param(blacker, 0); mode_param(fillin, 0); mode_param(o_correction, 0); enddef;"
)


def run_mf(font, workdir):
    """Run METAFONT; return the bytes of the GF file."""
    cmd = ["mf", f"\\{HIRES_MODE.format(ppi=PIXELS_PER_INCH)} mode=hires; mag:=1; nonstopmode; input {font}"]
    subprocess.run(cmd, cwd=workdir, check=True, capture_output=True)
    gfs = [f for f in os.listdir(workdir) if f.endswith("gf")]
    if len(gfs) != 1:
        raise RuntimeError(f"mf produced {len(gfs)} gf files for {font}")
    with open(os.path.join(workdir, gfs[0]), "rb") as fp:
        return fp.read()


def parse_gf(data):
    """Return (design_size, {code: (min_m, max_m, min_n, max_n, rows, tfm_width)}).

    rows maps a row number n to a list of black runs (m0, m1), m1 exclusive.
    """
    i = 0
    # postamble pointer: last 4+ bytes are 223 padding preceded by id byte (131)
    end = len(data)
    while data[end - 1] == 223:
        end -= 1
    post_ptr = struct.unpack(">i", data[end - 5:end - 1])[0]
    design_size = struct.unpack(">I", data[post_ptr + 5:post_ptr + 9])[0] / 2 ** 20
    widths = {}
    p = post_ptr + 37  # post, p, ds, cs, hppp, vppp, min_m, max_m, min_n, max_n
    while data[p] != 249:
        op = data[p]
        if op == 245:  # char_loc: c dx dy w p
            widths[data[p + 1]] = struct.unpack(">i", data[p + 10:p + 14])[0] / 2 ** 20
            p += 18
        elif op == 246:  # char_loc0: c dm w p
            widths[data[p + 1]] = struct.unpack(">i", data[p + 3:p + 7])[0] / 2 ** 20
            p += 11
        else:
            raise RuntimeError(f"unexpected postamble opcode {op}")
    glyphs = {}
    while i < post_ptr:
        op = data[i]
        if op == 247:  # pre
            i += 3 + data[i + 2]
            continue
        if op in (67, 68):  # boc / boc1
            if op == 67:
                c = struct.unpack(">i", data[i + 1:i + 5])[0]
                min_m, max_m, min_n, max_n = struct.unpack(">iiii", data[i + 9:i + 25])
                i += 25
            else:
                c = data[i + 1]
                dm, mx, dn, mn = data[i + 2], data[i + 3], data[i + 4], data[i + 5]
                min_m, max_m, min_n, max_n = mx - dm, mx, mn - dn, mn
                i += 6
            rows = {}
            row = max_n
            m = min_m
            paint = False
            while True:
                op = data[i]
                i += 1
                if op <= 66:
                    if op <= 63:
                        k = op
                    else:
                        nb = op - 63
                        k = int.from_bytes(data[i:i + nb], "big")
                        i += nb
                    if paint:
                        rows.setdefault(row, []).append((m, m + k))
                    m += k
                    paint = not paint
                elif op == 69:  # eoc
                    break
                elif 70 <= op <= 73:  # skip0..3
                    if op == 70:
                        skip = 0
                    else:
                        nb = op - 70
                        skip = int.from_bytes(data[i:i + nb], "big")
                        i += nb
                    row -= skip + 1
                    m = min_m
                    paint = False
                elif 74 <= op <= 238:  # new_row_n
                    row -= 1
                    m = min_m + (op - 74)
                    paint = True
                elif 239 <= op <= 242:
                    nb = op - 238
                    i += nb + int.from_bytes(data[i:i + nb], "big")
                elif op == 243:
                    i += 4
                elif op == 244:
                    pass
                else:
                    raise RuntimeError(f"bad GF opcode {op}")
            glyphs[c] = (min_m, max_m, min_n, max_n, rows, widths.get(c, 0.0))
            continue
        if op == 244:
            i += 1
            continue
        if 239 <= op <= 242:
            nb = op - 238
            i += 1 + nb + int.from_bytes(data[i + 1:i + 1 + nb], "big")
            continue
        if op == 243:
            i += 5
            continue
        raise RuntimeError(f"bad GF opcode {op} at {i}")
    return design_size, glyphs


def trace_bitmap(glyph, workdir):
    """Run potrace; return a list of closed contours of absolute points.

    Each contour is a list of segments ('l', (x, y)) or ('c', (x1, y1), (x2, y2), (x3, y3))
    plus the start point, in pixel units with y up and the origin at the GF origin.
    """
    min_m, max_m, min_n, max_n, rows, _ = glyph
    w = max_m - min_m
    h = max_n - min_n + 1  # row n covers [n, n+1)
    if w <= 0 or h <= 0 or not rows:
        return []
    stride = (w + 7) // 8
    img = bytearray(stride * h)
    for n, runs in rows.items():
        r = max_n - n
        if not 0 <= r < h:
            continue
        for m0, m1 in runs:
            for m in range(m0, m1):
                x = m - min_m
                if 0 <= x < w:
                    img[r * stride + x // 8] |= 0x80 >> (x % 8)
    pbm = os.path.join(workdir, "g.pbm")
    with open(pbm, "wb") as fp:
        fp.write(b"P4\n%d %d\n" % (w, h) + bytes(img))
    out = subprocess.run(
        ["potrace", "-b", "svg", "-u", "10", "-t", "2", "-a", "1.0", "-O", "0.2", "-o", "-", pbm],
        check=True, capture_output=True,
    ).stdout.decode()
    contours = []
    for d in re.findall(r'<path[^>]* d="([^"]*)"', out):
        toks = re.findall(r"[MmLlCcz]|-?\d+(?:\.\d+)?", d)
        k = 0
        cur = start = None
        segs = []
        cmd = None
        while k < len(toks):
            t = toks[k]
            if t.isalpha():
                cmd = t
                k += 1
                if cmd == "z":
                    contours.append((start, segs))
                    segs = []
                    cur = start
                continue
            nargs = {"M": 2, "m": 2, "L": 2, "l": 2, "C": 6, "c": 6}[cmd]
            a = [float(v) for v in toks[k:k + nargs]]
            k += nargs
            rel = cmd.islower()
            if cmd in "Mm":
                cur = (cur[0] + a[0], cur[1] + a[1]) if rel and cur else (a[0], a[1])
                start = cur
                cmd = "l" if rel else "L"  # implicit lineto after moveto
            elif cmd in "Ll":
                p = (cur[0] + a[0], cur[1] + a[1]) if rel else (a[0], a[1])
                segs.append(("l", p))
                cur = p
            else:
                pts = [
                    (cur[0] + a[2 * j], cur[1] + a[2 * j + 1]) if rel else (a[2 * j], a[2 * j + 1])
                    for j in range(3)
                ]
                segs.append(("c",) + tuple(pts))
                cur = pts[2]
    # potrace works in 10 units per pixel with y up from the bitmap bottom
    sc = 0.1
    res = []
    for start, segs in contours:
        def tx(p):
            return (min_m + p[0] * sc, min_n + p[1] * sc)
        s = tx(start)
        ss = []
        for seg in segs:
            ss.append((seg[0],) + tuple(tx(p) for p in seg[1:]))
        res.append((s, ss))
    return res


def enc_num(v):
    v = int(v)
    if -107 <= v <= 107:
        return bytes([v + 139])
    if 108 <= v <= 1131:
        v -= 108
        return bytes([247 + (v >> 8), v & 255])
    if -1131 <= v <= -108:
        v = -v - 108
        return bytes([251 + (v >> 8), v & 255])
    return b"\xff" + struct.pack(">i", v)


def charstring(width, contours):
    cs = bytearray()
    cs += enc_num(0) + enc_num(width) + bytes([13])  # hsbw
    cx = cy = 0
    for start, segs in contours:
        sx, sy = round(start[0]), round(start[1])
        cs += enc_num(sx - cx) + enc_num(sy - cy) + bytes([21])  # rmoveto
        cx, cy = sx, sy
        for seg in segs:
            if seg[0] == "l":
                x, y = round(seg[1][0]), round(seg[1][1])
                if (x, y) == (cx, cy):
                    continue
                cs += enc_num(x - cx) + enc_num(y - cy) + bytes([5])
                cx, cy = x, y
            else:
                pts = [(round(p[0]), round(p[1])) for p in seg[1:]]
                prev = (cx, cy)
                vals = []
                for p in pts:
                    vals += [p[0] - prev[0], p[1] - prev[1]]
                    prev = p
                cs += b"".join(enc_num(v) for v in vals) + bytes([8])
                cx, cy = pts[2]
        cs += bytes([9])  # closepath
    cs += bytes([14])
    return bytes(cs)


def encrypt(data, r, n=4):
    data = b"\0" * n + data
    out = bytearray()
    for b in data:
        c = b ^ (r >> 8)
        r = ((c + r) * 52845 + 22719) & 0xFFFF
        out.append(c)
    return bytes(out)


def glyph_name(code):
    if 33 <= code <= 126:
        return UV2AGL[code]
    return f"g{code}"


def make_font(font, design_size, glyphs, workdir):
    scale = 1000.0 / (design_size * PIXELS_PER_INCH / 72.27)  # pixels -> 1000-unit em
    chars = {}
    bbox = [10 ** 9, 10 ** 9, -10 ** 9, -10 ** 9]
    for code in sorted(glyphs):
        g = glyphs[code]
        width = round(g[5] * 1000.0)
        contours = trace_bitmap(g, workdir)
        if not contours:
            continue
        for start, segs in contours:
            for seg in [("l", start)] + segs:
                for p in seg[1:]:
                    bbox[0] = min(bbox[0], p[0] * scale)
                    bbox[1] = min(bbox[1], p[1] * scale)
                    bbox[2] = max(bbox[2], p[0] * scale)
                    bbox[3] = max(bbox[3], p[1] * scale)
        scaled = [
            ((s[0] * scale, s[1] * scale), [(sg[0],) + tuple((p[0] * scale, p[1] * scale) for p in sg[1:]) for sg in segs])
            for s, segs in contours
        ]
        chars[code] = (glyph_name(code), charstring(width, scaled))
    if not chars:
        raise RuntimeError(f"{font}: no glyph outlines")
    bbox = [int(bbox[0]) - 1, int(bbox[1]) - 1, int(bbox[2]) + 1, int(bbox[3]) + 1]

    head = [
        f"%!PS-AdobeFont-1.0: {font} 001.001",
        "11 dict begin",
        "/FontInfo 8 dict dup begin",
        "/version (001.001) readonly def",
        f"/Notice (Traced from the METAFONT source of {font} with mf and potrace) readonly def",
        f"/FullName ({font}) readonly def",
        f"/FamilyName ({font}) readonly def",
        "/Weight (Regular) readonly def",
        "/ItalicAngle 0 def",
        "/isFixedPitch false def",
        "end readonly def",
        f"/FontName /{font} def",
        "/FontType 1 def",
        "/PaintType 0 def",
        "/FontMatrix [0.001 0 0 0.001 0 0] readonly def",
        "/FontBBox {%d %d %d %d} readonly def" % tuple(bbox),
        "/Encoding 256 array",
        "0 1 255 {1 index exch /.notdef put} for",
    ]
    head += [f"dup {c} /{chars[c][0]} put" for c in sorted(chars)]
    head += ["readonly def", "currentdict end", "currentfile eexec", ""]
    clear = "\n".join(head).encode()

    def rd(name, cs):
        enc = encrypt(cs, 4330)
        return b"/%s %d RD " % (name.encode(), len(enc)) + enc + b" ND\n"

    private = bytearray()
    private += (
        b"dup /Private 8 dict dup begin\n"
        b"/RD {string currentfile exch readstring pop} executeonly def\n"
        b"/ND {noaccess def} executeonly def\n"
        b"/NP {noaccess put} executeonly def\n"
        b"/BlueValues [] def\n"
        b"/MinFeature {16 16} def\n"
        b"/password 5839 def\n"
        b"/lenIV 4 def\n"
    )
    subrs = [
        bytes([139 + 3, 139 + 0, 12, 16, 12, 17, 12, 17, 12, 33, 11]),  # 3 0 callothersubr pop pop setcurrentpoint return
        bytes([139 + 0, 139 + 1, 12, 16, 11]),  # 0 1 callothersubr return
        bytes([139 + 0, 139 + 2, 12, 16, 11]),  # 0 2 callothersubr return
        bytes([11]),  # return
    ]
    private += b"/Subrs %d array\n" % len(subrs)
    for k, s in enumerate(subrs):
        enc = encrypt(s, 4330)
        private += b"dup %d %d RD " % (k, len(enc)) + enc + b" NP\n"
    private += b"ND\n"
    private += b"2 index /CharStrings %d dict dup begin\n" % (len(chars) + 1)
    private += rd(".notdef", enc_num(0) + enc_num(250) + bytes([13, 14]))
    for c in sorted(chars):
        private += rd(chars[c][0], chars[c][1])
    private += b"end\nend\nreadonly put\nnoaccess put\ndup /FontName get exch definefont pop\nmark currentfile closefile\n"
    binary = encrypt(bytes(private), 55665)
    trailer = ("0" * 64 + "\n") * 8 + "cleartomark\n"
    out = bytearray()
    for kind, blob in ((1, clear), (2, binary), (1, trailer.encode())):
        out += b"\x80" + bytes([kind]) + struct.pack("<I", len(blob)) + blob
    out += b"\x80\x03"
    return bytes(out)


def main(argv):
    font = [a for a in argv if not a.startswith("--")]
    if len(font) != 1:
        raise SystemExit("usage: mf_trace.py [--formats=pfb] [--noround] [--no-afm] FONT")
    font = font[0]
    with tempfile.TemporaryDirectory() as workdir:
        gf = run_mf(font, workdir)
        ds, glyphs = parse_gf(gf)
        pfb = make_font(font, ds, glyphs, workdir)
    with open(f"{font}.pfb", "wb") as fp:
        fp.write(pfb)


if __name__ == "__main__":
    main(sys.argv[1:])

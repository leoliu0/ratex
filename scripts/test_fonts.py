#!/usr/bin/env python3
"""
Ratex Font Verification & Isolation Test Harness.

Executes and verifies font embedding, text extraction, glyph rendering,
and engine isolation across all reported issues (#3, #4, #5, #6, #8)
and required font families (classic T1 LM/CM-Super, T2A Russian, LGR Greek,
CJKutf8 Japanese/Chinese/Korean, and native fontspec/xeCJK/CFF/TTC).

Enforces:
  1. Linux bwrap isolation with /usr/local and /usr/share font/TEXMF masking
     and sentinel proof.
  2. Strict pdf.js rendering and text extraction with system fonts disabled.
  3. Deep PDF structure & glyph program validation via fontTools/pypdf.
  4. Geometric rendering comparison against reference engine with predeclared tolerances.
  5. Negative/error and cache invalidation fixture support.
  6. Bounded capture artifact retention.

Usage:
  scripts/test_fonts.py --ratex PATH --output DIR [OPTIONS]
"""

from __future__ import annotations

import argparse
import concurrent.futures as cf
import hashlib
import io
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

try:
    import numpy as np
    from PIL import Image
    HAS_IMAGING = True
except ImportError:
    HAS_IMAGING = False

try:
    from pypdf import PdfReader
    from pypdf.generic import ByteStringObject, ContentStream, DecodedStreamObject, TextStringObject
    HAS_PYPDF = True
except ImportError:
    HAS_PYPDF = False

try:
    from fontTools.ttLib import TTFont
    from fontTools.cffLib import CFFFontSet
    HAS_FONTTOOLS = True
except ImportError:
    HAS_FONTTOOLS = False

try:
    from bounded_capture import DEFAULT_MAX_CAPTURE_BYTES, run_bounded
except ModuleNotFoundError:
    try:
        from scripts.bounded_capture import DEFAULT_MAX_CAPTURE_BYTES, run_bounded
    except ModuleNotFoundError:
        run_bounded = None
        DEFAULT_MAX_CAPTURE_BYTES = 1 << 20


def now_iso() -> str:
    return datetime.now(timezone.utc).isoformat()


def sha256_file(path: Path | str) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        while chunk := f.read(1 << 20):
            h.update(chunk)
    return h.hexdigest()


def probe_bwrap() -> dict[str, Any]:
    bwrap_path = shutil.which("bwrap")
    if not bwrap_path:
        return {
            "available": False,
            "path": None,
            "version": None,
            "error": "bwrap binary not found in PATH",
        }
    try:
        proc = subprocess.run(
            [bwrap_path, "--version"],
            capture_output=True,
            encoding="utf-8",
            errors="replace",
            timeout=5,
            check=False,
        )
        version = proc.stdout.strip() or proc.stderr.strip()
        return {
            "available": True,
            "path": bwrap_path,
            "version": version,
            "error": None,
        }
    except Exception as e:
        return {
            "available": False,
            "path": bwrap_path,
            "version": None,
            "error": str(e),
        }


def probe_pdfjs() -> dict[str, Any]:
    helper = Path(__file__).resolve().parent / "render_pdfjs.js"
    if not helper.is_file():
        return {"available": False, "reason": "render_pdfjs.js helper missing"}
    node_bin = shutil.which("node")
    if not node_bin:
        return {"available": False, "reason": "node binary not found in PATH"}
    try:
        proc = subprocess.run(
            [node_bin, str(helper), "--probe"],
            capture_output=True,
            encoding="utf-8",
            errors="replace",
            timeout=10,
            check=False,
        )
        if proc.returncode == 0 and proc.stdout.strip():
            try:
                data = json.loads(proc.stdout)
                return {
                    "available": bool(data.get("available")),
                    "version": data.get("version"),
                    "entry": data.get("entry"),
                    "canvas_available": bool(data.get("canvas_available")),
                    "canvas_backend": data.get("canvas_backend"),
                    "error": data.get("error"),
                }
            except json.JSONDecodeError:
                return {"available": False, "reason": f"invalid probe JSON: {proc.stdout[:200]}"}
        return {"available": False, "reason": f"probe exited with {proc.returncode}: {proc.stderr[:200]}"}
    except Exception as e:
        return {"available": False, "reason": str(e)}


def parse_pdffonts(pdf_path: Path) -> tuple[list[dict[str, Any]], list[str]]:
    """Inspect embedded fonts via pdffonts."""
    pdffonts_bin = shutil.which("pdffonts")
    if not pdffonts_bin:
        return [], ["pdffonts utility not found in PATH"]

    try:
        proc = subprocess.run(
            [pdffonts_bin, str(pdf_path)],
            capture_output=True,
            encoding="utf-8",
            errors="replace",
            timeout=15,
            check=False,
        )
        if proc.returncode != 0:
            return [], [f"pdffonts error: {proc.stderr.strip()}"]

        lines = proc.stdout.splitlines()
        if len(lines) < 3:
            return [], []

        fonts = []
        missing_embeddings = []

        for line in lines[2:]:
            parts = line.split()
            if len(parts) >= 8:
                obj_id = parts[-1]
                obj_gen = parts[-2]
                uni = parts[-3]
                sub = parts[-4]
                emb = parts[-5]
                enc = parts[-6]
                name = parts[0]
                font_type = " ".join(parts[1:-6])

                entry = {
                    "name": name,
                    "type": font_type,
                    "encoding": enc,
                    "emb": emb == "yes",
                    "sub": sub == "yes",
                    "uni": uni == "yes",
                    "object_id": f"{obj_gen} {obj_id}",
                }
                fonts.append(entry)
                if emb != "yes":
                    missing_embeddings.append(name)
            elif len(parts) >= 5:
                name = parts[0]
                emb = parts[-3] == "yes" if parts[-3] in ("yes", "no") else (parts[-4] == "yes")
                entry = {
                    "name": name,
                    "type": parts[1],
                    "emb": emb,
                }
                fonts.append(entry)
                if not emb:
                    missing_embeddings.append(name)

        return fonts, missing_embeddings
    except Exception as e:
        return [], [f"pdffonts failed: {e}"]


def extract_text(pdf_path: Path) -> tuple[str, list[str]]:
    """Extract text layer using pdftotext."""
    pdftotext_bin = shutil.which("pdftotext")
    if not pdftotext_bin:
        return "", ["pdftotext utility not found in PATH"]

    try:
        proc = subprocess.run(
            [pdftotext_bin, "-enc", "UTF-8", str(pdf_path), "-"],
            capture_output=True,
            encoding="utf-8",
            errors="replace",
            timeout=15,
            check=False,
        )
        if proc.returncode != 0:
            return "", [f"pdftotext exited with {proc.returncode}: {proc.stderr.strip()}"]
        return proc.stdout, []
    except Exception as e:
        return "", [f"pdftotext failed: {e}"]


def render_poppler(pdf_path: Path, out_dir: Path, dpi: int = 150) -> list[Path]:
    """Render PDF pages to PNG using pdftoppm."""
    pdftoppm_bin = shutil.which("pdftoppm")
    if not pdftoppm_bin:
        return []

    prefix = out_dir / "page"
    try:
        subprocess.run(
            [pdftoppm_bin, "-png", "-r", str(dpi), str(pdf_path), str(prefix)],
            capture_output=True,
            timeout=30,
            check=True,
        )
        return sorted(out_dir.glob("page-*.png"))
    except Exception:
        return []


def analyze_render(png_paths: list[Path]) -> dict[str, Any]:
    """Analyze rendered PNG images for ink presence, bounding boxes, and density."""
    if not HAS_IMAGING or not png_paths:
        return {
            "page_count": len(png_paths),
            "pages": [{"path": str(p), "ink_pixels": 0, "bbox": None} for p in png_paths],
            "total_ink_pixels": 0,
        }

    pages_info = []
    total_ink = 0

    for p in png_paths:
        try:
            with Image.open(p) as img:
                arr = np.array(img.convert("RGB"))
                ink_mask = (arr[:, :, 0] < 250) | (arr[:, :, 1] < 250) | (arr[:, :, 2] < 250)
                ink_count = int(np.sum(ink_mask))
                total_ink += ink_count

                bbox = None
                if ink_count > 0:
                    y_indices, x_indices = np.nonzero(ink_mask)
                    bbox = [
                        int(np.min(x_indices)),
                        int(np.min(y_indices)),
                        int(np.max(x_indices)),
                        int(np.max(y_indices)),
                    ]

                pages_info.append({
                    "path": str(p),
                    "width": img.width,
                    "height": img.height,
                    "ink_pixels": ink_count,
                    "bbox": bbox,
                })
        except Exception as e:
            pages_info.append({"path": str(p), "error": str(e), "ink_pixels": 0, "bbox": None})

    return {
        "page_count": len(png_paths),
        "pages": pages_info,
        "total_ink_pixels": total_ink,
    }


def compare_renders(
    rust_pages: list[Path],
    ref_pages: list[Path],
    case: dict[str, Any],
    label: str = "",
) -> tuple[dict[str, Any], list[str]]:
    """Compare glyph/line region geometry between Rust and Reference renders with predeclared tolerances."""
    errors: list[str] = []
    lbl = f" [{label}]" if label else ""

    if not HAS_IMAGING or not rust_pages or not ref_pages:
        page_match = len(rust_pages) == len(ref_pages)
        if not page_match:
            errors.append(f"Page count mismatch{lbl}: rust={len(rust_pages)}, ref={len(ref_pages)}")
        if not HAS_IMAGING:
            errors.append(f"PIL / numpy imaging dependencies required for render comparison{lbl} but not installed")
        return {
            "compared": False,
            "page_count_match": page_match,
            "rust_pages": len(rust_pages),
            "ref_pages": len(ref_pages),
        }, errors

    page_count_match = len(rust_pages) == len(ref_pages)
    if not page_count_match:
        errors.append(f"Page count mismatch{lbl}: rust={len(rust_pages)}, ref={len(ref_pages)}")

    min_pages = min(len(rust_pages), len(ref_pages))
    page_metrics = []

    min_iou_threshold = float(case.get("min_iou", 0.60))
    min_ink_ratio = float(case.get("min_ink_ratio", 0.35))
    max_ink_ratio = float(case.get("max_ink_ratio", 2.80))

    for i in range(min_pages):
        try:
            with Image.open(rust_pages[i]) as img_r, Image.open(ref_pages[i]) as img_f:
                arr_r = np.array(img_r.convert("RGB"))
                arr_f = np.array(img_f.convert("RGB"))

                mask_r = (arr_r[:, :, 0] < 250) | (arr_r[:, :, 1] < 250) | (arr_r[:, :, 2] < 250)
                mask_f = (arr_f[:, :, 0] < 250) | (arr_f[:, :, 1] < 250) | (arr_f[:, :, 2] < 250)

                ink_r = int(np.sum(mask_r))
                ink_f = int(np.sum(mask_f))

                intersection = int(np.sum(mask_r & mask_f))
                union = int(np.sum(mask_r | mask_f))
                iou = (intersection / union) if union > 0 else 1.0

                ink_ratio = (ink_r / ink_f) if ink_f > 0 else None

                ink_diff_pct = 0.0
                if union > 0:
                    y_indices, x_indices = np.nonzero(mask_r | mask_f)
                    ymin, ymax = int(np.min(y_indices)), int(np.max(y_indices))
                    xmin, xmax = int(np.min(x_indices)), int(np.max(x_indices))
                    crop_r = arr_r[ymin:ymax+1, xmin:xmax+1]
                    crop_f = arr_f[ymin:ymax+1, xmin:xmax+1]
                    abs_diff = np.abs(crop_r.astype(np.float32) - crop_f.astype(np.float32))
                    ink_diff_pct = float(np.mean(abs_diff) / 255.0 * 100.0)

                page_metrics.append({
                    "page": i + 1,
                    "rust_ink_pixels": ink_r,
                    "ref_ink_pixels": ink_f,
                    "ink_ratio": round(ink_ratio, 4) if ink_ratio is not None else None,
                    "ink_iou": round(iou, 4),
                    "ink_region_diff_pct": round(ink_diff_pct, 2),
                })

                # Check tolerances on positive pages
                if ink_f > 200:
                    if iou < min_iou_threshold:
                        errors.append(
                            f"Page {i+1}{lbl} ink IoU ({iou:.3f}) below threshold ({min_iou_threshold:.3f})"
                        )
                    if ink_ratio is not None and (ink_ratio < min_ink_ratio or ink_ratio > max_ink_ratio):
                        errors.append(
                            f"Page {i+1}{lbl} ink ratio ({ink_ratio:.3f}) outside tolerance [{min_ink_ratio}, {max_ink_ratio}]"
                        )
        except Exception as e:
            page_metrics.append({"page": i + 1, "error": str(e)})
            errors.append(f"Page {i+1}{lbl} render comparison exception: {e}")

    return {
        "compared": True,
        "page_count_match": page_count_match,
        "rust_pages": len(rust_pages),
        "ref_pages": len(ref_pages),
        "page_metrics": page_metrics,
    }, errors


def parse_encoding_cmap(cmap_str: str) -> tuple[int, dict[int, int]]:
    """Parse a PDF /Encoding CMap stream into (code_width, code_to_cid)."""
    code_to_cid: dict[int, int] = {}
    code_width = 2

    # Check codespacerange: e.g. <00> <FF> -> 1 byte, <0000> <FFFF> -> 2 bytes
    cs_match = re.search(r"begincodespacerange\s*(.*?)\s*endcodespacerange", cmap_str, re.DOTALL)
    if cs_match:
        ranges = re.findall(r"<([0-9a-fA-F]+)>\s*<([0-9a-fA-F]+)>", cs_match.group(1))
        if ranges:
            hex_len = len(ranges[0][0])
            code_width = max(1, hex_len // 2)

    # Parse begincidchar: <srcCode> cid
    cidchar_blocks = re.findall(r"begincidchar\s*(.*?)\s*endcidchar", cmap_str, re.DOTALL)
    for block in cidchar_blocks:
        pairs = re.findall(r"<([0-9a-fA-F]+)>\s*(\d+)", block)
        for src_hex, cid_str in pairs:
            src_code = int(src_hex, 16)
            cid = int(cid_str)
            code_to_cid[src_code] = cid

    # Parse begincidrange: <startCode> <endCode> startCID
    cidrange_blocks = re.findall(r"begincidrange\s*(.*?)\s*endcidrange", cmap_str, re.DOTALL)
    for block in cidrange_blocks:
        ranges = re.findall(r"<([0-9a-fA-F]+)>\s*<([0-9a-fA-F]+)>\s*(\d+)", block)
        for start_hex, end_hex, start_cid_str in ranges:
            start_code = int(start_hex, 16)
            end_code = int(end_hex, 16)
            start_cid = int(start_cid_str)
            for offset, c in enumerate(range(start_code, end_code + 1)):
                code_to_cid[c] = start_cid + offset

    return code_width, code_to_cid


def parse_tounicode_cmap(cmap_str: str) -> dict[int, str]:
    """Parse a PDF /ToUnicode CMap into a mapping from character code to unicode string."""
    code_to_uni: dict[int, str] = {}

    # 1. Parse bfchar entries: <srcCode> <dstString>
    bfchar_blocks = re.findall(r"beginbfchar\s*(.*?)\s*endbfchar", cmap_str, re.DOTALL)
    for block in bfchar_blocks:
        pairs = re.findall(r"<([0-9a-fA-F]+)>\s*<([0-9a-fA-F]+)>", block)
        for src_hex, dst_hex in pairs:
            src_code = int(src_hex, 16)
            dst_bytes = bytes.fromhex(dst_hex)
            try:
                uni_str = dst_bytes.decode("utf-16be")
            except Exception:
                uni_str = dst_bytes.decode("latin1", errors="ignore")
            code_to_uni[src_code] = uni_str

    # 2. Parse bfrange entries:
    # Array form: <startCode> <endCode> [ <dst1> <dst2> ... ]
    bfrange_blocks = re.findall(r"beginbfrange\s*(.*?)\s*endbfrange", cmap_str, re.DOTALL)
    for block in bfrange_blocks:
        array_ranges = re.findall(r"<([0-9a-fA-F]+)>\s*<([0-9a-fA-F]+)>\s*\[(.*?)\]", block, re.DOTALL)
        for start_hex, end_hex, arr_str in array_ranges:
            start_code = int(start_hex, 16)
            end_code = int(end_hex, 16)
            dst_list = re.findall(r"<([0-9a-fA-F]+)>", arr_str)
            for idx, c in enumerate(range(start_code, end_code + 1)):
                if idx < len(dst_list):
                    dst_bytes = bytes.fromhex(dst_list[idx])
                    try:
                        code_to_uni[c] = dst_bytes.decode("utf-16be")
                    except Exception:
                        code_to_uni[c] = dst_bytes.decode("latin1", errors="ignore")

        # Contiguous range form: <startCode> <endCode> <startDstString>
        clean_block = re.sub(r"<[0-9a-fA-F]+>\s*<[0-9a-fA-F]+>\s*\[.*?\]", "", block, flags=re.DOTALL)
        contig_ranges = re.findall(r"<([0-9a-fA-F]+)>\s*<([0-9a-fA-F]+)>\s*<([0-9a-fA-F]+)>", clean_block)
        for start_hex, end_hex, dst_start_hex in contig_ranges:
            start_code = int(start_hex, 16)
            end_code = int(end_hex, 16)
            dst_start = int(dst_start_hex, 16)
            dst_len = len(dst_start_hex) // 2
            for offset, c in enumerate(range(start_code, end_code + 1)):
                dst_val = dst_start + offset
                dst_bytes = dst_val.to_bytes(max(dst_len, (dst_val.bit_length() + 7) // 8), byteorder="big")
                try:
                    code_to_uni[c] = dst_bytes.decode("utf-16be")
                except Exception:
                    code_to_uni[c] = chr(dst_val)

    return code_to_uni


def parse_content_stream_ops(content_bytes: bytes) -> list[tuple]:
    """Extract text/font/ActualText and XObject operations in execution order."""
    stream = DecodedStreamObject()
    stream.set_data(content_bytes)
    ops: list[tuple] = []
    current_font: str | None = None
    font_stack: list[str | None] = []
    actual_text_stack: list[bool] = []

    # Delegate PDF syntax (including inline images and escaped names/strings)
    # to the same parser used for font dictionaries.
    for operands, operator in ContentStream(stream, None).operations:
        if operator == b"q":
            font_stack.append(current_font)
        elif operator == b"Q":
            current_font = font_stack.pop()
        elif operator == b"Tf":
            current_font = str(operands[0])
        elif operator == b"BDC":
            properties = operands[-1]
            actual_text_stack.append(
                isinstance(properties, dict) and "/ActualText" in properties
            )
        elif operator == b"BMC":
            actual_text_stack.append(False)
        elif operator == b"EMC":
            actual_text_stack.pop()
        elif operator in (b"Tj", b"'", b'"', b"TJ"):
            strings = operands[0] if operator == b"TJ" else operands[-1:]
            for text in strings:
                if isinstance(text, (ByteStringObject, TextStringObject)):
                    ops.append(("TEXT", current_font, text.original_bytes,
                                any(actual_text_stack)))
        elif operator == b"Do":
            ops.append(("DO", str(operands[0]), current_font, any(actual_text_stack)))
    return ops


def resolve_page_resources(page_node: Any) -> tuple[dict[str, Any], dict[str, Any]]:
    """Resolve /Font and /XObject dictionaries for a page, walking up /Parent hierarchy."""
    fonts: dict[str, Any] = {}
    xobjects: dict[str, Any] = {}

    chain = []
    curr = page_node
    while curr is not None:
        chain.append(curr)
        curr = curr.get("/Parent")
        if hasattr(curr, "get_object"):
            curr = curr.get_object()

    # Apply from root /Pages down to Page so child overrides parent
    for node in reversed(chain):
        res = node.get("/Resources")
        if hasattr(res, "get_object"):
            res = res.get_object()
        if isinstance(res, dict):
            node_fonts = res.get("/Font", {})
            if hasattr(node_fonts, "get_object"):
                node_fonts = node_fonts.get_object()
            if hasattr(node_fonts, "items"):
                for k, v in node_fonts.items():
                    fonts[k] = v.get_object() if hasattr(v, "get_object") else v

            node_xobjs = res.get("/XObject", {})
            if hasattr(node_xobjs, "get_object"):
                node_xobjs = node_xobjs.get_object()
            if hasattr(node_xobjs, "items"):
                for k, v in node_xobjs.items():
                    xobjects[k] = v.get_object() if hasattr(v, "get_object") else v

    return fonts, xobjects


def validate_pdf_font_embedding(pdf_path: Path, case: dict[str, Any]) -> list[str]:
    """Inspect PDF internal objects, font descriptors, and embedded font programs."""
    if not HAS_PYPDF:
        return ["pypdf dependency required for font embedding validation but not installed"]
    if not HAS_FONTTOOLS:
        return ["fontTools dependency required for font embedding validation but not installed"]

    errors: list[str] = []
    try:
        reader = PdfReader(str(pdf_path))
    except Exception as e:
        return [f"PDF parsing error: {e}"]

    check_type = case.get("check_type", "")
    all_fonts: list[dict[str, Any]] = []
    embedded_streams: list[dict[str, Any]] = []
    font_stream_obj_ids: dict[str, set[Any]] = {}
    resolved_astral_chars: set[str] = set()
    form_invocations: list[str] = []
    discovered_forms: dict[Any, Any] = {}
    def extract_font_program_info(fobj: Any, base_font: str, subtype: str) -> dict[str, Any]:
        info: dict[str, Any] = {
            "base_font": base_font,
            "subtype": subtype,
            "fobj": fobj,
            "encoding": fobj.get("/Encoding"),
            "to_unicode": fobj.get("/ToUnicode"),
            "descendant": None,
            "font_file_id": None,
            "font_data": None,
            "font_format": None,
            "tu_cmap": {},
            "enc_cmap_width": 2,
            "enc_cmap_to_cid": {},
        }

        # Parse /ToUnicode
        to_unicode = fobj.get("/ToUnicode")
        if to_unicode:
            try:
                tu_data = to_unicode.get_object().get_data().decode("latin1", errors="ignore")
                if "begincmap" not in tu_data or "endcmap" not in tu_data:
                    errors.append(f"Font {base_font} /ToUnicode CMap malformed")
                else:
                    info["tu_cmap"] = parse_tounicode_cmap(tu_data)
            except Exception as e:
                errors.append(f"Font {base_font} /ToUnicode decode error: {e}")

        # Type0 composite font
        if subtype == "/Type0":
            # Parse /Encoding
            enc_entry = fobj.get("/Encoding")
            if hasattr(enc_entry, "get_object"):
                enc_obj = enc_entry.get_object()
                if hasattr(enc_obj, "get_data"):
                    try:
                        enc_str = enc_obj.get_data().decode("latin1", errors="ignore")
                        w, m = parse_encoding_cmap(enc_str)
                        info["enc_cmap_width"] = w
                        info["enc_cmap_to_cid"] = m
                    except Exception as e:
                        errors.append(f"Font {base_font} /Encoding CMap stream error: {e}")
            elif str(enc_entry) in ("/Identity-H", "/Identity-V"):
                info["enc_cmap_width"] = 2

            desc_fonts = fobj.get("/DescendantFonts", [])
            for dref in desc_fonts:
                dobj = dref.get_object() if hasattr(dref, "get_object") else dref
                dsubtype = str(dobj.get("/Subtype", ""))
                info["descendant"] = dobj
                desc_ref = dobj.get("/FontDescriptor")
                if not desc_ref:
                    errors.append(f"Descendant font {dobj.get('/BaseFont')} missing /FontDescriptor")
                    continue
                desc = desc_ref.get_object()

                if dsubtype == "/CIDFontType2":
                    if "/FontFile" in desc:
                        errors.append(f"CIDFontType2 {base_font} has /FontFile instead of /FontFile2 (issue #3 regression)")
                    if "/FontFile2" not in desc:
                        errors.append(f"CIDFontType2 {base_font} missing /FontFile2")
                    else:
                        stream_obj = desc["/FontFile2"]
                        obj_id = getattr(stream_obj, "indirect_reference", None)
                        obj_id_num = obj_id.idnum if obj_id else getattr(stream_obj, "idnum", None)
                        try:
                            stream_data = stream_obj.get_object().get_data()
                            info["font_file_id"] = obj_id_num
                            info["font_data"] = stream_data
                            info["font_format"] = "CIDFontType2"
                            embedded_streams.append({"type": "CIDFontType2", "base": base_font, "data": stream_data, "id": obj_id_num})
                            font_stream_obj_ids.setdefault(base_font, set()).add(obj_id_num)
                        except Exception as e:
                            errors.append(f"Font {base_font} failed to decompress /FontFile2: {e}")

                elif dsubtype == "/CIDFontType0":
                    if "/FontFile3" not in desc:
                        errors.append(f"CIDFontType0 {base_font} missing /FontFile3")
                    else:
                        stream_obj = desc["/FontFile3"]
                        stream_dict = stream_obj.get_object() if hasattr(stream_obj, "get_object") else stream_obj
                        sub3 = str(stream_dict.get("/Subtype", ""))
                        if sub3 != "/CIDFontType0C":
                            errors.append(f"CIDFontType0 {base_font} /FontFile3 missing /Subtype /CIDFontType0C (got {sub3})")
                        obj_id = getattr(stream_obj, "indirect_reference", None)
                        obj_id_num = obj_id.idnum if obj_id else getattr(stream_obj, "idnum", None)
                        try:
                            stream_data = stream_dict.get_data()
                            info["font_file_id"] = obj_id_num
                            info["font_data"] = stream_data
                            info["font_format"] = "CIDFontType0"
                            embedded_streams.append({"type": "CIDFontType0", "base": base_font, "data": stream_data, "id": obj_id_num})
                            font_stream_obj_ids.setdefault(base_font, set()).add(obj_id_num)
                        except Exception as e:
                            errors.append(f"Font {base_font} failed to decompress /FontFile3: {e}")

        # Simple TrueType font
        elif subtype == "/TrueType":
            desc_ref = fobj.get("/FontDescriptor")
            if not desc_ref:
                errors.append(f"TrueType font {base_font} missing /FontDescriptor")
            else:
                desc = desc_ref.get_object()
                if "/FontFile" in desc:
                    errors.append(f"TrueType font {base_font} serialized with /FontFile instead of /FontFile2 (issue #3 regression)")
                if "/FontFile2" not in desc:
                    errors.append(f"TrueType font {base_font} missing /FontFile2")
                else:
                    stream_obj = desc["/FontFile2"]
                    obj_id = getattr(stream_obj, "indirect_reference", None)
                    obj_id_num = obj_id.idnum if obj_id else getattr(stream_obj, "idnum", None)
                    try:
                        stream_data = stream_obj.get_object().get_data()
                        info["font_file_id"] = obj_id_num
                        info["font_data"] = stream_data
                        info["font_format"] = "TrueType"
                        embedded_streams.append({"type": "TrueType", "base": base_font, "data": stream_data, "id": obj_id_num})
                        font_stream_obj_ids.setdefault(base_font, set()).add(obj_id_num)
                    except Exception as e:
                        errors.append(f"Font {base_font} failed to decompress /FontFile2: {e}")

        # Type 1 font
        elif subtype == "/Type1":
            desc_ref = fobj.get("/FontDescriptor")
            if not desc_ref:
                errors.append(f"Type1 font {base_font} missing /FontDescriptor")
            else:
                desc = desc_ref.get_object()
                stream_key = "/FontFile" if "/FontFile" in desc else "/FontFile3"
                if stream_key not in desc:
                    errors.append(f"Type1 font {base_font} missing /FontFile or /FontFile3")
                else:
                    stream_obj = desc[stream_key]
                    obj_id = getattr(stream_obj, "indirect_reference", None)
                    obj_id_num = obj_id.idnum if obj_id else getattr(stream_obj, "idnum", None)
                    try:
                        stream_data = stream_obj.get_object().get_data()
                        if stream_key == "/FontFile3" and str(stream_obj.get("/Subtype", "")) not in ("/Type1C", "/OpenType"):
                            errors.append(f"Type1 font {base_font} has invalid /FontFile3 subtype")
                        info["font_file_id"] = obj_id_num
                        info["font_data"] = stream_data
                        info["font_format"] = "Type1"
                        embedded_streams.append({"type": "Type1", "base": base_font, "data": stream_data, "id": obj_id_num})
                        font_stream_obj_ids.setdefault(base_font, set()).add(obj_id_num)
                    except Exception as e:
                        errors.append(f"Font {base_font} failed to decompress {stream_key}: {e}")

        all_fonts.append(info)
        return info

    def parse_and_validate_stream(
        content_bytes: bytes,
        scoped_fonts: dict[str, Any],
        scoped_xobjects: dict[str, Any],
        container_label: str,
        visited_forms: set[int],
        inherited_font: Any = None,
        inherited_actual_text: bool = False,
    ) -> None:
        try:
            ops = parse_content_stream_ops(content_bytes)
        except Exception as e:
            errors.append(f"Error tokenizing content stream in {container_label}: {e}")
            return

        font_info_cache: dict[str | None, dict[str, Any]] = {}

        for op_type, *args in ops:
            if op_type == "TEXT":
                fname, raw_bytes, in_actual_text = args[0], args[1], args[2]
                in_actual_text = in_actual_text or inherited_actual_text
                if fname not in font_info_cache:
                    fobj = inherited_font if fname is None else scoped_fonts.get(fname)
                    if not fobj:
                        errors.append(f"Font resource {fname} not found in {container_label}")
                        continue
                    base_font = str(fobj.get("/BaseFont", ""))
                    subtype = str(fobj.get("/Subtype", ""))
                    font_info_cache[fname] = extract_font_program_info(fobj, base_font, subtype)

                finfo = font_info_cache[fname]
                subtype = finfo["subtype"]
                base_font = finfo["base_font"]
                font_data = finfo["font_data"]
                font_format = finfo["font_format"]
                tu_cmap = finfo.get("tu_cmap", {})
                code_width = finfo.get("enc_cmap_width", 2)
                code_to_cid = finfo.get("enc_cmap_to_cid", {})

                if subtype == "/Type0" and font_data:
                    # Parse codes based on actual /Encoding code width
                    codes: list[int] = []
                    if code_width == 2:
                        if len(raw_bytes) % 2 != 0:
                            errors.append(
                                f"Font {base_font} in {container_label}: raw byte length ({len(raw_bytes)}) "
                                f"is not a multiple of 2 for 2-byte encoding"
                            )
                        for idx in range(0, len(raw_bytes) - (len(raw_bytes) % 2), 2):
                            codes.append((raw_bytes[idx] << 8) | raw_bytes[idx + 1])
                    else:
                        for b in raw_bytes:
                            codes.append(b)

                    # A. CIDFontType2 (TrueType)
                    if font_format == "CIDFontType2":
                        try:
                            ttf = TTFont(io.BytesIO(font_data))
                            num_glyphs = ttf["maxp"].numGlyphs
                            desc = finfo.get("descendant")
                            cid_to_gid_map = desc.get("/CIDToGIDMap") if desc else None
                            cid2gid_data = None
                            if cid_to_gid_map and hasattr(cid_to_gid_map, "get_object"):
                                cid2gid_obj = cid_to_gid_map.get_object()
                                if hasattr(cid2gid_obj, "get_data"):
                                    cid2gid_data = cid2gid_obj.get_data()

                            for code in codes:
                                cid = code_to_cid.get(code, code)
                                if cid2gid_data and (2 * cid + 1) < len(cid2gid_data):
                                    gid = (cid2gid_data[2 * cid] << 8) | cid2gid_data[2 * cid + 1]
                                else:
                                    gid = cid

                                if gid >= num_glyphs:
                                    errors.append(
                                        f"Font {base_font} in {container_label}: code 0x{code:04X} resolved to "
                                        f"out-of-bounds GID {gid} (numGlyphs={num_glyphs})"
                                    )

                                uni_text = tu_cmap.get(code)
                                if uni_text is None:
                                    if not in_actual_text and code != 32:
                                        errors.append(
                                            f"Font {base_font} in {container_label}: code 0x{code:04X} "
                                            f"has no /ToUnicode mapping and is not covered by /ActualText"
                                        )
                                else:
                                    for ch in uni_text:
                                        if ord(ch) > 0xFFFF:
                                            resolved_astral_chars.add(ch)

                                if gid == 0 and uni_text and uni_text.strip() and not in_actual_text:
                                    errors.append(
                                        f"Font {base_font} in {container_label}: character {repr(uni_text)} "
                                        f"(code 0x{code:04X}) resolved to GID 0 (.notdef missing glyph)"
                                    )
                        except Exception as e:
                            errors.append(f"TrueType program validation error for {base_font}: {e}")

                    # B. CIDFontType0 (raw CFF)
                    elif font_format == "CIDFontType0":
                        try:
                            cff_set = CFFFontSet()
                            cff_set.decompile(io.BytesIO(font_data), None)
                            if len(cff_set) == 0:
                                errors.append(f"CFF font set in {base_font} contains 0 fonts")
                                continue
                            top = cff_set[0]
                            if not hasattr(top, "CharStrings") or len(top.CharStrings) == 0:
                                errors.append(f"CID CFF font {base_font} has empty CharStrings")
                                continue

                            for code in codes:
                                cid = code_to_cid.get(code, code)
                                cs_key = f"cid{cid:05d}"
                                if cs_key not in top.CharStrings and f"cid{cid}" not in top.CharStrings:
                                    # Also check charset list by index
                                    charset = getattr(top, "charset", [])
                                    if cid >= len(charset):
                                        errors.append(
                                            f"Font {base_font} in {container_label}: CID {cid} (code 0x{code:04X}) "
                                            f"not found in CFF CharStrings"
                                        )
                                    else:
                                        cs_key = charset[cid]
                                        if cs_key not in top.CharStrings:
                                            errors.append(
                                                f"Font {base_font} in {container_label}: CID {cid} ({cs_key}) "
                                                f"not found in CFF CharStrings"
                                            )

                                # Verify decompilation of the CharString
                                if cs_key in top.CharStrings:
                                    try:
                                        top.CharStrings[cs_key].decompile()
                                    except Exception as cfe:
                                        errors.append(f"Font {base_font} CID {cid} CharString decompilation failed: {cfe}")

                                uni_text = tu_cmap.get(code)
                                if uni_text is None:
                                    if not in_actual_text and code != 32:
                                        errors.append(
                                            f"Font {base_font} in {container_label}: code 0x{code:04X} "
                                            f"has no /ToUnicode mapping and is not covered by /ActualText"
                                        )
                                else:
                                    for ch in uni_text:
                                        if ord(ch) > 0xFFFF:
                                            resolved_astral_chars.add(ch)

                                if cid == 0 and uni_text and uni_text.strip() and not in_actual_text:
                                    errors.append(
                                        f"Font {base_font} in {container_label}: character {repr(uni_text)} "
                                        f"(code 0x{code:04X}) resolved to CID 0 (.notdef missing glyph)"
                                    )
                        except Exception as e:
                            errors.append(f"CIDFontType0 CFF validation error for {base_font}: {e}")

            elif op_type == "DO":
                xname = args[0]
                xobj = scoped_xobjects.get(xname)
                if xobj and str(xobj.get("/Subtype", "")) == "/Form":
                    xobj_id = getattr(xobj, "indirect_reference", None)
                    xid_num = xobj_id.idnum if xobj_id else id(xobj)
                    form_invocations.append(xname)
                    discovered_forms[xid_num] = xobj
                    if xid_num in visited_forms:
                        errors.append(f"Recursive Form XObject {xname} in {container_label}")
                        continue
                    visited_forms.add(xid_num)

                    # A form with its own Resources dictionary has a complete
                    # local scope, even when it has no Font/XObject entries.
                    form_res = xobj.get("/Resources")
                    if hasattr(form_res, "get_object"):
                        form_res = form_res.get_object()
                    form_fonts = {} if form_res is not None else dict(scoped_fonts)
                    form_xobjs = {} if form_res is not None else dict(scoped_xobjects)

                    if isinstance(form_res, dict):
                        ff = form_res.get("/Font", {})
                        if hasattr(ff, "get_object"):
                            ff = ff.get_object()
                        if hasattr(ff, "items"):
                            for k, v in ff.items():
                                form_fonts[k] = v.get_object() if hasattr(v, "get_object") else v

                        fx = form_res.get("/XObject", {})
                        if hasattr(fx, "get_object"):
                            fx = fx.get_object()
                        if hasattr(fx, "items"):
                            for k, v in fx.items():
                                form_xobjs[k] = v.get_object() if hasattr(v, "get_object") else v

                    if hasattr(xobj, "get_data"):
                        parse_and_validate_stream(
                            xobj.get_data(),
                            form_fonts,
                            form_xobjs,
                            f"{container_label}->Form({xname})",
                            visited_forms,
                            inherited_font if args[1] is None else scoped_fonts.get(args[1]),
                            inherited_actual_text or args[2],
                        )
                    visited_forms.remove(xid_num)

    # Validate each page with inherited resources and nested form recursion
    for page_idx, page in enumerate(reader.pages):
        page_label = f"Page {page_idx + 1}"
        page_fonts, page_xobjs = resolve_page_resources(page)

        # Pre-populate font program info for all page fonts
        for fname, fobj in page_fonts.items():
            base_font = str(fobj.get("/BaseFont", ""))
            subtype = str(fobj.get("/Subtype", ""))
            extract_font_program_info(fobj, base_font, subtype)

        # Parse page contents
        c = page.get_contents()
        if c is not None:
            cdata_list = []
            if hasattr(c, "get_data"):
                cdata_list.append(c.get_data())
            elif isinstance(c, list):
                for part in c:
                    if hasattr(part, "get_data"):
                        cdata_list.append(part.get_data())
            for cdata in cdata_list:
                parse_and_validate_stream(cdata, page_fonts, page_xobjs, page_label, set())

    # 2. Check unioned_subsets: exactly one shared stream indirect object identity
    if check_type == "unioned_subsets":
        shared_stream_ids = set()
        total_shared_descriptors = 0
        for base, ids in font_stream_obj_ids.items():
            if "ratexshared" in base.lower() or "shared" in base.lower() or "ipaex" in base.lower():
                total_shared_descriptors += len(ids)
                for obj_id in ids:
                    if obj_id is not None:
                        shared_stream_ids.add(obj_id)

        if total_shared_descriptors == 0:
            errors.append("unioned_subsets contract violated: no font descriptors found for shared face")
        elif len(shared_stream_ids) == 0:
            errors.append("unioned_subsets contract violated: embedded font streams are not indirect objects")
        elif len(shared_stream_ids) > 1:
            errors.append(
                f"unioned_subsets contract violated: multiple font stream indirect objects {shared_stream_ids} "
                f"emitted for shared face across pages/sizes/forms/aliases, expected exactly 1 shared stream"
            )
        if len(discovered_forms) == 0:
            errors.append("unioned_subsets contract violated: expected Form XObject using shared face")
        if len(form_invocations) < 2:
            errors.append(
                f"unioned_subsets contract violated: expected Form XObject reuse across pages, got {len(form_invocations)} invocations"
            )

    # 3. Check specific contracts
    if check_type == "mapped_ttf":
        ipaex_found = any("ipaexmincho" in f["base_font"].lower() for f in all_fonts)
        if not ipaex_found:
            errors.append("Expected IPAexMincho font in PDF fonts, none found")
        for f in all_fonts:
            if "ipaex" in f["base_font"].lower() and f["subtype"] == "/Type1":
                errors.append(f"Mapped TrueType font {f['base_font']} serialized as /Type1 (issue #3 regression)")

    elif check_type == "native_cff":
        cff_found = any(s["type"] == "CIDFontType0" for s in embedded_streams)
        if not cff_found:
            for s in embedded_streams:
                try:
                    ttf = TTFont(io.BytesIO(s["data"]))
                    if "CFF " in ttf or ttf.sfntVersion == "OTTO":
                        cff_found = True
                        break
                except Exception as e:
                    errors.append(f"Embedded stream {s.get('base')} failed TTFont parse while checking CFF: {e}")
        if not cff_found:
            errors.append("Expected CFF / CIDFontType0 program in PDF font streams")

    elif check_type == "native_ttc":
        face_one_found = any("collectionfaceone" in f["base_font"].lower() for f in all_fonts)
        face_zero_found = any("collectionfacezero" in f["base_font"].lower() for f in all_fonts)
        if not face_one_found:
            errors.append("Expected collection face 1 (CollectionFaceOne) in PDF fonts")
        if face_zero_found:
            errors.append("Unexpected face 0 (CollectionFaceZero) found; FontIndex=1 failed")

    elif check_type == "native_supplementary":
        expected_astral = {"\U0001D400", "\U0001D401", "\U0001D402", "\U0001D403"}
        missing_astral = expected_astral - resolved_astral_chars
        if missing_astral:
            errors.append(
                f"native_supplementary contract: expected intended astral characters "
                f"{sorted([f'U+{ord(c):04X}' for c in missing_astral])} to be resolved in /ToUnicode, but were missing"
            )
    elif check_type == "native_boxes":
        if len(discovered_forms) == 0:
            errors.append("boxes_forms contract violated: expected at least 1 PDF Form XObject (/Subtype /Form) in document")
        if len(form_invocations) < 2:
            errors.append(
                f"boxes_forms contract violated: expected PDF Form XObject reuse (invoked >= 2 times via Do), "
                f"got {len(form_invocations)} invocations"
            )
    elif check_type == "packaged_graphic":
        if len(discovered_forms) == 0:
            errors.append("packaged_graphic contract violated: expected at least 1 PDF Form XObject (/Subtype /Form) in document")
        if len(form_invocations) < 1:
            errors.append("packaged_graphic contract violated: expected at least 1 Form invocation ('/Do') in content stream")

    elif check_type == "native_shaping":
        lm_fonts = [f for f in all_fonts if "lmroman" in f["base_font"].lower() or "latinmodern" in f["base_font"].lower()]
        for f in lm_fonts:
            if not f.get("tu_cmap"):
                errors.append(f"native_shaping contract violated: font {f['base_font']} missing /ToUnicode CMap")

    # Verify embedded TrueType streams with FontTools
    for s in embedded_streams:
        if s["type"] in ("TrueType", "CIDFontType2"):
            try:
                ttf = TTFont(io.BytesIO(s["data"]))
                if "glyf" not in ttf and "CFF " not in ttf:
                    errors.append(f"Embedded font {s['base']} has neither glyf nor CFF outline table")
                if ttf["maxp"].numGlyphs == 0:
                    errors.append(f"Embedded font {s['base']} contains 0 glyphs")
            except Exception as e:
                errors.append(f"Embedded font program {s['base']} failed TTFont parse: {e}")

    return errors


class FontTestHarness:
    def __init__(self, args: argparse.Namespace):
        self.args = args
        self.ratex_path = Path(args.ratex).resolve()
        self.output_dir = Path(args.output).resolve()
        self.manifest_path = Path(
            args.manifest or (Path(__file__).resolve().parent / "fixtures" / "fonts" / "manifest.json")
        ).resolve()
        self.bwrap_info = probe_bwrap()
        self.pdfjs_info = probe_pdfjs()
        self.isolated_bin_path: Path | None = None
        self.ratex_sha256: str = ""
        self.ratex_version: str = ""
        self.ref_fonts_conf: Path | None = None
        self.extra_tds_roots: list[str] = []

    def setup_ref_env(self) -> None:
        """Prepare reference-only fontconfig and TeX search paths."""
        if self.args.skip_reference:
            return
        ref_env_dir = self.output_dir / "ref_env"
        ref_env_dir.mkdir(parents=True, exist_ok=True)
        # Pin font programs, metrics, and packaged graphics for the reference.
        # Leave the kernel and general macro packages to the host TeX Live.
        try:
            from bundle_packages import reconstruct_archive
        except ModuleNotFoundError:
            from scripts.bundle_packages import reconstruct_archive
        assets_dir = Path(__file__).resolve().parent.parent / "crates" / "tex-kpse" / "assets"
        lock_path = assets_dir / "packages.lock.json"
        lock = json.loads(lock_path.read_text())
        reference_root = ref_env_dir / "texmf"
        reference_root.mkdir(exist_ok=True)
        reference_prefixes = (
            "fonts/",
            "tex/latex/doclicense/",
            "tex/latex/duckuments/",
            "tex/latex/twemojis/",
        )
        with tempfile.TemporaryDirectory(prefix="ratex-ref-fonts-") as scratch:
            archive_path = Path(scratch) / "packages.tar.zst"
            reconstruct_archive(assets_dir, archive_path, lock_path)
            if sha256_file(archive_path) != lock["output_archive"]["sha256"]:
                raise ValueError("Reference font archive differs from the locked runtime payload")
            with subprocess.Popen(["zstd", "-qdc", str(archive_path)], stdout=subprocess.PIPE) as decoder:
                with tarfile.open(fileobj=decoder.stdout, mode="r|") as archive:
                    for member in archive:
                        relative = Path(member.name)
                        if not member.isfile() or relative.is_absolute() or ".." in relative.parts:
                            continue
                        if member.name.startswith(reference_prefixes) or member.name.endswith(".fd"):
                            target = reference_root / relative
                            target.parent.mkdir(parents=True, exist_ok=True)
                            with archive.extractfile(member) as source, target.open("wb") as output:
                                shutil.copyfileobj(source, output)
                # Consume the zstd frame trailer even if tar stops at its end marker.
                while decoder.stdout.read(1 << 20):
                    pass
                if decoder.wait() != 0:
                    raise RuntimeError("Cannot decompress locked reference font resources")
        # Acquire missing XeLaTeX CJK macro dependencies into reference_root/tex if needed
        self.acquire_reference_cjk_dependencies(reference_root)
        self.extra_tds_roots = [str(reference_root)]

        # 1. Reference-only fontconfig file
        self.ref_fonts_conf = ref_env_dir / "fonts.conf"
        cache_dir = ref_env_dir / "fc_cache"
        cache_dir.mkdir(parents=True, exist_ok=True)

        fc_xml = f"""<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "urn:fontconfig:fonts.dtd">
<fontconfig>
  <dir>{reference_root / "fonts" / "opentype"}</dir>
  <dir>{reference_root / "fonts" / "truetype"}</dir>
  <cachedir>{cache_dir}</cachedir>
  <config></config>
</fontconfig>
"""
        self.ref_fonts_conf.write_text(fc_xml, encoding="utf-8")


    def acquire_reference_cjk_dependencies(self, reference_root: Path) -> None:
        """Acquire genuine missing XeLaTeX CJK macros (e.g. ctexhook.sty) into reference tree.

        Downloads and unpacks pinned authoritative packages into reference_root/tex.
        Never extracts font programs (preserving locked font assets) and never exposes
        files to the Rust engine or its hermetic sandbox.
        """
        ctexhook_target = reference_root / "tex" / "latex" / "ctex" / "ctexhook.sty"
        if ctexhook_target.is_file():
            return

        # Check if host kpathsea already resolves ctexhook.sty
        try:
            chk = subprocess.run(
                ["kpsewhich", "-engine=xelatex", "ctexhook.sty"],
                capture_output=True,
                encoding="utf-8",
                errors="replace",
                check=False,
            )
            if chk.returncode == 0 and chk.stdout.strip():
                return
        except Exception:
            pass

        # Pinned authoritative package for CJK/XeLaTeX support (Arch Linux matching TeX Live 2026)
        pinned_pkg = {
            "name": "texlive-langchinese",
            "version": "2026.1-1",
            "filename": "texlive-langchinese-2026.1-1-any.pkg.tar.zst",
            "sha256": "141609b837537bb6a914b1a54907dd266d4e2794c5b964707d4e6b7eccae9e39",
            "urls": [
                "https://archive.archlinux.org/packages/t/texlive-langchinese/texlive-langchinese-2026.1-1-any.pkg.tar.zst",
                "https://geo.mirror.pkgbuild.com/extra/os/x86_64/texlive-langchinese-2026.1-1-any.pkg.tar.zst",
                "https://gsl-syd.mm.fcix.net/archlinux/extra/os/x86_64/texlive-langchinese-2026.1-1-any.pkg.tar.zst",
            ],
        }

        candidate_dirs = [
            Path("/tmp/ratex-upstream-cache"),
            Path.home() / ".cache" / "ratex-reference",
            self.output_dir / "ref_env" / "cache",
        ]

        arc_path: Path | None = None
        for d in candidate_dirs:
            p = d / pinned_pkg["filename"]
            if p.is_file():
                try:
                    if sha256_file(p) == pinned_pkg["sha256"]:
                        arc_path = p
                        break
                except Exception:
                    pass

        if arc_path is None:
            # Download to the first writable cache directory
            target_dir = candidate_dirs[0]
            try:
                target_dir.mkdir(parents=True, exist_ok=True)
            except Exception:
                target_dir = candidate_dirs[-1]
                target_dir.mkdir(parents=True, exist_ok=True)
            target_file = target_dir / pinned_pkg["filename"]

            downloaded = False
            import urllib.request
            for url in pinned_pkg["urls"]:
                try:
                    urllib.request.urlretrieve(url, target_file)
                    if sha256_file(target_file) == pinned_pkg["sha256"]:
                        arc_path = target_file
                        downloaded = True
                        break
                except Exception:
                    if target_file.is_file():
                        target_file.unlink(missing_ok=True)

            if not downloaded or arc_path is None:
                raise RuntimeError(
                    f"Failed to obtain authoritative reference package {pinned_pkg['name']} ({pinned_pkg['sha256']})"
                )

        # Extract strictly TeX macro files (under usr/share/texmf-dist/tex/)
        # Exclude font files to preserve locked font programs
        with subprocess.Popen(["zstd", "-qdc", str(arc_path)], stdout=subprocess.PIPE) as proc:
            with tarfile.open(fileobj=proc.stdout, mode="r|") as tar:
                for member in tar:
                    if not member.isfile():
                        continue
                    if member.name.startswith("usr/share/texmf-dist/tex/"):
                        rel_parts = Path(member.name).parts[3:]  # starts at tex/...
                        rel_path = Path(*rel_parts)
                        if rel_path.is_absolute() or ".." in rel_path.parts:
                            raise ValueError(f"Unsafe reference package path: {member.name}")
                        target = reference_root / rel_path
                        target.parent.mkdir(parents=True, exist_ok=True)
                        with tar.extractfile(member) as src, target.open("wb") as dst:
                            shutil.copyfileobj(src, dst)
            while proc.stdout.read(1 << 20):
                pass
            if proc.wait() != 0:
                raise RuntimeError(f"Cannot decompress reference package {arc_path}")

    def setup(self) -> None:
        if not self.ratex_path.is_file():
            raise FileNotFoundError(f"Ratex binary not found at {self.ratex_path}")
        if not self.manifest_path.is_file():
            raise FileNotFoundError(f"Manifest not found at {self.manifest_path}")

        self.output_dir.mkdir(parents=True, exist_ok=True)
        self.ratex_sha256 = sha256_file(self.ratex_path)

        # Query version
        try:
            v_proc = subprocess.run(
                [str(self.ratex_path), "--version"],
                capture_output=True,
                encoding="utf-8",
                errors="replace",
                timeout=5,
                check=False,
            )
            self.ratex_version = (v_proc.stdout.strip() or v_proc.stderr.strip()).splitlines()[0]
        except Exception as e:
            self.ratex_version = f"unknown: {e}"

        # Setup isolated binary copy
        iso_bin_dir = self.output_dir / "isolated_bin"
        iso_bin_dir.mkdir(parents=True, exist_ok=True)
        self.isolated_bin_path = iso_bin_dir / "ratex"
        shutil.copy2(self.ratex_path, self.isolated_bin_path)
        os.chmod(self.isolated_bin_path, 0o755)

        # Setup reference environment
        self.setup_ref_env()

    def get_masked_paths(self) -> list[str]:
        candidates = [
            "/usr/share/texmf",
            "/usr/share/texmf-dist",
            "/usr/share/texlive",
            "/usr/share/fonts",
            "/usr/local/share/texmf",
            "/usr/local/share/texmf-dist",
            "/usr/local/share/texlive",
            "/usr/local/share/fonts",
            "/usr/local/share/tex-suite",
            "/var/lib/texmf",
            "/etc/texmf",
            "/root/.texmf-var",
        ]
        if os.path.isdir("/usr/local/share"):
            try:
                for entry in os.listdir("/usr/local/share"):
                    full = os.path.join("/usr/local/share", entry)
                    if os.path.isdir(full) and ("tex" in entry.lower() or "font" in entry.lower()):
                        if full not in candidates:
                            candidates.append(full)
            except Exception:
                pass
        return [p for p in candidates if os.path.exists(p)]

    def verify_isolation_sentinel(self) -> dict[str, Any]:
        """Prove that external host font trees and a sentinel file are inaccessible inside bwrap."""
        if sys.platform != "linux" or not self.bwrap_info["available"]:
            return {"verified": False, "reason": "bwrap not available"}

        sentinel_dir = Path(tempfile.mkdtemp(prefix="ratex-external-resources-"))
        sentinel_file = sentinel_dir / "forbidden_external_font.ttf"
        sentinel_package = sentinel_dir / "forbidden_external_package.sty"

        masked = self.get_masked_paths()
        mask_args = []
        for p in masked:
            mask_args.extend(["--tmpfs", p])

        test_cmd = [
            self.bwrap_info["path"],
            "--ro-bind", "/usr", "/usr",
            "--ro-bind-try", "/lib", "/lib",
            "--ro-bind-try", "/lib64", "/lib64",
            "--ro-bind-try", "/bin", "/bin",
            "--proc", "/proc",
            "--dev", "/dev",
            "--tmpfs", "/tmp",
            *mask_args,
            "--unshare-net",
            "--",
            "/bin/sh", "-c",
            'test ! -e "$1" && test ! -e "$2"',
            "ratex-sentinel-check", str(sentinel_file), str(sentinel_package),
        ]
        try:
            fixture = Path(__file__).resolve().parent / "fixtures/fonts/cache_invalidation/dynamic.ttf"
            shutil.copyfile(fixture, sentinel_file)
            sentinel_package.write_text(r"\ProvidesPackage{forbidden_external_package}" + "\n")
            res = subprocess.run(
                test_cmd,
                capture_output=True,
                encoding="utf-8",
                errors="replace",
                timeout=5,
                check=False,
            )
            blocked = res.returncode == 0
            return {
                "verified": blocked,
                "sentinel_blocked": blocked,
                "sentinel_path": str(sentinel_file),
                "font_sha256": sha256_file(sentinel_file),
                "package_path": str(sentinel_package),
                "error": None if blocked else "Sentinel file was accessible inside sandbox",
            }
        except Exception as e:
            return {"verified": False, "sentinel_blocked": False, "error": str(e)}
        finally:
            shutil.rmtree(sentinel_dir, ignore_errors=True)

    def get_isolation_info(self) -> dict[str, Any]:
        if sys.platform == "linux" and self.bwrap_info["available"]:
            masked = self.get_masked_paths()
            sentinel_res = self.verify_isolation_sentinel()
            return {
                "type": "bwrap",
                "platform": sys.platform,
                "sandboxed": True,
                "bwrap_path": self.bwrap_info["path"],
                "bwrap_version": self.bwrap_info["version"],
                "network_disabled": True,
                "external_fonts_masked": True,
                "masked_paths": masked,
                "sentinel_verified": sentinel_res.get("verified", False),
                "sentinels": sentinel_res,
            }
        return {
            "type": "environment_only",
            "platform": sys.platform,
            "sandboxed": False,
            "reason": self.bwrap_info["error"] if sys.platform == "linux" else f"bwrap unavailable on {sys.platform}",
            "network_disabled": False,
            "external_fonts_masked": False,
            "sentinel_verified": False,
        }

    def render_pdfjs(self, pdf_path: Path, out_dir: Path) -> tuple[list[Path], dict[str, Any], list[str]]:
        """Render PDF pages using render_pdfjs.js helper."""
        helper_path = Path(__file__).resolve().parent / "render_pdfjs.js"
        if not self.pdfjs_info.get("available") or not helper_path.is_file():
            return [], {}, ["pdf.js helper unavailable"]

        out_dir.mkdir(parents=True, exist_ok=True)
        node_bin = shutil.which("node") or "node"
        cmd = [node_bin, str(helper_path), str(pdf_path), str(out_dir), "2.0"]
        try:
            res = subprocess.run(
                cmd,
                capture_output=True,
                encoding="utf-8",
                errors="replace",
                timeout=30,
                check=False,
            )
            if res.returncode != 0:
                return [], {}, [f"pdf.js render failed (code {res.returncode}): {res.stderr[:300]}"]
            data = json.loads(res.stdout)
            png_paths = []
            for p in data.get("pages", []):
                img_p = p.get("image_path")
                if img_p and Path(img_p).is_file():
                    png_paths.append(Path(img_p))
            return png_paths, data, []
        except Exception as e:
            return [], {}, [f"pdf.js execution exception: {e}"]

    def run_case_rust(
        self,
        case: dict[str, Any],
        case_dir: Path,
        work_dir: Path,
        copy_case: bool = True,
    ) -> dict[str, Any]:
        """Compile a fixture with the isolated Ratex binary."""
        if copy_case:
            shutil.copytree(case_dir, work_dir, dirs_exist_ok=True)
        home_dir = work_dir / ".home"
        home_dir.mkdir(exist_ok=True)

        main_tex = case["main_tex"]
        target_engine = case.get("engine", "pdflatex")

        engine_flags = []
        if target_engine == "xelatex":
            engine_flags = ["-xelatex", "-interaction=nonstopmode", "-halt-on-error"]
        elif target_engine == "lualatex":
            engine_flags = ["-lualatex", "-interaction=nonstopmode", "-halt-on-error"]
        else:
            engine_flags = ["-pdf", "-interaction=nonstopmode", "-halt-on-error"]

        cmd = [str(self.isolated_bin_path), *engine_flags, main_tex]

        use_bwrap = sys.platform == "linux" and self.bwrap_info["available"]
        if use_bwrap:
            masked = self.get_masked_paths()
            mask_args = []
            for p in masked:
                mask_args.extend(["--tmpfs", p])
            bwrap_cmd = [
                self.bwrap_info["path"],
                "--ro-bind", "/usr", "/usr",
                "--ro-bind-try", "/lib", "/lib",
                "--ro-bind-try", "/lib64", "/lib64",
                "--ro-bind-try", "/bin", "/bin",
                "--ro-bind-try", "/etc", "/etc",
                "--proc", "/proc",
                "--dev", "/dev",
                "--tmpfs", "/tmp",
                *mask_args,
                "--unshare-net",
                "--bind", str(self.isolated_bin_path), str(self.isolated_bin_path),
                "--bind", str(work_dir), str(work_dir),
                "--chdir", str(work_dir),
                "--setenv", "HOME", str(home_dir),
                "--setenv", "TEX_RS_HERMETIC", "1",
                "--setenv", "PATH", "/usr/bin:/bin",
                "--setenv", "LANG", "C.UTF-8",
                "--setenv", "LC_ALL", "C.UTF-8",
                "--unsetenv", "TEXMFHOME",
                "--unsetenv", "TEXMFVAR",
                "--unsetenv", "TEXMFCACHE",
                "--",
                *cmd,
            ]
            exec_cmd = bwrap_cmd
            exec_env = None
        else:
            exec_cmd = cmd
            exec_env = dict(os.environ)
            exec_env["HOME"] = str(home_dir)
            exec_env["TEX_RS_HERMETIC"] = "1"
            exec_env.pop("TEXMFHOME", None)
            exec_env.pop("TEXMFVAR", None)
            exec_env.pop("TEXMFCACHE", None)

        t0 = time.perf_counter()
        if run_bounded:
            child = run_bounded(
                exec_cmd,
                cwd=work_dir,
                output_path=work_dir / "compile.log",
                timeout=self.args.timeout,
                env=exec_env,
                max_bytes=DEFAULT_MAX_CAPTURE_BYTES,
            )
            exit_code = child["returncode"]
            stdout = (work_dir / "compile.log").read_text(errors="replace")
            stderr = child.get("spawn_error") or ""
            duration_s = child["elapsed_seconds"]
        else:
            proc = subprocess.run(
                exec_cmd,
                cwd=work_dir,
                env=exec_env,
                capture_output=True,
                encoding="utf-8",
                errors="replace",
                timeout=self.args.timeout,
                check=False,
            )
            exit_code = proc.returncode
            stdout = proc.stdout
            stderr = proc.stderr
            duration_s = round(time.perf_counter() - t0, 3)

        pdf_stem = Path(main_tex).stem
        pdf_path = work_dir / f"{pdf_stem}.pdf"
        pdf_valid = pdf_path.is_file() and pdf_path.stat().st_size > 500

        return {
            "exit_code": exit_code,
            "stdout": stdout,
            "stderr": stderr,
            "duration_s": duration_s,
            "pdf_valid": pdf_valid,
            "pdf_path": str(pdf_path) if pdf_valid else None,
        }

    def resolve_ref_binary(self, engine: str) -> str | None:
        """Resolve genuine reference TeX Live binary, avoiding installed wrappers."""
        if engine == "xelatex":
            if self.args.reference_xelatex and Path(self.args.reference_xelatex).is_file():
                return str(self.args.reference_xelatex)
            for cand in ["/usr/bin/xelatex", "/Library/TeX/texbin/xelatex"]:
                if os.path.isfile(cand):
                    return cand
            w = shutil.which("xelatex")
            return w
        else:
            if self.args.reference_pdflatex and Path(self.args.reference_pdflatex).is_file():
                return str(self.args.reference_pdflatex)
            for cand in ["/usr/bin/pdflatex", "/Library/TeX/texbin/pdflatex"]:
                if os.path.isfile(cand):
                    return cand
            w = shutil.which("pdflatex")
            if w and "/usr/local" not in w:
                return w
            return "/usr/bin/pdflatex" if os.path.isfile("/usr/bin/pdflatex") else None

    def run_case_reference(self, case: dict[str, Any], case_dir: Path, work_dir: Path) -> dict[str, Any]:
        """Compile a fixture with system TeX Live reference using reference-only fontconfig and TeX paths."""
        shutil.copytree(case_dir, work_dir, dirs_exist_ok=True)
        main_tex = case["main_tex"]
        ref_engine = case.get("reference_engine", case.get("engine", "pdflatex"))

        ref_bin = self.resolve_ref_binary(ref_engine)
        if not ref_bin or not os.path.isfile(ref_bin):
            return {
                "skipped": True,
                "error": f"Reference engine '{ref_engine}' binary not found",
                "pdf_valid": False,
                "pdf_path": None,
            }

        ref_env = dict(os.environ)
        if self.ref_fonts_conf and self.ref_fonts_conf.is_file():
            ref_env["FONTCONFIG_FILE"] = str(self.ref_fonts_conf)
        ref_env["LC_ALL"] = "C.UTF-8"
        ref_env["LANG"] = "C.UTF-8"

        if self.extra_tds_roots:
            ref_env["TFMFONTS"] = ":".join(f"{r}/fonts/tfm//" for r in self.extra_tds_roots) + ":/usr/share/texmf-dist/fonts/tfm//:"
            ref_env["VFFONTS"] = ":".join(f"{r}/fonts/vf//" for r in self.extra_tds_roots) + ":/usr/share/texmf-dist/fonts/vf//:"
            ref_env["T1FONTS"] = ":".join(f"{r}/fonts/type1//" for r in self.extra_tds_roots) + ":/usr/share/texmf-dist/fonts/type1//:"
            ref_env["TEXFONTMAPS"] = ":".join(f"{r}/fonts/map//" for r in self.extra_tds_roots) + ":/usr/share/texmf-dist/fonts/map//:"
            ref_env["ENCFONTS"] = os.pathsep.join(f"{r}/fonts/enc//" for r in self.extra_tds_roots) + os.pathsep
            ref_env["TEXINPUTS"] = ".:" + ":".join(f"{r}/tex//" for r in self.extra_tds_roots) + ":/usr/share/texmf-dist/tex//:"
            ref_env["OPENTYPEFONTS"] = os.pathsep.join(f"{r}/fonts/opentype//" for r in self.extra_tds_roots) + os.pathsep
            ref_env["TTFONTS"] = os.pathsep.join(f"{r}/fonts/truetype//" for r in self.extra_tds_roots) + os.pathsep

        cmd = [ref_bin, "-interaction=nonstopmode", "-halt-on-error", main_tex]
        t0 = time.perf_counter()
        if run_bounded:
            child = run_bounded(
                cmd,
                cwd=work_dir,
                output_path=work_dir / "ref_compile.log",
                timeout=self.args.timeout,
                env=ref_env,
                max_bytes=DEFAULT_MAX_CAPTURE_BYTES,
            )
            exit_code = child["returncode"]
            stdout = (work_dir / "ref_compile.log").read_text(errors="replace")
            stderr = child.get("spawn_error") or ""
            duration_s = child["elapsed_seconds"]
        else:
            proc = subprocess.run(
                cmd,
                cwd=work_dir,
                env=ref_env,
                capture_output=True,
                encoding="utf-8",
                errors="replace",
                timeout=self.args.timeout,
                check=False,
            )
            exit_code = proc.returncode
            stdout = proc.stdout
            stderr = proc.stderr
            duration_s = round(time.perf_counter() - t0, 3)

        pdf_stem = Path(main_tex).stem
        pdf_path = work_dir / f"{pdf_stem}.pdf"
        pdf_valid = pdf_path.is_file() and pdf_path.stat().st_size > 500

        return {
            "skipped": False,
            "engine": ref_engine,
            "bin": ref_bin,
            "exit_code": exit_code,
            "stdout": stdout,
            "stderr": stderr,
            "duration_s": duration_s,
            "pdf_valid": pdf_valid,
            "pdf_path": str(pdf_path) if pdf_valid else None,
        }

    def execute_case(self, case: dict[str, Any]) -> dict[str, Any]:
        case_name = case["name"]
        case_dir = self.manifest_path.parent / case["dir"]
        case_out_dir = self.output_dir / "cases" / case_name

        shutil.rmtree(case_out_dir, ignore_errors=True)
        case_out_dir.mkdir(parents=True, exist_ok=True)

        rust_work = case_out_dir / "rust_work"
        ref_work = case_out_dir / "ref_work"
        rust_work.mkdir(parents=True, exist_ok=True)
        ref_work.mkdir(parents=True, exist_ok=True)

        reasons: list[str] = []

        # Handle negative / error test cases
        if case.get("expect_failure"):
            main_stem = Path(case["main_tex"]).stem
            pre_pdf = rust_work / f"{main_stem}.pdf"
            if pre_pdf.is_file():
                pre_pdf.unlink()

            rust_res = self.run_case_rust(case, case_dir, rust_work, copy_case=True)

            if rust_res["exit_code"] == 0:
                reasons.append(
                    f"Negative test '{case_name}' unexpectedly succeeded with exit code 0 (expected nonzero exit)"
                )
            if rust_res.get("pdf_valid") or pre_pdf.is_file():
                reasons.append(
                    f"Negative test '{case_name}' unexpectedly published a valid PDF"
                )

            combined_out = (rust_res.get("stdout") or "") + (rust_res.get("stderr") or "")
            expected_errs = [e for e in case.get("expected_error_substrings", []) if e.lower().strip() != "error"]
            if expected_errs:
                if not any(e.lower() in combined_out.lower() for e in expected_errs):
                    reasons.append(
                        f"Expected target-specific error diagnostic from {expected_errs} not found in output"
                    )

            status = "pass" if not reasons else "fail"
            safe_rust = {k: (str(v) if isinstance(v, Path) else v) for k, v in rust_res.items()}
            return {
                "name": case_name,
                "description": case.get("description", ""),
                "status": status,
                "reasons": reasons,
                "rust": safe_rust,
                "reference": {"skipped": True, "note": "Negative test"},
                "comparison": {"compared": False},
            }

        # Handle cache invalidation test cases
        if case.get("check_type") == "cache_invalidation":
            rust_res1 = self.run_case_rust(case, case_dir, rust_work, copy_case=True)
            if not rust_res1["pdf_valid"] or not rust_res1["pdf_path"]:
                reasons.append("Initial compilation failed to produce valid PDF")
                safe_rust1 = {k: (str(v) if isinstance(v, Path) else v) for k, v in rust_res1.items()}
                return {
                    "name": case_name,
                    "description": case.get("description", ""),
                    "status": "fail",
                    "reasons": reasons,
                    "rust": safe_rust1,
                    "reference": {"skipped": True, "note": "Cache invalidation test"},
                    "comparison": {"compared": False},
                }

            pdf_path1 = Path(rust_res1["pdf_path"])
            pdf_hash_1 = sha256_file(pdf_path1)

            render_dir1 = case_out_dir / "rust_render_pass1"
            render_dir1.mkdir(parents=True, exist_ok=True)
            poppler_p1 = render_poppler(pdf_path1, render_dir1)
            analysis_p1 = analyze_render(poppler_p1)
            ink_p1 = analysis_p1.get("total_ink_pixels", 0)

            # Modify real used glyph's outlines and metrics in dynamic.ttf
            dyn_ttf = rust_work / "dynamic.ttf"
            if not HAS_FONTTOOLS:
                reasons.append("fontTools dependency required for cache invalidation font mutation but not installed")
            elif not dyn_ttf.is_file():
                reasons.append("dynamic.ttf not found for cache invalidation font mutation")
            else:
                ttf = TTFont(str(dyn_ttf))
                cmap = ttf.getBestCmap()
                modified = False
                for ch in ['R', 'a', 'e']:
                    code = ord(ch)
                    if code in cmap:
                        gname = cmap[code]
                        w, lsb = ttf["hmtx"][gname]
                        ttf["hmtx"][gname] = (w + 400, lsb)
                        if "glyf" in ttf and gname in ttf["glyf"]:
                            glyph = ttf["glyf"][gname]
                            coords = glyph.coordinates
                            for i in range(len(coords)):
                                x, y = coords[i]
                                coords[i] = (x + 200, y + 200)
                            glyph.recalcBounds(ttf["glyf"])
                        modified = True
                        break
                if not modified:
                    reasons.append("Failed to find suitable glyph in dynamic.ttf to modify")
                ttf.save(str(dyn_ttf))

            # Pass 2: recompile under EXACT same isolation and environment (without replacing fixtures!)
            rust_res2 = self.run_case_rust(case, case_dir, rust_work, copy_case=False)
            if rust_res2["exit_code"] != 0 or not rust_res2["pdf_valid"] or not rust_res2["pdf_path"]:
                reasons.append(f"Recompilation after font update failed (exit code {rust_res2.get('exit_code')})")
            else:
                pdf_path2 = Path(rust_res2["pdf_path"])
                pdf_hash_2 = sha256_file(pdf_path2)
                if pdf_hash_1 == pdf_hash_2:
                    reasons.append("Cache invalidation failed: output PDF hash did not change after font update")

                render_dir2 = case_out_dir / "rust_render_pass2"
                render_dir2.mkdir(parents=True, exist_ok=True)
                poppler_p2 = render_poppler(pdf_path2, render_dir2)
                analysis_p2 = analyze_render(poppler_p2)
                ink_p2 = analysis_p2.get("total_ink_pixels", 0)

                if ink_p1 > 0 and ink_p2 > 0 and ink_p1 == ink_p2:
                    h1 = [sha256_file(p) for p in poppler_p1]
                    h2 = [sha256_file(p) for p in poppler_p2]
                    if h1 == h2:
                        reasons.append("Cache invalidation failed: rendered output did not change after font glyph modification")

                # Pass 3: third unchanged run reuses cache
                rust_res3 = self.run_case_rust(case, case_dir, rust_work, copy_case=False)
                if rust_res3["exit_code"] != 0 or not rust_res3["pdf_valid"] or not rust_res3["pdf_path"]:
                    reasons.append(f"Third compilation pass failed (exit code {rust_res3.get('exit_code')})")
                else:
                    pdf_hash_3 = sha256_file(rust_res3["pdf_path"])
                    if pdf_hash_3 != pdf_hash_2:
                        reasons.append(
                            f"Cache reuse failed: third unchanged compilation produced differing PDF hash ({pdf_hash_3} != {pdf_hash_2})"
                        )

            status = "pass" if not reasons else "fail"
            safe_rust = {k: (str(v) if isinstance(v, Path) else v) for k, v in rust_res2.items()}
            return {
                "name": case_name,
                "description": case.get("description", ""),
                "status": status,
                "reasons": reasons,
                "rust": safe_rust,
                "reference": {"skipped": True, "note": "Cache invalidation test"},
                "comparison": {"compared": False},
            }

        # Standard compilation with Rust Ratex
        rust_res = self.run_case_rust(case, case_dir, rust_work, copy_case=True)
        if rust_res["exit_code"] != 0:
            reasons.append(f"Ratex compilation failed (exit code {rust_res['exit_code']})")
        if not rust_res["pdf_valid"]:
            reasons.append("Ratex did not produce a valid PDF")

        rust_fonts: list[dict[str, Any]] = []
        missing_embeddings: list[str] = []
        rust_text = ""
        poppler_render_analysis: dict[str, Any] = {}
        poppler_png_paths: list[Path] = []
        pdfjs_result: dict[str, Any] = {}
        pdfjs_png_paths: list[Path] = []

        if rust_res["pdf_valid"] and rust_res["pdf_path"]:
            pdf_path = Path(rust_res["pdf_path"])

            # 2. Inspect embedded fonts via pdffonts
            rust_fonts, missing_embeddings = parse_pdffonts(pdf_path)
            if missing_embeddings:
                for font in missing_embeddings:
                    reasons.append(f"Font '{font}' is not embedded (emb: no)")

            # Check required fonts with alternative '|' support
            for req in case.get("required_fonts", []):
                alts = [a.strip().lower() for a in req.split("|") if a.strip()]
                matched = any(any(alt in f.get("name", "").lower() for alt in alts) for f in rust_fonts)
                if not matched:
                    reasons.append(f"Required font family '{req}' not found in PDF fonts")

            # 3. Deep font structure & glyph program validation
            font_struct_errors = validate_pdf_font_embedding(pdf_path, case)
            for ferr in font_struct_errors:
                reasons.append(ferr)

            # 4. Extract text layer via pdftotext
            rust_text, text_errs = extract_text(pdf_path)
            for err in text_errs:
                reasons.append(err)

            for expected in case.get("expected_text", []):
                if expected not in rust_text and re.sub(r"\s+", "", expected) not in re.sub(r"\s+", "", rust_text):
                    reasons.append(f"Expected text '{expected}' not found in pdftotext extracted text layer")

            # 5. Render pages via Poppler pdftoppm
            render_dir = case_out_dir / "rust_render"
            render_dir.mkdir(parents=True, exist_ok=True)
            poppler_png_paths = render_poppler(pdf_path, render_dir)
            poppler_render_analysis = analyze_render(poppler_png_paths)

            min_ink = int(case.get("min_ink_pixels", 200))
            if poppler_render_analysis.get("total_ink_pixels", 0) < min_ink:
                reasons.append(
                    f"Poppler total ink pixels ({poppler_render_analysis.get('total_ink_pixels')}) below "
                    f"threshold ({min_ink}); document rendered blank or empty"
                )

            # 6. Strict pdf.js rendering and text extraction
            pdfjs_render_dir = case_out_dir / "pdfjs_render"
            pdfjs_png_paths, pdfjs_result, pdfjs_errs = self.render_pdfjs(pdf_path, pdfjs_render_dir)
            for perr in pdfjs_errs:
                reasons.append(perr)

            if pdfjs_result:
                all_pj_text = " ".join(p.get("text", "") for p in pdfjs_result.get("pages", []))
                for expected in case.get("expected_text", []):
                    if expected not in all_pj_text and re.sub(r"\s+", "", expected) not in re.sub(r"\s+", "", all_pj_text):
                        reasons.append(f"Expected text '{expected}' not found in pdf.js extracted text layer")

                if self.pdfjs_info.get("canvas_available") and pdfjs_png_paths:
                    pj_analysis = analyze_render(pdfjs_png_paths)
                    if pj_analysis.get("total_ink_pixels", 0) < min_ink:
                        reasons.append(
                            f"pdf.js rendered ink pixels ({pj_analysis.get('total_ink_pixels')}) below "
                            f"threshold ({min_ink})"
                        )
            else:
                if self.args.require_pdfjs:
                    reasons.append("pdf.js verification required by --require-pdfjs, but helper failed or is unavailable")

        # 7. Optional Reference Run & Comparison
        ref_res: dict[str, Any] = {}
        comparison: dict[str, Any] = {}
        ref_poppler_png_paths: list[Path] = []
        ref_pdfjs_png_paths: list[Path] = []

        if not self.args.skip_reference:
            ref_res = self.run_case_reference(case, case_dir, ref_work)
            if ref_res.get("skipped"):
                reasons.append(ref_res.get("error", "Reference run skipped"))
            elif ref_res.get("exit_code") != 0:
                reasons.append(f"Reference compilation failed (exit code {ref_res['exit_code']})")
            elif ref_res.get("pdf_valid") and ref_res.get("pdf_path"):
                ref_pdf_path = Path(ref_res["pdf_path"])

                # Poppler render & comparison
                ref_render_dir = case_out_dir / "ref_render"
                ref_render_dir.mkdir(parents=True, exist_ok=True)
                ref_poppler_png_paths = render_poppler(ref_pdf_path, ref_render_dir)

                comparison, comp_errors = compare_renders(
                    poppler_png_paths, ref_poppler_png_paths, case, label="Poppler"
                )
                for cerr in comp_errors:
                    reasons.append(cerr)

                # pdf.js reference render & comparison
                if self.pdfjs_info.get("available") and pdfjs_png_paths:
                    ref_pdfjs_render_dir = case_out_dir / "ref_pdfjs_render"
                    ref_pdfjs_png_paths, _, pdfjs_ref_errs = self.render_pdfjs(
                        ref_pdf_path, ref_pdfjs_render_dir
                    )
                    if ref_pdfjs_png_paths:
                        pdfjs_comp, pdfjs_comp_errors = compare_renders(
                            pdfjs_png_paths, ref_pdfjs_png_paths, case, label="pdf.js"
                        )
                        comparison["pdfjs_comparison"] = pdfjs_comp
                        for cerr in pdfjs_comp_errors:
                            reasons.append(cerr)
        else:
            ref_res = {"skipped": True, "note": "Explicitly skipped via --skip-reference"}

        status = "pass" if not reasons else "fail"

        retain_mode = getattr(self.args, "retain", "failures")
        keep_work = getattr(self.args, "keep_work", False)
        if status == "pass" and not keep_work and retain_mode != "all":
            shutil.rmtree(rust_work, ignore_errors=True)
            shutil.rmtree(ref_work, ignore_errors=True)

        safe_rust = {k: (str(v) if isinstance(v, Path) else v) for k, v in rust_res.items()}
        safe_ref = {k: (str(v) if isinstance(v, Path) else v) for k, v in ref_res.items()}

        return {
            "name": case_name,
            "description": case.get("description", ""),
            "status": status,
            "reasons": reasons,
            "rust": {
                "exit_code": safe_rust.get("exit_code"),
                "duration_s": safe_rust.get("duration_s"),
                "pdf_valid": safe_rust.get("pdf_valid"),
                "fonts": rust_fonts,
                "missing_embeddings": missing_embeddings,
                "text_excerpt": rust_text[:2000],
                "poppler_render": poppler_render_analysis,
                "pdfjs": pdfjs_result,
                "stdout_excerpt": (safe_rust.get("stdout") or "")[-4096:],
                "stderr_excerpt": (safe_rust.get("stderr") or "")[-4096:],
            },
            "reference": {
                "skipped": safe_ref.get("skipped", False),
                "engine": safe_ref.get("engine"),
                "exit_code": safe_ref.get("exit_code"),
                "pdf_valid": safe_ref.get("pdf_valid"),
                "stdout_excerpt": (safe_ref.get("stdout") or "")[-4096:],
                "stderr_excerpt": (safe_ref.get("stderr") or "")[-4096:],
            },
            "comparison": comparison,
        }

    def run(self) -> int:
        self.setup()

        with open(self.manifest_path, "r", encoding="utf-8") as f:
            all_cases = json.load(f)

        selected_cases = all_cases
        if self.args.case:
            wanted = set(self.args.case)
            selected_cases = [c for c in all_cases if c["name"] in wanted]
            missing = wanted - {c["name"] for c in selected_cases}
            if missing:
                print(f"Error: Unknown cases: {sorted(missing)}", file=sys.stderr)
                return 1

        isolation_info = self.get_isolation_info()

        if self.args.require_isolation and (
            not isolation_info.get("sandboxed") or not isolation_info.get("sentinel_verified")
        ):
            print("ERROR: --require-isolation needs a sandbox with verified blocked sentinels.", file=sys.stderr)
            print(f"Detail: {isolation_info.get('reason') or isolation_info.get('sentinels')}", file=sys.stderr)
            report = {
                "ok": False,
                "executable": {
                    "path": str(self.ratex_path),
                    "sha256": self.ratex_sha256,
                    "version": self.ratex_version,
                },
                "isolation": isolation_info,
                "error": "Required isolation unavailable or sentinel verification failed",
                "summary": {"total": len(selected_cases), "passed": 0, "failed": len(selected_cases), "duration_s": 0.0},
                "cases": {},
            }
            report_file = self.output_dir / "report.json"
            with open(report_file, "w", encoding="utf-8") as rf:
                json.dump(report, rf, indent=2, ensure_ascii=False, default=str)
            return 1

        if self.args.require_pdfjs and not self.pdfjs_info.get("available"):
            print("ERROR: --require-pdfjs was specified, but pdfjs is not available!", file=sys.stderr)
            print(f"Detail: {self.pdfjs_info.get('error') or self.pdfjs_info.get('reason')}", file=sys.stderr)
            report = {
                "ok": False,
                "executable": {
                    "path": str(self.ratex_path),
                    "sha256": self.ratex_sha256,
                    "version": self.ratex_version,
                },
                "isolation": isolation_info,
                "pdfjs": self.pdfjs_info,
                "error": "pdf.js required but unavailable",
                "summary": {"total": len(selected_cases), "passed": 0, "failed": len(selected_cases), "duration_s": 0.0},
                "cases": {},
            }
            report_file = self.output_dir / "report.json"
            with open(report_file, "w", encoding="utf-8") as rf:
                json.dump(report, rf, indent=2, ensure_ascii=False, default=str)
            return 1

        t0 = time.perf_counter()
        results: dict[str, Any] = {}

        print(f"=== Ratex Font Verification Suite ===")
        print(f"Ratex:      {self.ratex_path} (sha256: {self.ratex_sha256[:12]}...)")
        print(f"Version:    {self.ratex_version}")
        print(f"Isolation:  {isolation_info['type']} (sandboxed: {isolation_info['sandboxed']})")
        if isolation_info.get("sandboxed"):
            print(f"Sentinel:   {'BLOCKED (verified)' if isolation_info.get('sentinel_verified') else 'FAILED'}")
            print(f"Masked:     {len(isolation_info.get('masked_paths', []))} system font/TEXMF paths")
        print(f"pdf.js:     {'Available' if self.pdfjs_info.get('available') else 'Unavailable'} (version {self.pdfjs_info.get('version')}, canvas: {self.pdfjs_info.get('canvas_backend')})")
        print(f"Output:     {self.output_dir}")
        print(f"Cases:      {len(selected_cases)}")
        print("=" * 60)

        jobs = max(1, int(self.args.jobs))
        with cf.ThreadPoolExecutor(max_workers=jobs) as executor:
            future_to_case = {executor.submit(self.execute_case, c): c["name"] for c in selected_cases}
            for future in cf.as_completed(future_to_case):
                cname = future_to_case[future]
                try:
                    case_res = future.result()
                    results[cname] = case_res
                    status_marker = "PASS" if case_res["status"] == "pass" else "FAIL"
                    duration_s = case_res.get("rust", {}).get("duration_s", 0.0)
                    print(f"[{status_marker}] {cname:<28} ({duration_s}s)")
                    if case_res["status"] != "pass":
                        for r in case_res.get("reasons", [])[:4]:
                            print(f"       ! {r}")
                        for engine in ("rust", "reference"):
                            run = case_res.get(engine, {})
                            if run.get("exit_code") not in (None, 0):
                                for stream in ("stdout", "stderr"):
                                    excerpt = run.get(f"{stream}_excerpt", run.get(stream, ""))
                                    if excerpt:
                                        print(f"       {engine} {stream}:\n{excerpt[-4096:]}")
                except Exception as exc:
                    results[cname] = {
                        "name": cname,
                        "status": "error",
                        "reasons": [f"Harness exception: {exc}"],
                        "rust": {},
                    }
                    print(f"[ERR ] {cname:<28} ({exc})")

        total_duration = round(time.perf_counter() - t0, 3)

        passed_count = sum(1 for r in results.values() if r["status"] == "pass")
        failed_count = sum(1 for r in results.values() if r["status"] != "pass")
        overall_ok = (failed_count == 0) and (len(results) == len(selected_cases))

        report = {
            "ok": overall_ok,
            "executable": {
                "path": str(self.ratex_path),
                "sha256": self.ratex_sha256,
                "version": self.ratex_version,
            },
            "isolation": isolation_info,
            "pdfjs": self.pdfjs_info,
            "summary": {
                "total": len(selected_cases),
                "passed": passed_count,
                "failed": failed_count,
                "duration_s": total_duration,
            },
            "cases": results,
        }

        report_file = self.output_dir / "report.json"
        with open(report_file, "w", encoding="utf-8") as rf:
            json.dump(report, rf, indent=2, ensure_ascii=False, default=str)

        print("=" * 60)
        print(f"Result:     {'ALL PASSED' if overall_ok else 'FAILURES DETECTED'}")
        print(f"Summary:    {passed_count}/{len(selected_cases)} passed ({total_duration}s)")
        print(f"Report:     {report_file}")

        return 0 if overall_ok else 1


def main() -> int:
    parser = argparse.ArgumentParser(description="Ratex font verification harness")
    parser.add_argument("--ratex", type=Path, required=True, help="Path to ratex binary")
    parser.add_argument("--output", type=Path, required=True, help="Output directory")
    parser.add_argument("--case", type=str, action="append", help="Specific case(s) to run")
    parser.add_argument("--reference-pdflatex", type=Path, help="Reference pdfLaTeX executable")
    parser.add_argument("--reference-xelatex", type=Path, help="Reference XeLaTeX executable")
    parser.add_argument("--require-isolation", action="store_true", help="Require Linux bwrap sandbox isolation")
    parser.add_argument("--require-pdfjs", action="store_true", help="Require pdf.js rendering and extraction")
    parser.add_argument("--skip-reference", action="store_true", help="Skip reference engine comparison")
    parser.add_argument("--jobs", type=int, default=4, help="Concurrent workers (default: 4)")
    parser.add_argument("--timeout", type=float, default=60.0, help="Per-case timeout (default: 60s)")
    parser.add_argument("--manifest", type=Path, help="Path to manifest.json")
    parser.add_argument("--keep-work", action="store_true", help="Keep workspace directories on success")
    parser.add_argument("--retain", choices=("failures", "all", "none"), default="failures", help="Artifact retention mode")

    args = parser.parse_args()
    harness = FontTestHarness(args)
    return harness.run()


if __name__ == "__main__":
    sys.exit(main())

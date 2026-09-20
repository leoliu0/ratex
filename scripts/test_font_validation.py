#!/usr/bin/env python3
"""Regressions for font attribution in PDF content streams."""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from test_fonts import HAS_FONTTOOLS, HAS_PYPDF, parse_content_stream_ops, validate_pdf_font_embedding

if HAS_PYPDF:
    from pypdf import PdfWriter
    from pypdf.generic import (
        ArrayObject, DecodedStreamObject, DictionaryObject, FloatObject,
        NameObject, NumberObject,
    )


@unittest.skipUnless(HAS_PYPDF, "pypdf is required")
class ContentFontStateTests(unittest.TestCase):
    def test_graphics_restore_restores_nested_font_selections(self):
        content = b"""
            BT /Latin 10 Tf (before) Tj ET
            q BT /Symbols 10 Tf <0100> Tj ET
              q BT /Other 10 Tf <0200> Tj ET Q
              BT <0101> Tj ET
            Q BT (after) Tj ET
        """
        self.assertEqual(parse_content_stream_ops(content), [
            ("TEXT", "/Latin", b"before", False),
            ("TEXT", "/Symbols", b"\x01\x00", False),
            ("TEXT", "/Other", b"\x02\x00", False),
            ("TEXT", "/Symbols", b"\x01\x01", False),
            ("TEXT", "/Latin", b"after", False),
        ])

    def test_text_array_order_and_nested_marked_content(self):
        content = b"""
            BT /Latin 10 Tf
            /Span << /ActualText (whole) >> BDC
              /Inner BMC [(first) -20 <7365636f6e64>] TJ EMC
              (third) Tj
            EMC
            (last) Tj ET
        """
        self.assertEqual(parse_content_stream_ops(content), [
            ("TEXT", "/Latin", b"first", True),
            ("TEXT", "/Latin", b"second", True),
            ("TEXT", "/Latin", b"third", True),
            ("TEXT", "/Latin", b"last", False),
        ])


@unittest.skipUnless(HAS_PYPDF and HAS_FONTTOOLS, "pypdf and fonttools are required")
class FontResourceTests(unittest.TestCase):
    def test_unembedded_type1_is_rejected(self):
        writer = PdfWriter()
        page = writer.add_blank_page(width=200, height=200)
        font = DictionaryObject({
            NameObject("/Type"): NameObject("/Font"),
            NameObject("/Subtype"): NameObject("/Type1"),
            NameObject("/BaseFont"): NameObject("/Helvetica"),
        })
        page[NameObject("/Resources")] = DictionaryObject({
            NameObject("/Font"): DictionaryObject({NameObject("/F1"): font}),
        })
        stream = DecodedStreamObject()
        stream.set_data(b"BT /F1 12 Tf 20 50 Td (hello) Tj ET")
        page[NameObject("/Contents")] = stream
        with tempfile.TemporaryDirectory() as temp:
            pdf = Path(temp) / "unembedded.pdf"
            writer.write(pdf)
            self.assertTrue(validate_pdf_font_embedding(pdf, {}))

    def test_form_resources_do_not_leak_page_fonts(self):
        writer = PdfWriter()
        page = writer.add_blank_page(width=200, height=200)
        glyph = DecodedStreamObject()
        glyph.set_data(b"600 0 0 0 600 700 d1 0 0 m 300 700 l 600 0 l f")
        # An actual embedded Type3 outline avoids depending on installed fonts.
        font = DictionaryObject({
            NameObject("/Type"): NameObject("/Font"),
            NameObject("/Subtype"): NameObject("/Type3"),
            NameObject("/FontBBox"): ArrayObject([NumberObject(n) for n in (0, 0, 600, 700)]),
            NameObject("/FontMatrix"): ArrayObject([FloatObject(n) for n in (.001, 0, 0, .001, 0, 0)]),
            NameObject("/CharProcs"): DictionaryObject({NameObject("/A"): glyph}),
            NameObject("/Encoding"): DictionaryObject({
                NameObject("/Differences"): ArrayObject([NumberObject(65), NameObject("/A")]),
            }),
            NameObject("/FirstChar"): NumberObject(65),
            NameObject("/LastChar"): NumberObject(65),
            NameObject("/Widths"): ArrayObject([NumberObject(600)]),
        })
        form = DecodedStreamObject()
        form.update({
            NameObject("/Type"): NameObject("/XObject"),
            NameObject("/Subtype"): NameObject("/Form"),
            NameObject("/BBox"): ArrayObject([NumberObject(n) for n in (0, 0, 200, 200)]),
            NameObject("/Resources"): DictionaryObject(),
        })
        form.set_data(b"BT /F1 12 Tf 20 50 Td (A) Tj ET")
        page[NameObject("/Resources")] = DictionaryObject({
            NameObject("/Font"): DictionaryObject({NameObject("/F1"): font}),
            NameObject("/XObject"): DictionaryObject({NameObject("/Form1"): form}),
        })
        contents = DecodedStreamObject()
        contents.set_data(b"/Form1 Do")
        page[NameObject("/Contents")] = contents
        with tempfile.TemporaryDirectory() as temp:
            pdf = Path(temp) / "form.pdf"
            writer.write(pdf)
            self.assertTrue(validate_pdf_font_embedding(pdf, {}))
            # Removing the entire dictionary invokes legacy resource inheritance.
            del form["/Resources"]
            writer.write(pdf)
            self.assertEqual(validate_pdf_font_embedding(pdf, {}), [])
            # The selected font is graphics state, not resource inheritance.
            form[NameObject("/Resources")] = DictionaryObject()
            form.set_data(b"BT 20 50 Td (A) Tj ET")
            contents.set_data(b"BT /F1 12 Tf ET /Form1 Do")
            writer.write(pdf)
            self.assertEqual(validate_pdf_font_embedding(pdf, {}), [])


if __name__ == "__main__":
    unittest.main()

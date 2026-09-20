#!/usr/bin/env node
/**
 * Strict pdf.js rendering and text extraction helper for Ratex font verification.
 *
 * Runs pdfjs-dist with system font substitution disabled (useSystemFonts: false)
 * to verify that all required glyphs are embedded directly in the PDF document.
 *
 * Supports modern ESM pdfjs-dist builds (v4+) as well as legacy builds,
 * and uses @napi-rs/canvas or canvas for pixel-exact rendering.
 *
 * Usage:
 *   node scripts/render_pdfjs.js --probe
 *   node scripts/render_pdfjs.js <pdf_path> <output_dir> [scale]
 */

const fs = require('fs');
const path = require('path');

async function loadPdfjs() {
  const candidates = [
    'pdfjs-dist/legacy/build/pdf.mjs',
    'pdfjs-dist/build/pdf.mjs',
    'pdfjs-dist',
    'pdfjs-dist/legacy/build/pdf.js',
  ];

  for (const candidate of candidates) {
    try {
      const mod = await import(candidate);
      if (mod && mod.getDocument) {
        return { pdfjs: mod, entry: candidate, version: mod.version || 'unknown' };
      }
    } catch {
      // try next
    }
  }

  // Fallback: CommonJS require
  try {
    const mod = require('pdfjs-dist');
    if (mod && mod.getDocument) {
      return { pdfjs: mod, entry: 'pdfjs-dist (cjs)', version: mod.version || 'unknown' };
    }
  } catch {
    // continue
  }

  try {
    const mod = require('pdfjs-dist/legacy/build/pdf.js');
    if (mod && mod.getDocument) {
      return { pdfjs: mod, entry: 'pdfjs-dist/legacy/build/pdf.js (cjs)', version: mod.version || 'legacy' };
    }
  } catch {
    // continue
  }

  throw new Error('Cannot load pdfjs-dist module (tried ESM and CJS entry points)');
}

async function loadCanvas() {
  try {
    const { createCanvas, Path2D, DOMMatrix, ImageData } = await import('@napi-rs/canvas');
    // pdf.js paths and the drawing context must come from the same backend.
    Object.assign(globalThis, { Path2D, DOMMatrix, ImageData });
    return {
      createCanvas,
      backend: '@napi-rs/canvas',
    };
  } catch {
    // fallback
  }

  try {
    const { createCanvas } = await import('canvas');
    return {
      createCanvas,
      backend: 'canvas',
    };
  } catch {
    // fallback
  }

  try {
    const napiCanvas = require('@napi-rs/canvas');
    if (napiCanvas.Path2D) {
      Object.assign(globalThis, {
        Path2D: napiCanvas.Path2D,
        DOMMatrix: napiCanvas.DOMMatrix,
        ImageData: napiCanvas.ImageData,
      });
    }
    return {
      createCanvas: napiCanvas.createCanvas,
      backend: '@napi-rs/canvas (cjs)',
    };
  } catch {
    // fallback
  }

  try {
    const { createCanvas } = require('canvas');
    return {
      createCanvas,
      backend: 'canvas (cjs)',
    };
  } catch {
    // fallback
  }

  return null;
}

async function probe() {
  const result = {
    available: false,
    version: null,
    entry: null,
    canvas_available: false,
    canvas_backend: null,
    error: null,
  };


  try {
    const canvasMod = await loadCanvas();
    if (canvasMod) {
      result.canvas_available = true;
      result.canvas_backend = canvasMod.backend;
    }
  } catch {
    result.canvas_available = false;
  }
  try {
    const { version, entry } = await loadPdfjs();
    result.available = true;
    result.version = version;
    result.entry = entry;
  } catch (e) {
    result.error = e.message;
    return result;
  }

  return result;
}

class NodeCanvasFactory {
  constructor(optionsOrFn) {
    if (typeof optionsOrFn === 'function') {
      this.createCanvas = optionsOrFn;
    } else if (optionsOrFn && typeof optionsOrFn.createCanvas === 'function') {
      this.createCanvas = optionsOrFn.createCanvas;
    } else if (NodeCanvasFactory._createCanvas) {
      this.createCanvas = NodeCanvasFactory._createCanvas;
    } else {
      throw new Error('NodeCanvasFactory: no createCanvas implementation provided');
    }
  }

  create(width, height) {
    const canvas = this.createCanvas(Math.max(1, Math.floor(width)), Math.max(1, Math.floor(height)));
    const context = canvas.getContext('2d');
    return { canvas, context };
  }

  reset(canvasAndContext, width, height) {
    canvasAndContext.canvas.width = Math.max(1, Math.floor(width));
    canvasAndContext.canvas.height = Math.max(1, Math.floor(height));
  }

  destroy(canvasAndContext) {
    canvasAndContext.canvas.width = 0;
    canvasAndContext.canvas.height = 0;
    canvasAndContext.canvas = null;
    canvasAndContext.context = null;
  }
}

async function renderPdf(pdfPath, outDir, scale = 2.0) {
  const probeInfo = await probe();
  if (!probeInfo.available) {
    return {
      ok: false,
      available: false,
      error: `pdfjs-dist not available: ${probeInfo.error}`,
      pages_count: 0,
      pages: [],
    };
  }

  const { pdfjs } = await loadPdfjs();
  const canvasMod = await loadCanvas();
  if (canvasMod) {
    NodeCanvasFactory._createCanvas = canvasMod.createCanvas;
  }

  fs.mkdirSync(outDir, { recursive: true });
  const rawBytes = fs.readFileSync(pdfPath);
  const data = new Uint8Array(rawBytes.buffer, rawBytes.byteOffset, rawBytes.byteLength);

  // Strict: disable system font substitution and disable FontFace so pdf.js
  // renders embedded font outlines directly via canvas paths in headless Node.
  // Pass CanvasFactory into getDocument so internal scratch canvases (e.g. for
  // Form XObjects, groups, masks, and patterns) use the same backend and prototype.
  const getDocumentParams = {
    data: data,
    isEvalSupported: false,
    useSystemFonts: false,
    disableFontFace: true,
    verbosity: 0,
  };
  if (canvasMod) {
    getDocumentParams.CanvasFactory = NodeCanvasFactory;
  }

  const loadingTask = pdfjs.getDocument(getDocumentParams);
  const doc = await loadingTask.promise;
  const numPages = doc.numPages;
  const pagesInfo = [];

  const canvasFactory = canvasMod
    ? (doc.canvasFactory || new NodeCanvasFactory(canvasMod.createCanvas))
    : null;
  const isCjk = (c) => (c >= 0x2e80 && c <= 0x9fff) || (c >= 0x3000 && c <= 0x30ff) || (c >= 0xff00 && c <= 0xffef);

  for (let i = 1; i <= numPages; i++) {
    const page = await doc.getPage(i);
    const textContent = await page.getTextContent({
      normalizeWhitespace: true,
      disableCombineTextItems: false,
    });

    const items = textContent.items.map((item) => ({
      str: item.str || '',
      fontName: item.fontName || '',
      hasEOL: Boolean(item.hasEOL),
      width: item.width || 0,
      height: item.height || 0,
    }));

    let combinedText = '';
    for (let j = 0; j < items.length; j++) {
      const cur = items[j].str;
      if (j === 0) {
        combinedText = cur;
      } else {
        const prev = items[j - 1].str;
        const prevLast = prev.length > 0 ? prev.charCodeAt(prev.length - 1) : 0;
        const curFirst = cur.length > 0 ? cur.charCodeAt(0) : 0;
        if (isCjk(prevLast) && isCjk(curFirst)) {
          combinedText += cur;
        } else {
          combinedText += ' ' + cur;
        }
      }
    }
    combinedText = combinedText.replace(/\s+/g, ' ').trim();
    let renderedPng = null;
    let inkPixels = 0;
    let bbox = null;

    if (canvasFactory) {
      const viewport = page.getViewport({ scale: parseFloat(scale) || 2.0 });
      const { canvas, context } = canvasFactory.create(viewport.width, viewport.height);

      // White background fill
      context.fillStyle = 'rgb(255, 255, 255)';
      context.fillRect(0, 0, viewport.width, viewport.height);

      const renderContext = {
        canvasContext: context,
        viewport: viewport,
        canvasFactory: canvasFactory,
      };

      await page.render(renderContext).promise;

      const pngPath = path.join(outDir, `page_${i}_pdfjs.png`);
      const buffer = typeof canvas.toBuffer === 'function'
        ? canvas.toBuffer('image/png')
        : (canvas.encode ? await canvas.encode('png') : null);

      if (buffer) {
        fs.writeFileSync(pngPath, buffer);
        renderedPng = pngPath;

        const { data: pixels, width, height } = context.getImageData(0, 0, canvas.width, canvas.height);
        let minX = width, minY = height, maxX = 0, maxY = 0;
        for (let y = 0; y < height; y++) {
          for (let x = 0; x < width; x++) {
            const idx = (y * width + x) * 4;
            if (pixels[idx] < 250 || pixels[idx + 1] < 250 || pixels[idx + 2] < 250) {
              inkPixels++;
              if (x < minX) minX = x;
              if (x > maxX) maxX = x;
              if (y < minY) minY = y;
              if (y > maxY) maxY = y;
            }
          }
        }
        if (inkPixels > 0) {
          bbox = [minX, minY, maxX, maxY];
        }
      }
    }

    pagesInfo.push({
      page: i,
      text: combinedText,
      item_count: items.length,
      items: items.slice(0, 100), // excerpt of first 100 items
      image_path: renderedPng,
      ink_pixels: inkPixels,
      bbox: bbox,
    });
  }

  return {
    ok: true,
    available: true,
    pages_count: numPages,
    pages: pagesInfo,
  };
}

async function main() {
  const args = process.argv.slice(2);
  if (args.length === 0 || args.includes('--probe')) {
    const info = await probe();
    console.log(JSON.stringify(info, null, 2));
    process.exit(info.available ? 0 : 1);
  }

  const pdfPath = path.resolve(args[0]);
  const outDir = path.resolve(args[1] || './pdfjs_render');
  const scale = args[2] ? parseFloat(args[2]) : 2.0;

  try {
    const result = await renderPdf(pdfPath, outDir, scale);
    const jsonPath = path.join(outDir, 'pdfjs_result.json');
    fs.writeFileSync(jsonPath, JSON.stringify(result, null, 2));
    console.log(JSON.stringify(result, null, 2));
    process.exit(result.ok ? 0 : 1);
  } catch (err) {
    console.error(`Render failed: ${err.stack || err.message}`);
    process.exit(1);
  }
}

if (require.main === module) {
  main().catch((err) => {
    console.error(err);
    process.exit(1);
  });
}

module.exports = { probe, renderPdf };

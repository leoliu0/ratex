// pdf.js preview: every page as a canvas; 1 PDF point = `zoom` CSS pixels.
import * as pdfjs from 'pdfjs-dist';

export class PdfView {
  constructor(container, { workerSrc, onInverseSearch, onRender }) {
    pdfjs.GlobalWorkerOptions.workerSrc = workerSrc;
    this.container = container;
    this.zoom = 1;
    // Fit the first page to the pane's width until the reader zooms.
    this.fitWidth = true;
    this.pdf = null;
    this.bytes = null;
    this.generation = 0;
    this.onInverseSearch = onInverseSearch;
    this.onRender = onRender ?? (() => {});
    container.addEventListener('dblclick', (event) => {
      const pageElement = event.target.closest('.pdf-page');
      if (!pageElement) return;
      const box = pageElement.getBoundingClientRect();
      this.onInverseSearch(
        Number(pageElement.dataset.page),
        (event.clientX - box.left) / this.zoom,
        (event.clientY - box.top) / this.zoom,
      );
    });
  }

  /** Render `bytes`, keeping the reader's relative scroll position. */
  async show(bytes) {
    const generation = ++this.generation;
    this.bytes = bytes;
    const pdf = await pdfjs.getDocument({ data: bytes.slice() }).promise;
    if (generation !== this.generation) {
      pdf.destroy();
      return;
    }
    if (this.fitWidth) {
      const first = await pdf.getPage(1);
      const width = first.getViewport({ scale: 1 }).width;
      const style = getComputedStyle(this.container);
      const available = this.container.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
      if (available > 0) this.zoom = Math.min(4, Math.max(0.25, available / width));
    }
    const pages = document.createElement('div');
    pages.className = 'pdf-pages';
    const ratio = window.devicePixelRatio || 1;
    for (let number = 1; number <= pdf.numPages; number++) {
      const page = await pdf.getPage(number);
      const viewport = page.getViewport({ scale: this.zoom });
      const wrapper = document.createElement('div');
      wrapper.className = 'pdf-page';
      wrapper.dataset.page = String(number);
      wrapper.style.width = `${viewport.width}px`;
      wrapper.style.height = `${viewport.height}px`;
      const canvas = document.createElement('canvas');
      canvas.width = Math.floor(viewport.width * ratio);
      canvas.height = Math.floor(viewport.height * ratio);
      canvas.style.width = `${viewport.width}px`;
      canvas.style.height = `${viewport.height}px`;
      wrapper.append(canvas);
      pages.append(wrapper);
      await page.render({
        canvas,
        canvasContext: canvas.getContext('2d'),
        viewport,
        transform: ratio === 1 ? null : [ratio, 0, 0, ratio, 0, 0],
      }).promise;
      if (generation !== this.generation) {
        pdf.destroy();
        return;
      }
    }
    const { scrollTop, scrollHeight, scrollLeft } = this.container;
    const fraction = scrollHeight ? scrollTop / scrollHeight : 0;
    this.container.replaceChildren(pages);
    this.container.scrollTop = fraction * this.container.scrollHeight;
    this.container.scrollLeft = scrollLeft;
    this.pdf?.destroy();
    this.pdf = pdf;
    this.container.dataset.pages = String(pdf.numPages);
    this.onRender();
  }

  clear() {
    this.generation++;
    this.pdf?.destroy();
    this.pdf = null;
    this.bytes = null;
    this.container.replaceChildren();
    delete this.container.dataset.pages;
    this.onRender();
  }

  async setZoom(zoom) {
    this.fitWidth = false;
    this.zoom = Math.min(4, Math.max(0.25, zoom));
    if (this.bytes) await this.show(this.bytes);
    else this.onRender();
  }

  /** Fit pages to the pane's width again, now and after later resizes. */
  async fit() {
    this.fitWidth = true;
    if (this.bytes) await this.show(this.bytes);
    else this.onRender();
  }

  /** 1-based number of the page at the top third of the pane, or 0. */
  currentPage() {
    const mark = this.container.scrollTop + this.container.clientHeight / 3;
    let current = 0;
    for (const page of this.container.querySelectorAll('.pdf-page')) {
      if (page.offsetTop > mark) break;
      current = Number(page.dataset.page);
    }
    return current || (this.pdf ? 1 : 0);
  }

  /** Scroll to and outline a rectangle given in PDF points from the top-left. */
  highlight({ page, x, y, width, height }) {
    const wrapper = this.container.querySelector(`.pdf-page[data-page="${page}"]`);
    if (!wrapper) return;
    this.container.querySelectorAll('.sync-mark').forEach((mark) => mark.remove());
    const mark = document.createElement('div');
    mark.className = 'sync-mark';
    mark.dataset.testid = 'sync-mark';
    mark.style.left = `${x * this.zoom - 2}px`;
    mark.style.top = `${y * this.zoom - 2}px`;
    mark.style.width = `${width * this.zoom + 4}px`;
    mark.style.height = `${height * this.zoom + 4}px`;
    wrapper.append(mark);
    const top = wrapper.offsetTop + y * this.zoom - this.container.clientHeight / 3;
    this.container.scrollTo({ top: Math.max(0, top) });
    setTimeout(() => mark.classList.add('fade'), 1200);
  }
}

// SyncTeX (version 1 text) for forward and inverse search. Coordinates in the
// file are scaled points from the page's top-left corner; the API below uses
// PDF big points with y measured downwards from the top of the page.
const SP_PER_BP = 65536 * 72.27 / 72;

/** Project path of a SyncTeX `Input:` name (`/project/a/b.tex`, `./b.tex`). */
function projectPath(name, entryDir) {
  let path = name.replace(/^\/project\//, '');
  if (path.startsWith('./')) path = entryDir + path.slice(2);
  return path.replace(/\/\.\//g, '/');
}

export function parseSynctex(text, entry) {
  const entryDir = entry.includes('/') ? entry.replace(/\/[^/]*$/, '/') : '';
  const inputs = new Map();
  const pages = new Map();
  let unit = 1;
  let xOffset = 0;
  let yOffset = 0;
  let page = null;
  for (const line of text.split('\n')) {
    if (line.startsWith('Input:')) {
      const rest = line.slice(6);
      const colon = rest.indexOf(':');
      inputs.set(Number(rest.slice(0, colon)), projectPath(rest.slice(colon + 1), entryDir));
    } else if (line.startsWith('Unit:')) {
      unit = Number(line.slice(5)) || 1;
    } else if (line.startsWith('X Offset:')) {
      xOffset = Number(line.slice(9)) || 0;
    } else if (line.startsWith('Y Offset:')) {
      yOffset = Number(line.slice(9)) || 0;
    } else if (line.startsWith('{')) {
      page = Number(line.slice(1));
      pages.set(page, []);
    } else if (line.startsWith('}')) {
      page = null;
    } else if (page !== null && /^[([xkg$hv]/.test(line)) {
      const kind = line[0];
      const match = /^.(\d+),(\d+)(?::(-?\d+),(-?\d+)(?::(-?\d+),(-?\d+),(-?\d+))?)?/.exec(line);
      if (!match || kind === '[') continue;
      const [, tag, number, x, y, w, h, d] = match;
      if (x === undefined) continue;
      const bp = (value) => Number(value) * unit / SP_PER_BP;
      pages.get(page).push({
        kind,
        file: Number(tag),
        line: Number(number),
        x: bp(x) + xOffset / SP_PER_BP,
        y: bp(y) + yOffset / SP_PER_BP,
        w: w === undefined ? 0 : bp(w),
        h: h === undefined ? 0 : bp(h),
        d: d === undefined ? 0 : bp(d),
      });
    }
  }
  return { inputs, pages };
}

/**
 * Source position -> PDF location. Prefers records on exactly `line`, else
 * the nearest following line, else the nearest preceding one.
 */
export function forwardSearch(sync, path, line) {
  const tags = [...sync.inputs].filter(([, p]) => p === path).map(([tag]) => tag);
  if (!tags.length) return null;
  let best = null;
  for (const [page, records] of sync.pages) {
    for (const record of records) {
      if (!tags.includes(record.file)) continue;
      const distance = record.line >= line ? record.line - line : (line - record.line) * 1000;
      if (!best || distance < best.distance || (distance === best.distance && page < best.page)) {
        best = { distance, page, line: record.line, records: [] };
      }
      if (best.page === page && best.line === record.line) best.records.push(record);
    }
  }
  if (!best) return null;
  const boxes = best.records;
  const top = Math.min(...boxes.map((r) => r.y - Math.max(r.h, 8)));
  const bottom = Math.max(...boxes.map((r) => r.y + r.d));
  const left = Math.min(...boxes.map((r) => r.x));
  const right = Math.max(...boxes.map((r) => r.x + Math.max(r.w, 4)));
  return { page: best.page, x: left, y: top, width: right - left, height: bottom - top };
}

/** PDF location (page, big points from the top-left) -> source position. */
export function inverseSearch(sync, page, x, y) {
  const records = sync.pages.get(page);
  if (!records?.length) return null;
  let best = null;
  for (const record of records) {
    const top = record.y - record.h;
    const bottom = record.y + record.d;
    const right = record.x + record.w;
    const inside = record.kind === '(' && x >= record.x && x <= right && y >= top && y <= bottom;
    // Distance to the record's box (or point), weighting vertical misses.
    const dx = x < record.x ? record.x - x : x > right ? x - right : 0;
    const dy = y < top ? top - y : y > bottom ? y - bottom : 0;
    const score = inside ? record.w * (record.h + record.d) * 1e-6 : 1e3 + dx + 4 * dy;
    if (!best || score < best.score) best = { score, record };
  }
  const path = sync.inputs.get(best.record.file);
  return path === undefined ? null : { path, line: best.record.line };
}

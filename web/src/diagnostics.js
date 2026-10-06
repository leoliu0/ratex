// Parse the compiler's rendered diagnostics:
//   error: Undefined control sequence `\foo`
//     --> /project/main.tex:12:5
// Message continuation lines start with "  | ".

function projectPath(name) {
  return name.replace(/^\/project\//, '').replace(/^\.\//, '');
}

export function parseDiagnostics(text) {
  const items = [];
  let current = null;
  for (const line of (text ?? '').split('\n')) {
    const head = /^(error|warning): (.*)$/.exec(line);
    if (head) {
      current = { severity: head[1], message: head[2], path: null, line: 0, column: 1 };
      items.push(current);
      continue;
    }
    if (!current) continue;
    const continued = /^ {2}\| (.*)$/.exec(line);
    if (continued && current.path === null) {
      current.message += `\n${continued[1]}`;
      continue;
    }
    const location = /^\s*--> (.+):(\d+):(\d+)\s*$/.exec(line);
    if (location && current.path === null) {
      current.path = projectPath(location[1]);
      current.line = Number(location[2]);
      current.column = Number(location[3]);
    }
  }
  return items;
}

export type Piece = {start: number; text: string; kind?: 'strong' | 'em' | 'code'};
export type Line = {start: number; kind: string; pieces: Piece[]};

export function readingLines(content: string, markdown: boolean): Line[] {
  let start = 0;
  let fence = '';
  return content.split('\n').map(raw => {
    const line = raw.replace(/\r$/, '');
    const offset = start;
    start += raw.length + 1;
    if (!markdown) return {start: offset, kind: 'text', pieces: [{start: offset, text: line}]};
    const delimiter = line.match(/^ {0,3}(`{3,}|~{3,})(.*)$/);
    if (delimiter && (!fence || (delimiter[1][0] === fence[0] && delimiter[1].length >= fence.length && !delimiter[2].trim()))) {
      fence = fence ? '' : delimiter[1];
      return {start: offset, kind: 'fence', pieces: []};
    }
    if (fence) return {start: offset, kind: 'codeblock', pieces: [{start: offset, text: line}]};
    const block = line.match(/^ {0,3}(#{1,6}|>|[-+*]|\d+[.)])\s+/);
    const marker = block?.[1];
    const kind = marker?.startsWith('#') ? 'h' + marker.length : marker === '>' ? 'quote' : marker ? 'bullet' : 'text';
    const skip = block?.[0].length ?? 0;
    const body = line.slice(skip);
    const pieces: Piece[] = [];
    let cursor = 0;
    for (const token of body.matchAll(/(`|\*\*|__|\*|_)([^\n]+?)\1/g)) {
      const at = token.index!;
      if (at > cursor) pieces.push({start: offset + skip + cursor, text: body.slice(cursor, at)});
      pieces.push({start: offset + skip + at + token[1].length, text: token[2],
        kind: token[1] === '`' ? 'code' : token[1].length === 2 ? 'strong' : 'em'});
      cursor = at + token[0].length;
    }
    if (cursor < body.length) pieces.push({start: offset + skip + cursor, text: body.slice(cursor)});
    return {start: offset, kind, pieces};
  });
}

export function selectedOffsets(root: HTMLElement, content: string, selection: Selection | null, indexed = false) {
  if (!selection || selection.isCollapsed || !selection.rangeCount) return null;
  const range = selection.getRangeAt(0);
  const position = (node: Node, index: number) => {
    const el = (node instanceof Element ? node : node.parentElement)?.closest<HTMLElement>('[data-offset]');
    if (!el || !root.contains(el)) return null;
    const prefix = document.createRange();
    prefix.selectNodeContents(el);
    prefix.setEnd(node, index);
    return Number(el.dataset.offset) + prefix.toString().length;
  };
  const a = position(range.startContainer, range.startOffset);
  const b = position(range.endContainer, range.endOffset);
  if (a === null || b === null) return null;
  const raw = content.slice(a, b);
  if (!indexed && raw !== range.toString()) return null;
  const surface = raw.trim();
  const start = a + raw.length - raw.trimStart().length;
  if (!surface || surface.length > 240 || /[\r\n]/.test(surface)) return null;
  return {start, end: start + surface.length, surface};
}

export function articleMatches(blocks: Piece[][], query: string) {
  const term = query.trim();
  if (!term) return [];
  const pattern = new RegExp(term.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'giu');
  return blocks.flatMap(pieces => {
    const text = pieces.map(piece => piece.text).join('');
    let index = 0;
    let offset = 0;
    function sourceAt(position: number) {
      while (position >= offset + pieces[index].text.length) {
        offset += pieces[index++].text.length;
      }
      return pieces[index].start + position - offset;
    }
    return Array.from(text.matchAll(pattern), match => ({
      start: sourceAt(match.index),
      end: sourceAt(match.index + match[0].length - 1) + 1
    }));
  });
}

export function editRange(before: string, after: string, cursor: number) {
  let oldEnd = before.length - (after.length - cursor);
  let newEnd = cursor;
  if (oldEnd < 0 || before.slice(oldEnd) !== after.slice(newEnd)) {
    oldEnd = before.length;
    newEnd = after.length;
    while (oldEnd > 0 && newEnd > 0 && before[oldEnd - 1] === after[newEnd - 1]) {
      oldEnd--;
      newEnd--;
    }
  }
  let start = 0;
  while (start < oldEnd && start < newEnd && before[start] === after[start]) start++;
  return {start, oldEnd, delta: newEnd - oldEnd};
}

type UnchangedText = {start:number; end:number; offset:number};

export function keepUnchanged(spans: UnchangedText[], before: string, after: string, cursor: number) {
  const {start, oldEnd, delta} = editRange(before,after,cursor);
  return spans.flatMap(span=>{
    const left = Math.min(span.end,start-span.offset);
    const right = Math.max(span.start,oldEnd-span.offset);
    const kept: UnchangedText[] = [];
    if (span.start < left) kept.push({...span,end:left});
    if (right < span.end) kept.push({...span,start:right,offset:span.offset+delta});
    return kept;
  });
}

export function changedRanges(spans: UnchangedText[], beforeLength: number, afterLength: number) {
  const changes: {start:number;end:number;delta:number}[] = [];
  let original = 0;
  let current = 0;
  for (const span of [...spans,{start:beforeLength,end:beforeLength,offset:afterLength-beforeLength}]) {
    const next = span.start + span.offset;
    if (span.start !== original || next !== current) {
      changes.push({start:original,end:span.start,delta:(next-current)-(span.start-original)});
    }
    original = span.end;
    current = span.end + span.offset;
  }
  return changes;
}

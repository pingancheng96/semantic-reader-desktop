import assert from 'node:assert/strict';
import test from 'node:test';
import {articleMatches, readingLines} from '../dist/reading.js';

test('Markdown pieces map to unchanged UTF-16 source, including emoji and CRLF', () => {
  const source = '# Prices 📈\r\n\r\n> Profits **plummeted** and then *rose*.\n- Use `bank` here.';
  const lines = readingLines(source,true);
  assert.deepEqual(lines.map(line=>line.kind),['h1','text','quote','bullet']);
  for (const piece of lines.flatMap(line=>line.pieces)) {
    assert.equal(source.slice(piece.start,piece.start+piece.text.length),piece.text);
  }
  assert.equal(lines[2].pieces.find(p=>p.kind==='strong').text,'plummeted');
  assert.equal(lines[2].pieces.find(p=>p.kind==='em').text,'rose');
  assert.equal(lines[3].pieces.find(p=>p.kind==='code').text,'bank');
});

test('TXT preserves Markdown delimiters and Markdown code fences preserve code', () => {
  assert.equal(readingLines('# a **word**',false)[0].pieces[0].text,'# a **word**');
  const lines = readingLines('~~~md\n# **code**\n~~~',true);
  assert.equal(lines[1].kind,'codeblock');
  assert.equal(lines[1].pieces[0].text,'# **code**');
});

test('article search finds literal phrases across inline formatting at source offsets', () => {
  const source = '📚 Ancient **writing** systems.\nWRITING systems.\n[writing]';
  const blocks = readingLines(source, true).map(line => line.pieces);
  const matches = articleMatches(blocks, 'writing systems');
  assert.deepEqual(matches.map(({start,end}) => source.slice(start,end)), [
    'writing** systems', 'WRITING systems'
  ]);
  assert.equal(articleMatches(blocks, '[writing]').length, 1);
  assert.deepEqual(articleMatches(blocks, '**'), []);
  assert.deepEqual(articleMatches(blocks, 'systems. writing'), []);
});

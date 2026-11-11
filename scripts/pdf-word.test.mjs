import assert from 'node:assert/strict';
import test from 'node:test';
import {pdfWordAt,matchNativePdfWords} from '../dist/pdf-word.js';

test('PDF click offsets identify a word without selecting adjacent punctuation or spaces', () => {
  const text = '📚 “Assail,” they said. Assail!';
  const second = text.lastIndexOf('Assail');
  assert.deepEqual(pdfWordAt(text,second+2),{start:second,end:second+6,surface:'Assail'});
  assert.equal(pdfWordAt(text,second+6),null);
  assert.equal(pdfWordAt(text,2),null);
  assert.equal(pdfWordAt("they’re here",5)?.surface,"they’re");
});

test('PDFKit word geometry is matched to the corresponding occurrence', () => {
  const text = 'An attempt to tell the story. Another attempt to explain it.';
  const native = [...text.matchAll(/[a-z]+/gi)].map((match,i)=>({word:match[0],rect:[i,0,i+1,1]}));
  const attempts = matchNativePdfWords(text,native).filter(word=>word.surface==='attempt');
  assert.deepEqual(attempts.map(word=>word.start),[text.indexOf('attempt'),text.lastIndexOf('attempt')]);
  assert.notDeepEqual(attempts[0].rect,attempts[1].rect);
});

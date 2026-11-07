import assert from 'node:assert/strict';
import test from 'node:test';
import {dictionaryLookupWord} from '../dist/dictionary-word.js';

test('a verified headword replaces the inflected form even when reusing an older sense', () => {
  assert.equal(dictionaryLookupWord('danced','dance','danced','dance'),'dance');
  assert.equal(dictionaryLookupWord('danced','danced','danced','dance'),'dance');
});

test('an explicit edit takes precedence and an unverified lookup can still reuse a known lemma', () => {
  assert.equal(dictionaryLookupWord('danced','dance around','danced','dance'),'dance around');
  assert.equal(dictionaryLookupWord('danced','danced','dance',undefined),'dance');
});

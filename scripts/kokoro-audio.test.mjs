import {test} from 'node:test';
import assert from 'node:assert/strict';
import {phonemeIds} from '../dist/kokoro-audio.js';

test('Kokoro receives word-initial stress before the vowel in both accents', () => {
  for (const locale of ['en-GB', 'en-US']) {
    assert.deepEqual(phonemeIds('/ˌsɛt ˈdaʊn/', locale), [61,157,86,62,16,46,156,39,56]);
    assert.deepEqual(phonemeIds('/ˈoʊpən ʌp ˈdɔːrweɪ/', locale), locale === 'en-GB'
      ? [156,31,58,83,56,16,138,58,16,46,156,76,158,123,65,24]
      : [156,31,58,83,56,16,138,58,16,46,156,76,123,65,24]);
    assert.deepEqual(phonemeIds('/ˈrekɔːd/', locale), locale === 'en-GB'
      ? [123,156,47,53,76,158,46] : [123,156,47,53,76,46]);
  }
});

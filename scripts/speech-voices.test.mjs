import assert from 'node:assert/strict';
import test from 'node:test';
import {waitForVoice} from '../dist/speech-voices.js';

test('a first click waits for the matching local voice to load', async () => {
  const speech = new EventTarget();
  const voice = {lang:'en_GB', localService:true};
  let voices = [{lang:'en-GB', localService:false}];
  speech.getVoices = () => voices;
  const pending = waitForVoice(speech,'en-GB',new AbortController().signal);
  voices = [...voices,voice];
  speech.dispatchEvent(new Event('voiceschanged'));
  assert.equal(await pending,voice);
});

test('reports missing voices after the wait and supports cancellation', async () => {
  const speech = new EventTarget();
  speech.getVoices = () => [];
  const request = new AbortController();
  assert.equal(await waitForVoice(speech,'en-US',request.signal,5),undefined);
  const pending = waitForVoice(speech,'en-US',request.signal);
  request.abort();
  assert.equal(await pending,undefined);
});

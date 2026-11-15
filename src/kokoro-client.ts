import {readKokoro, type KokoroVariant} from './runtime.js';
import {infer} from './kokoro-audio.js';

type Ort = typeof import('onnxruntime-web');
type Session = import('onnxruntime-web').InferenceSession;
let runtime:{variant:KokoroVariant;ort:Ort;session:Session}|undefined;
const voices = new Map<string,Promise<Float32Array>>();
let pending = Promise.resolve();

async function load(variant:KokoroVariant) {
  if (runtime?.variant===variant) return runtime;
  if (runtime) {
    await runtime.session.release();
    runtime=undefined;
  }
  try {
    const module = new URL('./ort/ort.wasm.min.mjs',import.meta.url).href;
    const ort = await import(module) as Ort;
    ort.env.wasm.numThreads=1;
    ort.env.wasm.wasmPaths=new URL('./ort/',import.meta.url).href;
    const model = await readKokoro(variant,0,variant==='fp32' ? 325_532_232 : 92_361_116);
    const session = await ort.InferenceSession.create(model,{executionProviders:['wasm']});
    runtime={variant,ort,session};
    return runtime;
  } catch(error) {
    throw new Error('Kokoro 模型加载失败 · '+String(error));
  }
}

export function kokoroAudio(variant:KokoroVariant,ipa:string,locale:'en-GB'|'en-US'):Promise<ArrayBuffer> {
  const run = pending.then(async()=>{
    const {ort,session} = await load(variant);
    let voice = voices.get(locale);
    if (!voice) {
      voice = readKokoro(variant,locale==='en-US' ? 1 : 2,522_240).then(data=>new Float32Array(data))
        .catch(error=>{voices.delete(locale);throw error;});
      voices.set(locale,voice);
    }
    return infer(ort,session,await voice,ipa,locale);
  });
  pending=run.then(()=>{},()=>{});
  return run;
}

let context:AudioContext|undefined;
export async function playKokoro(variant:KokoroVariant,ipa:string,locale:'en-GB'|'en-US',current:()=>boolean) {
  return playKokoroInput(variant,()=>ipa,locale,current);
}

export function playKokoroText(variant:KokoroVariant,text:string,locale:'en-GB'|'en-US',current:()=>boolean) {
  return playKokoroInput(variant,async()=>{
    const path = new URL('./phonemize.mjs',import.meta.url).href;
    const {toIPA} = await import(path) as typeof import('phonemize');
    return toIPA(text,locale).replace(/[.,!?;:…—–“”‘’()]/g,' ');
  },locale,current);
}

async function playKokoroInput(variant:KokoroVariant,readIpa:()=>string|Promise<string>,locale:'en-GB'|'en-US',current:()=>boolean) {
  context ??= new AudioContext();
  const output = context;
  // Start audio during the click, before waiting for model loading or inference.
  await output.resume();
  const wave = await kokoroAudio(variant,await readIpa(),locale);
  if (!current()) return;
  const buffer = await output.decodeAudioData(wave);
  if (!current()) return;
  const source = output.createBufferSource();
  source.buffer=buffer;
  source.connect(output.destination);
  source.start();
  return source;
}

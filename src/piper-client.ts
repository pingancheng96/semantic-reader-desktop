import {readPiper} from './runtime.js';
import {wave} from './kokoro-audio.js';
type Ort=typeof import('onnxruntime-web');
type Phonemizer={callMain:(args:string[])=>number};
type Create=(options:Record<string,unknown>)=>Promise<Phonemizer>;
let resources:Promise<{ort:Ort;session:import('onnxruntime-web').InferenceSession;config:{audio:{sample_rate:number};espeak:{voice:string};inference:{noise_scale:number;length_scale:number;noise_w:number}};create:Create;wasm:Uint8Array;data:ArrayBuffer}>|undefined;
let pending=Promise.resolve();
let context:AudioContext|undefined;
async function load() {
  resources ??= (async()=>{
    const path=(name:string)=>new URL('./piper/'+name,import.meta.url).href;
    const ort=await import(new URL('./ort/ort.wasm.min.mjs',import.meta.url).href) as Ort;
    ort.env.wasm.numThreads=1;ort.env.wasm.wasmPaths=new URL('./ort/',import.meta.url).href;
    const [{createPiperPhonemize:create},wasm,data,config,model]=await Promise.all([
      import(path('phonemize.mjs')) as Promise<{createPiperPhonemize:Create}>,
      fetch(path('piper_phonemize.wasm')).then(response=>response.arrayBuffer()).then(bytes=>new Uint8Array(bytes)),
      fetch(path('piper_phonemize.data')).then(response=>response.arrayBuffer()),
      readPiper(1,4878).then(bytes=>JSON.parse(new TextDecoder().decode(bytes))),readPiper(0,63201294),
    ]);
    const session=await ort.InferenceSession.create(model,{executionProviders:['wasm']});
    return {ort,session,config,create,wasm,data};
  })().catch(error=>{resources=undefined;throw error;});
  return resources;
}
export function piperAudio(text:string):Promise<ArrayBuffer> {
  const run=pending.then(async()=>{
    if(!text.trim() || text.length>500)throw Error('请选择较短的词或短语');
    const {ort,session,config,create,wasm,data}=await load();
    const ids=await new Promise<number[]>((resolve,reject)=>{
      let settled=false;
      const finish=(error?:unknown,ids?:number[])=>{if(settled)return;settled=true;clearTimeout(timer);if(error)reject(error);else resolve(ids!);};
      const timer=setTimeout(()=>finish(Error('Piper 发音转换超时')),15000);
      void create({wasmBinary:wasm,getPreloadedPackage:()=>data.slice(0),
        print:(line:string)=>{try {const parsed=JSON.parse(line);if(Array.isArray(parsed.phoneme_ids))finish(undefined,parsed.phoneme_ids);}catch{}},
        printErr:(line:string)=>{console.warn('Piper',line);},onAbort:(error:unknown)=>finish(error),
      }).then(module=>{
        try {module.callMain(['-l',config.espeak.voice,'--input',JSON.stringify([{text:text.trim()}]),'--espeak_data','/espeak-ng-data']);}
        catch(error){finish(error);}
      },error=>finish(error));
    });
    if(!ids.length || ids.some(id=>!Number.isInteger(id) || id<0))throw Error('Piper 没有返回有效发音');
    const result=await session.run({
      input:new ort.Tensor('int64',BigInt64Array.from(ids,BigInt),[1,ids.length]),
      input_lengths:new ort.Tensor('int64',BigInt64Array.of(BigInt(ids.length)),[1]),
      scales:new ort.Tensor('float32',Float32Array.of(config.inference.noise_scale,config.inference.length_scale,config.inference.noise_w),[3]),
    });
    const samples=result[session.outputNames[0]].data;
    if(!(samples instanceof Float32Array) || !samples.length)throw Error('Piper 没有生成音频');
    return wave(samples,config.audio.sample_rate);
  });
  pending=run.then(()=>{},()=>{});return run;
}
export async function playPiper(text:string,current:()=>boolean) {
  context ??=new AudioContext();const output=context;await output.resume();
  const wav=await piperAudio(text);if(!current())return;
  const buffer=await output.decodeAudioData(wav);if(!current())return;
  const source=output.createBufferSource();source.buffer=buffer;source.connect(output.destination);source.start();return source;
}

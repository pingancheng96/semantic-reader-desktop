// English phoneme IDs from the Kokoro v1.0 model vocabulary.
const symbols: Record<string,number> = {
  ' ':16, A:24, I:25, O:31, W:39, Y:41, a:43, b:44, d:46, e:47, f:48, h:50, i:51,
  j:52, k:53, l:54, m:55, n:56, o:57, p:58, s:61, t:62, u:63, v:64, w:65, x:66, z:68,
  'ɑ':69,'ɐ':70,'ɒ':71,'æ':72,'ɔ':76,'ð':81,'ʤ':82,'ə':83,'ɚ':85,'ɛ':86,'ɜ':87,
  'ɡ':92,'ɪ':102,'ŋ':112,'θ':119,'ɹ':123,'ɾ':125,'ʃ':131,'ʧ':133,'ʊ':135,
  'ʌ':138,'ʒ':147,'ʔ':148,'ˈ':156,'ˌ':157,'ː':158,'ʰ':162,'ʲ':164,'ᵻ':177,
};

export function phonemeIds(ipa:string, locale:'en-GB'|'en-US') {
  const phonemes = ipa.trim().replace(/^[/\[]|[/\]]$/g,'').normalize('NFC')
    .replace(/[.‿͜͡]/g,'')
    .replace(/tʃ/g,'ʧ').replace(/dʒ/g,'ʤ')
    .replace(/eɪ/g,'A').replace(/[aɑ]ɪ/g,'I').replace(/[aɑ]ʊ/g,'W')
    .replace(/[oə]ʊ/g,'O').replace(/ɔɪ/g,'Y')
    .replace(/ɝ/g,'ɜɹ').replace(/ɻ|r/g,'ɹ').replace(/g/g,'ɡ')
    .replace(/ɫ/g,'l').replace(/([lmnŋ])\u0329/g,'ə$1')
    .replace(/\s+/g,' ')
    // Kokoro can voice a word-initial stress token before the onset.
    // Put that token immediately before the first vowel of the word.
    .replace(/(^| )([ˈˌ])([bdfhjklmnprstvwxzðʤɡŋθɹɾʃʧʒʔ]+)(?=[AIOWYaeiouɑɐɒæɔəɚɛɜɪʊʌᵻ])/g,'$1$3$2')
    .trim();
  // A length mark is expected by the British model. American input uses plain vowels.
  const text = locale==='en-US' ? phonemes.replace(/ː/g,'') : phonemes;
  const ids = [...text].map(symbol=>{
    if (!Object.hasOwn(symbols,symbol)) throw new Error(`Kokoro 暂不支持此音标符号「${symbol}」`);
    return symbols[symbol];
  });
  if (!ids.length || ids.length>510) throw new Error('Kokoro 音标长度无效');
  return ids;
}

export function wave(samples:Float32Array) {
  const buffer = new ArrayBuffer(44+samples.length*2);
  const header = new DataView(buffer);
  const text = (offset:number,value:string)=>[...value].forEach((character,index)=>header.setUint8(offset+index,character.charCodeAt(0)));
  text(0,'RIFF');header.setUint32(4,buffer.byteLength-8,true);
  text(8,'WAVEfmt ');header.setUint32(16,16,true);header.setUint16(20,1,true);
  header.setUint16(22,1,true);header.setUint32(24,24000,true);header.setUint32(28,48000,true);
  header.setUint16(32,2,true);header.setUint16(34,16,true);
  text(36,'data');header.setUint32(40,samples.length*2,true);
  samples.forEach((value,index)=>header.setInt16(44+index*2,Math.round(Math.max(-1,Math.min(1,value))*32767),true));
  return buffer;
}

export async function infer(
  ort: typeof import('onnxruntime-web'), session: import('onnxruntime-web').InferenceSession,
  voice: Float32Array, ipa:string, locale:'en-GB'|'en-US'
) {
  const ids = phonemeIds(ipa,locale);
  const offset = ids.length*256;
  const style = voice.slice(offset,offset+256);
  if (style.length!==256) throw new Error('Kokoro 声音文件不完整');
  const input = BigInt64Array.from([0,...ids,0],BigInt);
  const output = await session.run({
    input_ids:new ort.Tensor('int64',input,[1,input.length]),
    style:new ort.Tensor('float32',style,[1,256]),
    speed:new ort.Tensor('float32',Float32Array.of(1),[1]),
  });
  const audio = output[session.outputNames[0]].data;
  if (!(audio instanceof Float32Array) || !audio.length) throw new Error('Kokoro 没有生成音频');
  return wave(audio);
}

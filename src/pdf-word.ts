const words = new Intl.Segmenter('en',{granularity:'word'});

export function pdfWordAt(text:string, offset:number) {
  if (!Number.isInteger(offset) || offset<0 || offset>=text.length) return null;
  const word = words.segment(text).containing(offset);
  if (!word?.isWordLike) return null;
  return {start:word.index,end:word.index+word.segment.length,surface:word.segment};
}

export type NativeWord = {word:string;rect:[number,number,number,number]|null};
const normalize = (text:string) => text.normalize('NFKC').toLowerCase().replace(/’/g,"'");

export function matchNativePdfWords(text:string, native:NativeWord[]) {
  const indexed = Array.from(words.segment(text)).filter(word=>word.isWordLike);
  const indexedKeys = indexed.map(word=>normalize(word.segment));
  const nativeKeys = native.map(word=>normalize(word.word));
  const sameOrder = indexedKeys.length===nativeKeys.length && indexedKeys.every((key,i)=>key===nativeKeys[i]);
  const contexts = new Map<string,number>();
  const context = (keys:string[],i:number) => JSON.stringify(keys.slice(Math.max(0,i-2),i+3));
  if (!sameOrder) {
    for (let i=0;i<nativeKeys.length;i++) {
      const key = context(nativeKeys,i);
      contexts.set(key,contexts.has(key) ? -1 : i);
    }
  }
  return indexed.flatMap((word,i)=>{
    const at = sameOrder ? i : contexts.get(context(indexedKeys,i)) ?? -1;
    const rect = native[at]?.rect;
    return rect && indexedKeys[i]===nativeKeys[at]
      ? [{start:word.index,end:word.index+word.segment.length,surface:word.segment,rect}] : [];
  });
}

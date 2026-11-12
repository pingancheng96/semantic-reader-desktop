export const React = (window as unknown as {React: typeof import('react')}).React;
export const ReactDOM = (window as unknown as {ReactDOM: typeof import('react-dom/client') & Pick<typeof import('react-dom'),'createPortal'>}).ReactDOM;
type ProgressChannel<T> = {onmessage:(value:T)=>void};
type Invoke = <T>(command: string, args: object) => Promise<T>;
const tauri = (window as unknown as {__TAURI__: {core: {invoke: Invoke; Channel: new <T>() => ProgressChannel<T>}}}).__TAURI__;
export const exportFile = (documentId?: string) => tauri.core.invoke<boolean>('export_file', {documentId});
export const restoreBackupFile = () => tauri.core.invoke<boolean>('restore_backup_file',{});
export const importLocalFile = () => tauri.core.invoke<{id:string;title:string}|null>('import_file',{});
export async function readPdfRange(id:string, begin:number, end:number) {
  const bytes = await tauri.core.invoke<ArrayBuffer|number[]>('pdf_range',{id,begin,end});
  return bytes instanceof ArrayBuffer ? new Uint8Array(bytes) : Uint8Array.from(bytes);
}
export const call = <T>(op: string, payload: object = {}) => tauri.core.invoke<T>('request', {op, payload});

export const choosePronunciationFolder = <T>() => tauri.core.invoke<T|null>('pronunciation_folder',{});

export type KokoroVariant = 'fp32'|'int8';
export type KokoroStatus = Record<KokoroVariant,boolean>;

export function installKokoro(variant:KokoroVariant,onProgress:(value:{bytes:number;total:number})=>void) {
  const progress = new tauri.core.Channel<{bytes:number;total:number}>();
  progress.onmessage = onProgress;
  return tauri.core.invoke<void>('kokoro_install',{variant,progress});
}

export async function readKokoro(variant:KokoroVariant,asset:number,size:number) {
  const data = new Uint8Array(size);
  for (let offset=0;offset<size;offset+=8*1024*1024) {
    const length = Math.min(8*1024*1024,size-offset);
    const part = await tauri.core.invoke<ArrayBuffer|number[]>('kokoro_read',{variant,asset,offset,length});
    const bytes = part instanceof ArrayBuffer ? new Uint8Array(part) : Uint8Array.from(part);
    if (bytes.length!==length) throw new Error('Kokoro 模型读取不完整');
    data.set(bytes,offset);
  }
  return data.buffer;
}

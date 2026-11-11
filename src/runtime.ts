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

export type DictionaryStatus = {filename:string; words:number};
export type DictionaryProgress = {phase:'download'|'index'; bytes:number; total:number|null};
export function installDictionary(download:boolean, onProgress:(value:DictionaryProgress)=>void) {
  const progress = new tauri.core.Channel<DictionaryProgress>();
  progress.onmessage = onProgress;
  return tauri.core.invoke<DictionaryStatus|null>('dictionary_install',{download,progress});
}

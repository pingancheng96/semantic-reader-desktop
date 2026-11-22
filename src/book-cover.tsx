import type {ReactNode} from 'react';
import {React, call} from './runtime.js';
import {loadPdf, renderPage} from './pdf-reader.js';

const covers = new Map<string,Blob|null>();
let pending = Promise.resolve();

async function loadCover(id:string,format:string):Promise<Blob|null> {
  if (format==='epub') {
    const image = await call<string|null>('epub_cover',{id});
    if (!image) return null;
    const separator = image.indexOf(',');
    const mime = image.slice(5,image.indexOf(';'));
    const bytes = Uint8Array.from(atob(image.slice(separator+1)),character=>character.charCodeAt(0));
    return new Blob([bytes],{type:mime});
  }
  const {length} = await call<{length:number}>('pdf_load',{id});
  const task = await loadPdf(id,length,()=>{});
  try {
    const document = await task.promise;
    const page = await document.getPage(1);
    const original = page.getViewport({scale:1});
    const {canvas} = await renderPage(page,320/Math.max(original.width,original.height));
    return await new Promise(resolve=>canvas.toBlob(resolve,'image/jpeg',.85));
  } finally {
    await task.destroy();
  }
}

export function BookCover({id,format,label,visible,children}:{
  id:string; format:string; label:string; visible:boolean; children:ReactNode;
}) {
  const ref = React.useRef<HTMLSpanElement>(null);
  const [url,setUrl] = React.useState('');
  React.useEffect(()=>{
    const element = ref.current;
    if (!visible || !element || !['pdf','epub','web'].includes(format)) return;
    let active = true;
    let objectUrl = '';
    const observer = new IntersectionObserver(entries=>{
      if (!entries.some(entry=>entry.isIntersecting)) return;
      observer.disconnect();
      pending = pending.then(async()=>{
        if (!active || !element.getClientRects().length) return;
        if (format==='web') {
          const image = await call<string|null>('web_cover',{id});
          if (active) setUrl(image && /^data:image\/(png|jpeg|gif|webp|avif);base64,/.test(image) ? image : '');
          return;
        }
        if (!covers.has(id)) {
          const blob = await loadCover(id,format);
          if (covers.size>=100) covers.delete(covers.keys().next().value!);
          covers.set(id,blob);
        }
        const blob = covers.get(id);
        if (active && blob) {
          objectUrl = URL.createObjectURL(blob);
          setUrl(objectUrl);
        }
      }).catch(()=>{});
    });
    observer.observe(element);
    return ()=>{
      active = false;
      observer.disconnect();
      if (objectUrl) URL.revokeObjectURL(objectUrl);
      if (format!=='web') setUrl('');
    };
  },[id,format,visible]);
  return <span ref={ref} className="library-cover" data-format={label} aria-hidden="true">
    <span>{label}</span>{children}
    {url && <img src={url} alt="" onError={()=>setUrl('')}/>}
  </span>;
}

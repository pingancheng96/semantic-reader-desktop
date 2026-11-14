import {ReaderToolbar} from './reader-toolbar.js';
import {ReaderNavigation, readReaderPreference, saveReaderPreference, type ReaderNavigationState} from './reader-navigation.js';
import {readerGestures} from './reader-gestures.js';
import {pdfWordAt,matchNativePdfWords,type NativeWord} from './pdf-word.js';
import {useArticleSearchShortcut} from './article-search.js';
import type {PDFDocumentProxy, PDFPageProxy} from 'pdfjs-dist';
import type {Worker} from 'tesseract.js';
import {React, call, readPdfRange} from './runtime.js';
import {articleMatches, selectedOffsets} from './reading.js';
const {useEffect,useMemo,useRef,useState} = React;

const assets = (path:string) => new URL(path,import.meta.url).href;
let engine:Promise<typeof import('pdfjs-dist')>|undefined;
function pdfEngine() {
  engine ??= import(assets('./pdfjs/pdf.mjs')).then((module:typeof import('pdfjs-dist'))=>{
    module.GlobalWorkerOptions.workerSrc = assets('./pdfjs/pdf.worker.mjs');
    return module;
  });
  return engine;
}
export async function loadPdf(id:string, length:number, onError:(error:unknown)=>void) {
  const pdf = await pdfEngine();
  const chunkSize = 256 * 1024;
  const initial = await readPdfRange(id,0,Math.min(chunkSize,length));
  let aborted = false;
  class LocalPdfTransport extends pdf.PDFDataRangeTransport {
    requestDataRange(begin:number,end:number) {
      void readPdfRange(id,begin,end).then(bytes=>{
        if (!aborted) this.onDataRange(begin,bytes);
      },error=>{
        if (!aborted) {
          onError(error);
          void task.destroy();
        }
      });
    }
    abort() { aborted = true; }
  }
  const task = pdf.getDocument({
    range:new LocalPdfTransport(length,initial),rangeChunkSize:chunkSize,disableStream:true,disableAutoFetch:true,
    cMapUrl:assets('./pdfjs/cmaps/'),cMapPacked:true,standardFontDataUrl:assets('./pdfjs/standard_fonts/'),
    wasmUrl:assets('./pdfjs/wasm/'),iccUrl:assets('./pdfjs/iccs/'),enableXfa:false
  });
  return task;
}

type OutlineEntry = {title:string;dest:string|unknown[]|null;items:OutlineEntry[]};

function OutlineTree({items,open}: {items:OutlineEntry[];open:(destination:string|unknown[])=>void}) {
  return <ul>{items.map((item,index)=><li key={index}>
    {item.dest ? <button type="button" onClick={()=>{if (item.dest) open(item.dest);}}>{item.title}</button> : <span>{item.title}</span>}
    {item.items.length>0 && <OutlineTree items={item.items} open={open}/>}
  </li>)}</ul>;
}

type WordBox = {text:string;x:number;y:number;w:number;h:number};
type Layer = {native:Awaited<ReturnType<PDFPageProxy['getTextContent']>>;offsets:number[]} | {words:WordBox[];offsets:number[]};
type IndexedPage = {page:number;offset:number;text:string};
type Position = {start:number;end:number};
type Selection = Position & {document_id:string;surface:string};
type Mark = Position & {id:string;surface:string;meaning:string};
const clean = (text:string) => text.replace(/[\r\n\0]/g,' ');
function combine(parts:string[]) {
  let text = '';
  const offsets = parts.map(part=>{
    if (text && part && !/\s$/.test(text) && !/^\s/.test(part)) text += ' ';
    const offset = text.length;
    text += part;
    return offset;
  });
  return {text,offsets};
}

export async function renderPage(page:PDFPageProxy, scale:number, pixels=1, signal?:AbortSignal) {
  const viewport = page.getViewport({scale});
  const canvas = document.createElement('canvas');
  canvas.width = Math.ceil(viewport.width*pixels);
  canvas.height = Math.ceil(viewport.height*pixels);
  const render = page.render({canvas,viewport,transform:pixels===1 ? undefined : [pixels,0,0,pixels,0,0]});
  const cancel = () => render.cancel();
  signal?.addEventListener('abort',cancel,{once:true});
  if (signal?.aborted) cancel();
  try { await render.promise; }
  finally { signal?.removeEventListener('abort',cancel); }
  return {canvas,viewport};
}

async function indexPage(page:PDFPageProxy, ocr:()=>Promise<Worker>) {
  const native = await page.getTextContent();
  const parts = native.items.flatMap(item=>'str' in item ? [clean(item.str)] : []);
  if (parts.some(part=>part.trim())) {
    const {text,offsets} = combine(parts);
    return {text,layer:{native,offsets}};
  }
  const original = page.getViewport({scale:1});
  const {canvas} = await renderPage(page,2000/Math.max(original.width,original.height));
  const worker = await ocr();
  const {data} = await worker.recognize(canvas,{}, {blocks:true,text:false});
  const words = (data.blocks ?? []).flatMap(block=>block.paragraphs.flatMap(paragraph=>
    paragraph.lines.flatMap(line=>line.words))).filter(word=>word.text.trim()).map(({text,bbox})=>({
      text:clean(text),x:bbox.x0/canvas.width,y:bbox.y0/canvas.height,
      w:(bbox.x1-bbox.x0)/canvas.width,h:(bbox.y1-bbox.y0)/canvas.height
    }));
  const {text,offsets} = combine(words.map(word=>word.text));
  return {text,layer:{words,offsets}};
}

function sizePdfSheet(sheet:HTMLElement, scale:number) {
  const holder = sheet.parentElement;
  if (!holder || !sheet.style.width) return;
  holder.style.width = Number.parseFloat(sheet.style.width)*scale+'px';
  holder.style.height = Number.parseFloat(sheet.style.height)*scale+'px';
  sheet.style.transform = scale===1 ? '' : `scale(${scale})`;
}

function pdfWordAtPoint(layer:HTMLElement, text:string, x:number, y:number) {
  const caret = document.caretRangeFromPoint?.(x,y);
  const caretNode = caret?.startContainer;
  const span = (caretNode instanceof Element ? caretNode : caretNode?.parentElement)
    ?.closest<HTMLElement>('span[data-offset]');
  const hit = document.elementFromPoint(x,y)?.closest<HTMLElement>('span[data-offset]');
  if (hit?.classList.contains('ocr-word') && layer.contains(hit)) {
    const start = Number(hit.dataset.offset);
    const surface = hit.textContent ?? '';
    return surface && Number.isInteger(start) && text.slice(start,start+surface.length)===surface
      ? {start,end:start+surface.length,surface} : null;
  }
  if (!caret || !span || !layer.contains(span) || !(caretNode instanceof Text)) return null;
  const caretOffset = caret.startOffset;
  const range = document.createRange();
  let best:number|undefined;
  let distance = Infinity;
  for (const offset of [caretOffset,caretOffset-1]) {
    if (offset<0 || offset>=caretNode.length) continue;
    range.setStart(caretNode,offset);
    range.setEnd(caretNode,offset+1);
    const rect = range.getBoundingClientRect();
    if (x<rect.left-1 || x>rect.right+1 || y<rect.top-1 || y>rect.bottom+1) continue;
    const candidate = Math.abs(x-(rect.left+rect.right)/2);
    if (candidate<distance) {best=offset;distance=candidate;}
  }
  return best===undefined ? null : pdfWordAt(text,Number(span.dataset.offset)+best);
}

type PdfWordBox = Position & {surface:string;left:number;top:number;width:number;height:number};

function pageHighlights(layer:HTMLElement, offset:number, native:Map<number,PdfWordBox>, positions:(Position & {id:string;hit:boolean;selected?:boolean})[]) {
  const origin = layer.getBoundingClientRect();
  const sheet = layer.parentElement;
  const scale = sheet ? sheet.getBoundingClientRect().width/Number.parseFloat(sheet.style.width) : 1;
  if (!scale) return [];
  const spans = Array.from(layer.querySelectorAll<HTMLElement>('span[data-offset]'));
  return positions.flatMap(position=>{
    const exact = native.get(position.start-offset);
    if (exact && exact.end+offset===position.end) {
      return [{...exact,id:position.id,hit:position.hit,selected:position.selected}];
    }
    return spans.flatMap(span=>{
    const node = span.firstChild;
    if (!(node instanceof Text)) return [];
    const start = Number(span.dataset.offset)+offset;
    const from = Math.max(0,position.start-start);
    const to = Math.min(node.length,position.end-start);
    if (from>=to) return [];
    const range = document.createRange();
    range.setStart(node,from);range.setEnd(node,to);
    const rects = span.classList.contains('ocr-word') && from===0 && to===node.length
      ? [span.getBoundingClientRect()] : Array.from(range.getClientRects());
    return rects.map(rect=>({id:position.id,hit:position.hit,selected:position.selected,
      left:(rect.left-origin.left)/scale,top:(rect.top-origin.top)/scale,
      width:rect.width/scale,height:rect.height/scale}));
    });
  });
}

export function PdfReader<M extends Mark>({id,marks,focused,selection,visible,toolbar,navigation,onMark,onSelect,onClearSelection}: {
  id:string;marks:M[];focused:string;selection:Selection|null;visible:boolean;toolbar:HTMLElement|null;navigation:ReaderNavigationState;onMark:(mark:M)=>void;
  onSelect:(selection:Selection,doubleClick:boolean)=>void;onClearSelection:()=>void;
}) {
  const [pdf,setPdf] = useState<PDFDocumentProxy|null>(null);
  const [pages,setPages] = useState<IndexedPage[]>([]);
  const positionKey = `pdfReadingPosition:${id}`;
  const [page,setPage] = useState(1);
  useEffect(()=>{
    if (!pdf) return;
    saveReaderPreference(positionKey,String(page));
    saveReaderPreference(`readingProgress:${id}`,`第 ${page} / ${pdf.numPages} 页`);
  },[pdf,page,id,positionKey]);
  const [zoom,setZoom] = useState(1);
  const requestedZoom = useRef(1);
  const paintedZoom = useRef(1);
  const zoomTimer = useRef<ReturnType<typeof setTimeout>|undefined>(undefined);
  const pinch = useRef<number|null>(null);
  const zoomFrame = useRef<number|undefined>(undefined);
  const zoomPoint = useRef<{x:number;y:number}|undefined>(undefined);
  const lastJump = useRef('');
  const [pageDraft,setPageDraft] = useState<string|null>(null);
  const bookmarkKey = `pdfBookmarks:${id}`;
  const [bookmarks,setBookmarks] = useState<number[]>(()=>{
    try {
      const saved:unknown = JSON.parse(localStorage.getItem(bookmarkKey) ?? '[]');
      return Array.isArray(saved) ? [...new Set(saved.filter(number=>Number.isSafeInteger(number) && number>0))] : [];
    } catch { return []; }
  });
  const [bookmarkError,setBookmarkError] = useState('');
  function toggleBookmark(number:number) {
    const next = bookmarks.includes(number) ? bookmarks.filter(value=>value!==number)
      : [...bookmarks,number].sort((a,b)=>a-b);
    try {
      localStorage.setItem(bookmarkKey,JSON.stringify(next));
      setBookmarks(next);setBookmarkError('');
    } catch { setBookmarkError('无法保存 PDF 书签');navigation.showBookmarks(); }
  }
  const [outline,setOutline] = useState<OutlineEntry[]|null>(null);
  const [outlineError,setOutlineError] = useState('');
  const outlineRequest = useRef(0);
  const [error,setError] = useState('');
  const [indexError,setIndexError] = useState('');
  const [retry,setRetry] = useState(0);
  const [width,setWidth] = useState(0);
  const [paint,setPaint] = useState(0);
  const renderedPage = useRef(0);
  const nativeBoxes = useRef(new Map<number,PdfWordBox>());
  const ignoreMarkClick = useRef(false);
  const geometryCache = useRef(new Map<number,NativeWord[]>());
  const [find,setFind] = useState(false);
  const [query,setQuery] = useState('');
  const [match,setMatch] = useState(0);
  const frame = useRef<HTMLDivElement>(null);
  const paper = useRef<HTMLDivElement>(null);
  const pageArea = useRef<HTMLDivElement>(null);
  const touch = useRef<{x:number;y:number}|null>(null);
  const canvasHost = useRef<HTMLDivElement>(null);
  const textHost = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const savedPages = useRef<IndexedPage[]>([]);
  const indexed = pages[page-1];

  useEffect(()=>{
    let cancelled = false;
    let task:Awaited<ReturnType<typeof loadPdf>>|undefined;
    void (async()=>{
      try {
        const saved = await call<{length:number;pages:IndexedPage[]}>('pdf_load',{id});
        if (cancelled) return;
        task = await loadPdf(id,saved.length,error=>{if (!cancelled) setError('无法读取 PDF · '+String(error));});
        if (cancelled) {await task.destroy();return;}
        const loaded = await task.promise;
        if (!cancelled) {
          const savedPage = Number(readReaderPreference(positionKey));
          setPage(Number.isSafeInteger(savedPage) && savedPage>=1 && savedPage<=loaded.numPages ? savedPage : 1);
          savedPages.current=saved.pages;
          setPages(saved.pages);
          setPdf(loaded);
        }
      } catch (error) {if (!cancelled) setError('无法打开 PDF · '+String(error));}
    })();
    return ()=>{cancelled=true;void task?.destroy();};
  },[id,positionKey]);

  useEffect(()=>{
    if (!pdf) return;
    let cancelled = false;
    let worker:Promise<Worker>|undefined;
    function ocr() {
      const tesseract = (window as unknown as {Tesseract:typeof import('tesseract.js')}).Tesseract;
      worker ??= tesseract.createWorker('eng',1,{workerPath:assets('./ocr/worker.min.js'),
        corePath:assets('./ocr/tesseract-core-lstm.wasm.js'),langPath:assets('./ocr/'),
        workerBlobURL:false,cacheMethod:'none'});
      return worker;
    }
    setIndexError('');
    void (async()=>{
      try {
        for (let number=savedPages.current.length+1; number<=pdf.numPages && !cancelled; number++) {
          const result = await indexPage(await pdf.getPage(number),ocr);
          if (cancelled) break;
          const {page,offset,text} = await call<IndexedPage>('pdf_index_page',{id,page:number,...result});
          const saved = {page,offset,text};
          if (cancelled) break;
          savedPages.current = [...savedPages.current,saved];
          setPages(savedPages.current);
        }
      } catch (error) {
        if (!cancelled) setIndexError(String(error));
      } finally {
        if (worker) await worker.then(active=>active.terminate(),()=>{});
      }
    })();
    return ()=>{cancelled=true;};
  },[id,pdf,retry]);

  useEffect(()=>{
    if (!pageArea.current) return;
    const observer = new ResizeObserver(([entry])=>setWidth(entry.contentRect.width));
    observer.observe(pageArea.current);
    return ()=>observer.disconnect();
  },[]);

  useEffect(()=>{
    if (!pdf || width<1) return;
    let cancelled = false;
    const renderer = new AbortController();
    let nativeLayer:InstanceType<(typeof import('pdfjs-dist'))['TextLayer']>|undefined;
    const host = textHost.current;
    const canvas = canvasHost.current;
    const sheet = paper.current;
    if (!host || !canvas || !sheet) return;
    void (async()=>{
      try {
        const source = await pdf.getPage(page);
        if (cancelled) return;
        const original = source.getViewport({scale:1});
        const scale = width / original.width * zoom;
        const viewport = source.getViewport({scale});
        const pixels = Math.min(2,window.devicePixelRatio,4096/Math.max(viewport.width,viewport.height),
          Math.sqrt(12_000_000/(viewport.width*viewport.height)));
        const result = await renderPage(source,scale,pixels,renderer.signal);
        if (cancelled) return;
        setError('');
        setPaint(0);
        renderedPage.current=0;
        nativeBoxes.current.clear();
        host.replaceChildren();
        host.removeAttribute('style');host.removeAttribute('data-main-rotation');
        sheet.style.width = result.viewport.width+'px';
        sheet.style.height = result.viewport.height+'px';
        sheet.style.setProperty('--total-scale-factor',String(scale*source.userUnit));
        canvas.replaceChildren(result.canvas);
        paintedZoom.current=zoom;
        sizePdfSheet(sheet,requestedZoom.current/zoom);
        if (indexed) {
          const rawLayer = await call<string>('pdf_page_layer',{id,page});
          if (cancelled) return;
          const layer = JSON.parse(rawLayer) as Layer;
          if ('native' in layer) {
            const engine = await pdfEngine();
            if (cancelled) return;
            nativeLayer = new engine.TextLayer({textContentSource:layer.native,container:host,viewport:result.viewport});
            await nativeLayer.render();
            if (cancelled) return;
            nativeLayer.textDivs.forEach((span,i)=>{span.dataset.offset=String(layer.offsets[i]);});
            let geometry = geometryCache.current.get(page);
            if (!geometry) {
              geometry = await call<NativeWord[]>('pdf_native_boxes',{id,page}).catch(()=>[]);
              if (cancelled) return;
              geometryCache.current.set(page,geometry);
              if (geometryCache.current.size>12) {
                const oldest = geometryCache.current.keys().next().value;
                if (oldest!==undefined) geometryCache.current.delete(oldest);
              }
            }
            for (const {rect,...word} of matchNativePdfWords(indexed.text,geometry)) {
              const [x1,y1,x2,y2] = result.viewport.convertToViewportRectangle(rect);
              nativeBoxes.current.set(word.start,{...word,left:Math.min(x1,x2),top:Math.min(y1,y2),
                width:Math.abs(x2-x1),height:Math.abs(y2-y1)});
            }
          } else {
            for (const [i,word] of layer.words.entries()) {
              const span = document.createElement('span');
              span.className = 'ocr-word';
              span.dataset.offset = String(layer.offsets[i]);
              span.textContent = word.text;
              Object.assign(span.style,{left:word.x*100+'%',top:word.y*100+'%',
                fontSize:word.h*result.viewport.height+'px'});
              host.append(span);
              const measured = span.offsetWidth;
              if (measured) span.style.transform = `scaleX(${word.w*result.viewport.width/measured})`;
            }
          }
        }
        renderedPage.current=indexed ? page : 0;
        setPaint(previous=>previous+1);
      } catch (error) {if (!cancelled) setError('无法显示 PDF 页面 · '+String(error));}
    })();
    return ()=>{cancelled=true;renderer.abort();nativeLayer?.cancel();};
  },[id,pdf,page,width,indexed,zoom]);

  const matches = useMemo(()=>pages.flatMap(item=>articleMatches([[{start:item.offset,text:item.text}]],query)
    .map(hit=>({...hit,page:item.page}))),[pages,query]);
  const active = matches[match%Math.max(1,matches.length)];
  useEffect(()=>{if(active) setPage(active.page);},[active]);
  useEffect(()=>{
    const mark = marks.find(item=>item.id===focused);
    const target = mark && pages.find(item=>mark.start>=item.offset && mark.start<item.offset+item.text.length);
    if (target) setPage(target.page);
  },[focused,pages]);
  function openSearch() {
    setFind(true);
    input.current?.focus({preventScroll:true});
    input.current?.select();
  }
  function closeSearch() {
    setFind(false);setQuery('');setMatch(0);
    frame.current?.focus({preventScroll:true});
  }
  useArticleSearchShortcut(visible,openSearch);
  useEffect(()=>{if(find) input.current?.focus({preventScroll:true});},[find]);
  const highlights = useMemo(()=>{
    if (!indexed || !textHost.current || !paint || renderedPage.current!==page) return [];
    const positions = marks.filter(mark=>mark.start>=indexed.offset && mark.start<indexed.offset+indexed.text.length)
      .map(mark=>({...mark,hit:false}));
    const selected = selection?.document_id===id && selection.start>=indexed.offset &&
      selection.end<=indexed.offset+indexed.text.length ? [{...selection,id:'selection',hit:false,selected:true}] : [];
    return pageHighlights(textHost.current,indexed.offset,nativeBoxes.current,[
      ...positions,...selected,...(active?.page===page ? [{...active,id:'search',hit:true}] : [])
    ]);
  },[id,indexed,marks,active,page,paint,selection]);
  useEffect(()=>{
    const id = active?.page===page ? 'search' : focused;
    if (!id) {lastJump.current='';return;}
    const key = id==='search' ? `search:${active?.page}:${active?.start}` : id;
    const scroller = pageArea.current;
    const target = paper.current?.querySelector<HTMLElement>('[data-current-highlight]');
    if (key===lastJump.current || !scroller || !target) return;
    lastJump.current=key;
    const viewport = scroller.getBoundingClientRect();
    const bounds = target.getBoundingClientRect();
    const horizontal = bounds.left<viewport.left ? bounds.left-viewport.left
      : Math.max(0,bounds.right-viewport.right);
    scroller.scrollBy(horizontal,bounds.top+(bounds.height-viewport.height)/2-viewport.top);
  },[active,focused,page,highlights]);

  function previewZoom(value:number, point?:{x:number;y:number}) {
    const sheet = paper.current;
    const scroller = pageArea.current;
    if (!sheet || !scroller || !sheet.style.width) return;
    const old = sheet.getBoundingClientRect();
    if (!old.width || !old.height) return;
    const viewport = scroller.getBoundingClientRect();
    const clamp = (number:number,min:number,max:number) => Math.min(max,Math.max(min,number));
    const x = clamp(point && Number.isFinite(point.x) ? point.x : viewport.left+viewport.width/2,old.left,old.right);
    const y = clamp(point && Number.isFinite(point.y) ? point.y : viewport.top+viewport.height/2,old.top,old.bottom);
    sizePdfSheet(sheet,value/paintedZoom.current);
    const next = sheet.getBoundingClientRect();
    scroller.scrollBy(
      next.left+(x-old.left)*next.width/old.width-x,
      next.top+(y-old.top)*next.height/old.height-y,
    );
  }

  function commitZoom() {
    clearTimeout(zoomTimer.current);
    setZoom(requestedZoom.current);
  }

  function setPdfZoom(value:number, point?:{x:number;y:number}) {
    if (!Number.isFinite(value)) return;
    const next = Math.min(3,Math.max(.5,Math.round(value*100)/100));
    if (next===requestedZoom.current) return;
    requestedZoom.current=next;
    zoomPoint.current=point;
    if (zoomFrame.current===undefined) {
      zoomFrame.current=requestAnimationFrame(()=>{
        zoomFrame.current=undefined;
        previewZoom(requestedZoom.current,zoomPoint.current);
      });
    }
    clearTimeout(zoomTimer.current);
    if (pinch.current===null) zoomTimer.current=setTimeout(commitZoom,point ? 100 : 0);
  }

  useEffect(()=>()=>{
    clearTimeout(zoomTimer.current);
    if (zoomFrame.current!==undefined) cancelAnimationFrame(zoomFrame.current);
  },[]);

  function markAtPoint(clientX:number,clientY:number) {
    const sheet = paper.current;
    if (!sheet || renderedPage.current!==page) return;
    const rect = sheet.getBoundingClientRect();
    const scale = rect.width/Number.parseFloat(sheet.style.width);
    const x = (clientX-rect.left)/scale, y = (clientY-rect.top)/scale;
    const hit = highlights.find(item=>!item.hit && !item.selected && x>=item.left &&
      x<=item.left+item.width && y>=item.top && y<=item.top+item.height);
    return hit && marks.find(mark=>mark.id===hit.id);
  }

  function select(point?:{x:number;y:number}) {
    if (!indexed || !textHost.current || renderedPage.current!==page) return;
    let native:PdfWordBox|undefined;
    const bounds = paper.current?.getBoundingClientRect();
    if (point && bounds && paper.current) {
      const scale = bounds.width/Number.parseFloat(paper.current.style.width);
      const x = (point.x-bounds.left)/scale, y = (point.y-bounds.top)/scale;
      native = Array.from(nativeBoxes.current.values()).find(box=>x>=box.left && x<=box.left+box.width
        && y>=box.top && y<=box.top+box.height);
    }
    const selected = point ? native ?? pdfWordAtPoint(textHost.current,indexed.text,point.x,point.y)
      : selectedOffsets(textHost.current,indexed.text,window.getSelection(),true);
    if (!selected || (point && !native && nativeBoxes.current.has(selected.start))) return;
    const start = indexed.offset+selected.start;
    const end = indexed.offset+selected.end;
    const existing = marks.find(mark=>mark.start<=start && mark.end>=end);
    ignoreMarkClick.current=true;
    window.getSelection()?.removeAllRanges();
    if (existing && point) onMark(existing);
    else onSelect({document_id:id,start,end,surface:selected.surface},Boolean(point));
  }
  function turn(number:number) {
    if (!pdf || number<1 || number>pdf.numPages || number===page) return;
    commitZoom();
    if (selection?.document_id===id) onClearSelection();
    setPage(number);
    pageArea.current?.scrollTo({top:0,left:0});
  }
  useEffect(()=>{
    if (!pdf || !navigation.open || outline) return;
    let cancelled = false;
    void pdf.getOutline().then(
      result=>{if (!cancelled) {setOutline(result ?? []);setOutlineError('');}},
      ()=>{if (!cancelled) setOutlineError('无法读取 PDF 目录');}
    );
    return ()=>{cancelled=true;};
  },[pdf,navigation.open,outline]);

  async function openOutline(destination:string|unknown[]) {
    if (!pdf) return;
    const request = ++outlineRequest.current;
    try {
      const target = typeof destination==='string' ? await pdf.getDestination(destination) : destination;
      const first = target?.[0];
      const index = typeof first==='number' ? first
        : first && typeof first==='object' && 'num' in first && 'gen' in first
          ? await pdf.getPageIndex(first as {num:number;gen:number}) : -1;
      if (request!==outlineRequest.current) return;
      if (!Number.isInteger(index) || index<0 || index>=pdf.numPages) throw Error();
      setOutlineError('');
      turn(index+1);
      pageArea.current?.scrollTo({top:0,left:0});
    } catch {
      if (request===outlineRequest.current) setOutlineError('无法跳转到这个目录项');
    }
  }

  useEffect(()=>{
    if (!visible || !paper.current) return;
    const surface = paper.current;
    const viewport = document.querySelector<HTMLMetaElement>('meta[name="viewport"]');
    const previousViewport = viewport?.content;
    if (viewport) viewport.content = 'width=device-width, initial-scale=1, maximum-scale=1, user-scalable=no';
    function overPdf(event:Event) {
      if (event.target instanceof Node && surface.contains(event.target)) return true;
      const {clientX,clientY} = event as MouseEvent;
      return Number.isFinite(clientX) && Number.isFinite(clientY) &&
        surface.contains(document.elementFromPoint(clientX,clientY));
    }
    const listeners = new AbortController();
    const gestures = readerGestures({
      zoom:()=>requestedZoom.current, update:setPdfZoom,
      turn:direction=>turn(page+direction), width:()=>surface.clientWidth,
      canTurn:()=>requestedZoom.current<=1,
      start:()=>{clearTimeout(zoomTimer.current);pinch.current=requestedZoom.current;},
      end:()=>{pinch.current=null;commitZoom();},
    });
    gestures.bind(window,listeners.signal,event=>Boolean(pdf) && overPdf(event));
    return ()=>{
      pinch.current=null;
      listeners.abort();
      if (viewport && previousViewport!==undefined) viewport.content=previousViewport;
    };
  },[visible,pdf,page]);

  const move = (step:number) => {if(matches.length) setMatch((match+step+matches.length)%matches.length);};
  return <div ref={frame} className={navigation.open ? 'pdf-reader has-outline' : 'pdf-reader'} tabIndex={0} onKeyDown={event=>{
    if (event.nativeEvent.isComposing || event.altKey || event.metaKey || event.ctrlKey || event.shiftKey ||
        (event.target instanceof HTMLElement && event.target.closest('input,textarea,select,[contenteditable],.article-search'))) return;
    if (event.key==='ArrowLeft' || event.key==='ArrowRight') {
      event.preventDefault();
      turn(page+(event.key==='ArrowRight' ? 1 : -1));
    }
  }}>
    <ReaderToolbar target={toolbar} disabled={!pdf} zoom={zoom} changeZoom={setPdfZoom} resetLabel="适合宽度"
      bookmark={{active:bookmarks.includes(page),disabled:!pdf,toggle:()=>toggleBookmark(page)}}
      previousDisabled={!pdf || page<=1} nextDisabled={!pdf || page>=pdf.numPages}
      previous={()=>turn(page-1)} next={()=>turn(page+1)}
      pageIndicator={<form onSubmit={event=>{
        event.preventDefault();
        const number = Number(pageDraft ?? page);
        if (Number.isInteger(number)) turn(number);
        setPageDraft(null);
      }}>
        <input type="text" inputMode="numeric" aria-label="输入页码，回车跳转" disabled={!pdf}
          value={pageDraft ?? String(page)} onFocus={event=>event.currentTarget.select()}
          onChange={event=>setPageDraft(event.target.value)} onBlur={()=>setPageDraft(null)}/>
        <span>/ {pdf?.numPages ?? '…'} 页</span>
      </form>}/>
    {find && <div className="article-search" role="search" aria-label="文章内搜索" onKeyDown={event=>{
      if (event.key==='Escape' && !event.nativeEvent.isComposing) {
        event.preventDefault();closeSearch();
      }
    }}>
      <input ref={input} type="search" aria-label="搜索当前 PDF" placeholder="搜索当前 PDF" value={query}
        onChange={event=>{setQuery(event.target.value);setMatch(0);}} onKeyDown={event=>{
          if (event.key==='Enter' && !event.nativeEvent.isComposing) {
            event.preventDefault();move(event.shiftKey ? -1 : 1);
          }
        }}/>
      <span role="status">{query.trim() ? matches.length ? `${match%matches.length+1} / ${matches.length}` : '无匹配' : ''}</span>
      <button type="button" disabled={!matches.length} onClick={()=>move(-1)}>上一处</button>
      <button type="button" disabled={!matches.length} onClick={()=>move(1)}>下一处</button>
      <button type="button" onClick={closeSearch}>关闭</button>
    </div>}
    {navigation.open && <ReaderNavigation navigation={navigation} bookmarks={<>
      {bookmarkError && <p role="alert">{bookmarkError}</p>}
      {bookmarks.length ? <ul>{bookmarks.filter(number=>number<=(pdf?.numPages ?? 0)).map(number=>
        <li className="bookmark-row" key={number}>
          <button type="button" aria-current={number===page ? 'page' : undefined} onClick={()=>turn(number)}>第 {number} 页</button>
          <button type="button" aria-label={`删除第 ${number} 页书签`} onClick={()=>toggleBookmark(number)}>×</button>
        </li>)}</ul> : <p className="muted">点击标题行的 ☆ 添加本页书签。</p>}
    </>} contents={<>
      {outlineError && <p role="alert">{outlineError}</p>}
      {outline ? outline.length>0 ? <OutlineTree items={outline} open={destination=>void openOutline(destination)}/>
        : <p className="muted">这份 PDF 没有内置目录。</p>
        : !outlineError && <p role="status">正在读取目录…</p>}
    </>}/>}
    <div ref={pageArea} className="pdf-page-area" onClick={event=>{
      if (event.target instanceof Node && paper.current?.contains(event.target)) return;
      if (selection?.document_id===id) onClearSelection();
    }}>
    <div className="pdf-status">
      {pdf && pages.length<pdf.numPages && !indexError &&
        <small role="status">正在建立文字索引 · {pages.length}/{pdf.numPages} 页</small>}
      {indexError && <p role="alert">文字索引暂停 · {indexError} <button onClick={()=>setRetry(value=>value+1)}>重试</button></p>}
      {indexed && !indexed.text.trim() && <small>本页未识别出可查词的文字。</small>}
      {error && <p role="alert">{error}</p>}
    </div>
    <div className="pdf-sheet">
    <div ref={paper} className={zoom>1 ? 'pdf-paper zoomed' : 'pdf-paper'}
      onMouseDown={()=>{ignoreMarkClick.current=false;}}
      onMouseMove={event=>textHost.current?.classList.toggle('over-pdf-mark',
        Boolean(markAtPoint(event.clientX,event.clientY)))}
      onMouseLeave={()=>textHost.current?.classList.remove('over-pdf-mark')}
      onClick={event=>{
        if (ignoreMarkClick.current || window.getSelection()?.isCollapsed===false) return;
        const mark = markAtPoint(event.clientX,event.clientY);
        if (mark) onMark(mark);
        else if (selection?.document_id===id) onClearSelection();
      }} onTouchStart={event=>{
      const point = event.touches.length===1 ? event.touches[0] : null;
      touch.current = point ? {x:point.clientX,y:point.clientY} : null;
    }} onTouchCancel={()=>{touch.current=null;}} onTouchEnd={event=>{
      const start = touch.current;
      touch.current=null;
      const point = event.changedTouches[0];
      if (zoom>1 || !start || !point || event.touches.length || window.getSelection()?.isCollapsed===false) return;
      const dx = point.clientX-start.x, dy = point.clientY-start.y;
      if (Math.abs(dx)>=64 && Math.abs(dx)>Math.abs(dy)*1.5) turn(page+(dx<0 ? 1 : -1));
    }}>
      <div ref={canvasHost} className="pdf-canvas"/>
      <div ref={textHost} className="textLayer"
        onMouseUp={event=>{if(event.detail<2) select();}}
        onDoubleClick={event=>select({x:event.clientX,y:event.clientY})}/>
      <div className="pdf-highlights" aria-hidden="true">
        {highlights.map(({id,hit,selected,left,top,width,height},i)=><span key={i}
          className={selected ? 'selected-word' : hit ? 'current' : id===focused ? 'saved-word current' : 'saved-word'}
          data-current-highlight={id===(active?.page===page ? 'search' : focused) ? '' : undefined}
          style={{left,top,width,height}}/>)}
      </div>
    </div>
    </div>
    </div>
  </div>;
}

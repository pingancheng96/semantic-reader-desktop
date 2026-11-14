import {ReaderToolbar} from './reader-toolbar.js';
import {ReaderNavigation, readReaderPreference, saveReaderPreference, type ReaderNavigationState} from './reader-navigation.js';
import {readerGestures} from './reader-gestures.js';
import {React, call} from './runtime.js';
import {useArticleSearchShortcut} from './article-search.js';
import type {WebNode} from './web-captions.js';

const {useEffect,useRef,useState} = React;
type Toc = {label:string;href:string;subitems?:Toc[]};
type Location = {cfi:string;tocItem?:Toc;fraction?:number};
type View = HTMLElement & {
  open(file:File):Promise<void>; init(options:object):Promise<void>; close():void;
  isFixedLayout?:boolean;
  renderer?:(HTMLElement & {getContents():{doc:Document;index:number}[]});
  book:{toc?:Toc[]}; goTo(target:string|number):Promise<unknown>; prev():Promise<void>; next():Promise<void>;
  getCFI(index:number,range:Range):string; lastLocation?:Location;
  addAnnotation(annotation:{value:string;id:string}):Promise<unknown>;
  deleteAnnotation(annotation:{value:string;id:string}):Promise<unknown>;
  search(options:{query:string}):AsyncIterable<{subitems?:{cfi:string;excerpt:{match:string}}[]}|'done'>;
  clearSearch():void;
};
type Anchor = {node:Text; offsets:number[]};
type Mark = {id:string;start:number;end:number};

function bounds(node:WebNode):number[] {
  if (node.type==='text') return [node.start,node.end];
  return node.type==='element' ? node.children.flatMap(bounds) : [];
}

function align(doc:Document, content:string, start:number, end:number):Anchor[] {
  const walker = doc.createTreeWalker(doc.body,NodeFilter.SHOW_TEXT);
  const anchors:Anchor[] = [];
  const sourceOffsets:number[] = [];
  for (let i=start;i<end;i++) if (!/\s/u.test(content[i])) sourceOffsets.push(i);
  const source = sourceOffsets.map(offset=>content[offset]).join('');
  let cursor = 0;
  for (let node=walker.nextNode() as Text|null;node;node=walker.nextNode() as Text|null) {
    if (node.parentElement?.closest('script,style,template,noscript,svg')) continue;
    const text = node.data.replace(/\s/gu,'');
    if (!text) continue;
    const match = source.indexOf(text,cursor);
    if (match<0 || match-cursor>64) continue;
    const offsets = new Array<number>(node.length+1).fill(-1);
    let index = match;
    for (let i=0;i<node.length;i++) {
      if (/\s/u.test(node.data[i])) continue;
      const offset = sourceOffsets[index++];
      offsets[i]=offset;
      offsets[i+1]=offset+1;
    }
    cursor=index;
    anchors.push({node,offsets});
  }
  return anchors;
}

export function EpubReader<M extends Mark>({id,content,layout,marks,focused,visible,toolbar,navigation,onSelect,onMark}:{
  id:string;content:string;layout:string;marks:M[];focused:string;visible:boolean;toolbar:HTMLElement|null;navigation:ReaderNavigationState;
  onSelect:(selection:{document_id:string;surface:string;start:number;end:number},double:boolean)=>void;onMark:(mark:M)=>void;
}) {
  const host=useRef<HTMLDivElement>(null);
  const view=useRef<View|null>(null);
  const sections=useRef(new Map<number,Anchor[]>());
  const overlays=useRef(new Map<string,string>());
  const callbacks=useRef({onSelect,onMark,marks});
  callbacks.current={onSelect,onMark,marks};
  const [zoom,setZoom] = useState(1);
  const zoomValue = useRef(1);
  useEffect(()=>{
    if (!visible) return;
    const viewport = document.querySelector<HTMLMetaElement>('meta[name="viewport"]');
    if (!viewport) return;
    const original = viewport.content;
    viewport.content = 'width=device-width, initial-scale=1, maximum-scale=1, user-scalable=no';
    return ()=>{viewport.content=original;};
  },[visible]);
  function changeZoom(next:number) {
    if (!Number.isFinite(next)) return;
    zoomValue.current=Math.max(.5,Math.min(3,next));
    setZoom(zoomValue.current);
  }
  function applyZoom(reader:View, doc:Document) {
    if (reader.isFixedLayout) {
      const value = zoomValue.current===1 ? 'fit-page' : String(zoomValue.current);
      if (reader.renderer?.getAttribute('zoom')!==value) reader.renderer?.setAttribute('zoom',value);
    }
    else doc.body?.style.setProperty('zoom',String(zoomValue.current),'important');
  }
  useEffect(()=>{
    const reader=view.current;
    if (!reader) return;
    for (const {doc} of reader.renderer?.getContents() ?? []) applyZoom(reader,doc);
  },[zoom]);
  const [ready,setReady]=useState(false);
  const [error,setError]=useState('');
  const [toc,setToc]=useState<Toc[]>([]);
  const positionKey = `epubReadingPosition:${id}`;
  const [location,setLocation]=useState<Location|null>(null);
  const [revision,setRevision]=useState(0);
  const key=`epubBookmarks:${id}`;
  const [bookmarks,setBookmarks]=useState<{cfi:string;label:string}[]>(()=>{
    try {
      const saved=JSON.parse(localStorage.getItem(key) ?? '[]');
      return Array.isArray(saved) ? saved.filter(item=>typeof item?.cfi==='string' && typeof item?.label==='string') : [];
    } catch {return [];}
  });
  const [searchOpen,setSearchOpen]=useState(false);
  const [query,setQuery]=useState('');
  const [results,setResults]=useState<{cfi:string;excerpt:{match:string}}[]>([]);
  const [searching,setSearching]=useState(false);
  const searchVersion=useRef(0);
  useArticleSearchShortcut(visible,()=>setSearchOpen(true));

  function navigate(action:()=>Promise<unknown>) {
    void action().catch(()=>setError('无法跳转到这个位置'));
  }

  useEffect(()=>{
    let alive=true;
    let renderer:View|undefined;
    let poll:number|undefined;
    const listeners = new AbortController();
    let pointerOverBook = false;
    const gestures = readerGestures({
      zoom:()=>zoomValue.current, update:changeZoom,
      turn:direction=>{const reader=renderer;if (reader) navigate(()=>direction>0 ? reader.next() : reader.prev());},
      width:()=>host.current?.clientWidth ?? 1, threshold:55, shiftScroll:true, pinch:false,
    });
    window.addEventListener('pointermove',event=>{
      pointerOverBook=event.composedPath().includes(host.current!);
    },{capture:true,signal:listeners.signal});
    window.addEventListener('blur',()=>{pointerOverBook=false;gestures.reset();},{signal:listeners.signal});
    gestures.bind(window,listeners.signal,event=>{
      const area=host.current;
      if (!renderer || !area?.getClientRects().length) return false;
      if (event.composedPath().includes(area)) return true;
      const {clientX:x,clientY:y}=event as MouseEvent;
      const rect=area.getBoundingClientRect();
      if (x>=rect.left && x<rect.right && y>=rect.top && y<rect.bottom) return true;
      const zoom=event.type.startsWith('gesture') || (event as WheelEvent).ctrlKey;
      return zoom && (pointerOverBook || Boolean(renderer.renderer?.getContents().some(({doc})=>doc.hasFocus())));
    });
    const chapterNodes=(JSON.parse(layout) as {nodes:WebNode[]}).nodes;
    void (async()=>{
      const module=new URL('./foliate/view.js',import.meta.url).href;
      const [encoded]=await Promise.all([call<string>('epub_original',{id}),import(module)]);
      if (!alive) return;
      const bytes=Uint8Array.from(atob(encoded),character=>character.charCodeAt(0));
      renderer=document.createElement('foliate-view') as View;
      view.current=renderer;host.current?.append(renderer);
      renderer.addEventListener('external-link',event=>event.preventDefault());
      renderer.addEventListener('relocate',event=>{
        if (alive) {
          const current = (event as CustomEvent<Location>).detail;
          setLocation(current);
          if (current.cfi) saveReaderPreference(positionKey,current.cfi);
          const percent = typeof current.fraction==='number' ? `${Math.round(current.fraction*100)}%` : '';
          const progress = [current.tocItem?.label,percent].filter(Boolean).join(' · ');
          if (progress) saveReaderPreference(`readingProgress:${id}`,progress);
        }
      });
      const attached = new WeakSet<Document>();
      const selectionReaders = new WeakMap<Document,()=>void>();
      function attach({doc,index}:{doc:Document;index:number}) {
        if (attached.has(doc) || !doc.body) return;
        attached.add(doc);
        if (renderer) applyZoom(renderer,doc);
        doc.addEventListener('pointermove',()=>{pointerOverBook=true;},{signal:listeners.signal});
        gestures.bind(doc,listeners.signal,()=>alive);
        if (doc.defaultView) gestures.bind(doc.defaultView,listeners.signal,()=>alive);
        const chapter=chapterNodes[index];
        if (!chapter) return;
        const offsets=bounds(chapter);
        if (!offsets.length) return;
        const first=offsets[0];
        const next=chapterNodes.slice(index+1).find(node=>bounds(node).length>0);
        const last=next ? bounds(next)[0] ?? content.length : content.length;
        const headingLength=content.slice(first,last).match(/^第 \d+ 章\n\n/)?.[0].length ?? 0;
        let anchors=align(doc,content,first+headingLength,last);
        sections.current.set(index,anchors);
        setRevision(value=>value+1);
        function wordAt(event:MouseEvent) {
          const hit = doc.caretRangeFromPoint?.(event.clientX,event.clientY);
          const target = event.target;
          const point = hit?.startContainer;
          const offsetAtPoint = hit?.startOffset;
          if (!point || offsetAtPoint===undefined || !target) return false;
          const anchor = anchors.find(item=>item.node===point);
          if (!anchor) return false;
          for (const word of anchor.node.data.matchAll(/[\p{L}\p{M}\p{N}]+(?:[’'-][\p{L}\p{M}\p{N}]+)*/gu)) {
            const first = word.index!;
            const last = first + word[0].length;
            if (offsetAtPoint<first || offsetAtPoint>last) continue;
            const start = anchor.offsets[first], end = anchor.offsets[last];
            if (start<0 || end<=start || content.slice(start,end)!==word[0]) continue;
            const existing = callbacks.current.marks.find(mark=>mark.start<=start && mark.end>=end);
            if (existing) callbacks.current.onMark(existing);
            else callbacks.current.onSelect({document_id:id,start,end,surface:content.slice(start,end)},true);
            return true;
          }
          return false;
        }
        let lastSelection = '';
        const capture=(double:boolean)=>{

          const selection=doc.getSelection();
          if (!selection || selection.isCollapsed || !selection.rangeCount) return false;
          const range=selection.getRangeAt(0);
          if (!anchors.some(anchor=>range.intersectsNode(anchor.node))) {
            anchors=align(doc,content,first+headingLength,last);
            sections.current.set(index,anchors);
          }
          const selected=anchors.filter(anchor=>range.intersectsNode(anchor.node));
          const positions=selected.flatMap(anchor=>{
            const prefix=doc.createRange();
            prefix.selectNodeContents(anchor.node);
            const first=range.startContainer===anchor.node ? range.startOffset : range.comparePoint(anchor.node,0)<0
              ? (prefix.setEnd(range.startContainer,range.startOffset),prefix.toString().length) : 0;
            prefix.selectNodeContents(anchor.node);
            const last=range.endContainer===anchor.node ? range.endOffset : range.comparePoint(anchor.node,anchor.node.length)>0
              ? (prefix.setEnd(range.endContainer,range.endOffset),prefix.toString().length) : anchor.node.length;
            return anchor.offsets.slice(first,last+1).filter(offset=>offset>=0);
          });
          const start=positions[0],end=positions.at(-1);
          if (start===undefined || end===undefined || end<=start) return false;
          const surface=content.slice(start,end);
          if (surface.replace(/\s+/g,' ').trim()!==selection.toString().replace(/\s+/g,' ').trim()) return false;
          const signature = `${start}:${end}`;
          if (lastSelection===signature) return true;
          lastSelection=signature;
          setError('');
          const existing = double && callbacks.current.marks.find(mark=>mark.start<=start && mark.end>=end);
          if (existing) callbacks.current.onMark(existing);
          else callbacks.current.onSelect({document_id:id,start,end,surface},double);
          return true;
        };
        let doubleTimer: number | undefined;
        const doubleClick = (event:MouseEvent)=>{
          window.clearTimeout(doubleTimer);
          doubleTimer=window.setTimeout(()=>{
            if (!alive || !doc.defaultView || capture(true) || wordAt(event)) return;
            setError('未能把选中的单词定位到 EPUB 正文。');
          },0);
        };
        // WebKit reports the native word selection inside the book frame even
        // when the corresponding mouse event is unavailable to the host view.
        selectionReaders.set(doc,()=>{
          const selected = doc.getSelection()?.toString().trim() ?? '';
          if (!selected) { lastSelection='';return; }
          capture(!/\s/u.test(selected));
        });
        let selectionTimer: number | undefined;
        doc.addEventListener('selectionchange',()=>{
          window.clearTimeout(selectionTimer);
          if (doc.getSelection()?.isCollapsed) {
            lastSelection='';
            return;
          }
          selectionTimer=window.setTimeout(()=>{
            if (!alive || !doc.defaultView) return;
            const text = doc.getSelection()?.toString().trim() ?? '';
            if (text) capture(!/\s/u.test(text));
          },120);
        });
        doc.addEventListener('mouseup',event=>{
          if (event.detail===2) doubleClick(event);
          else if (event.detail<2) capture(false);
        },true);
        doc.addEventListener('dblclick',doubleClick,true);
        doc.addEventListener('keydown',event=>{
          if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase()==='f') {
            event.preventDefault();setSearchOpen(true);
          }
        });
      }
      renderer.addEventListener('show-annotation',event=>{
        const {value,range}=(event as CustomEvent<{value:string;range:Range}>).detail;
        if (range.startContainer.ownerDocument?.getSelection()?.isCollapsed===false) return;
        const mark=callbacks.current.marks.find(mark=>overlays.current.get(mark.id)===value);
        if (mark) callbacks.current.onMark(mark);
      });
      renderer.addEventListener('load',event=>attach((event as CustomEvent<{doc:Document;index:number}>).detail));
      poll=window.setInterval(()=>{
        for (const section of renderer?.renderer?.getContents() ?? []) {
          attach(section);
          selectionReaders.get(section.doc)?.();
        }
      },180);
      renderer.addEventListener('draw-annotation',event=>{
        const {draw,annotation}=(event as CustomEvent<{draw:(fn:(rects:DOMRect[])=>SVGElement)=>void;annotation:{id:string}}>).detail;
        draw(rects=>{
          const group=document.createElementNS('http://www.w3.org/2000/svg','g');
          group.style.pointerEvents='visiblePainted';
          group.style.cursor='pointer';
          group.addEventListener('click',click=>{
            const mark=callbacks.current.marks.find(mark=>mark.id===annotation.id);
            if (!mark) return;
            click.stopPropagation();
            callbacks.current.onMark(mark);
          });
          for (const rect of rects) {
            const shapes = [
              {x:rect.left,y:rect.top,width:rect.width,height:rect.height,fill:'#a8ce88','fill-opacity':.35,rx:3},
              {x:rect.left,y:rect.bottom-2,width:rect.width,height:2,fill:'#56886b'},
            ];
            for (const attributes of shapes) {
              const shape=document.createElementNS(group.namespaceURI,'rect');
              for (const [name,value] of Object.entries(attributes)) shape.setAttribute(name,String(value));
              group.append(shape);
            }
          }
          return group;
        });
      });
      await renderer.open(new File([bytes], 'book.epub',{type:'application/epub+zip'}));
      if (!alive) {renderer.close();return;}
      setToc(renderer.book.toc ?? []);
      await renderer.init({lastLocation:readReaderPreference(positionKey)});
      if (alive) setReady(true);
    })().catch(()=>{if(alive)setError('无法打开原版 EPUB');});
    return ()=>{alive=false;listeners.abort();window.clearInterval(poll);searchVersion.current++;renderer?.close();renderer?.remove();view.current=null;sections.current.clear();overlays.current.clear();};
  },[id,content,layout,positionKey]);

  useEffect(()=>{
    const reader=view.current;
    if (!ready || !reader) return;
    for (const [id,cfi] of overlays.current) void reader.deleteAnnotation({id,value:cfi}).catch(()=>{});
    overlays.current.clear();
    for (const mark of marks) for (const [index,anchors] of sections.current) {
      const start=anchors.find(anchor=>anchor.offsets.includes(mark.start));
      const end=[...anchors].reverse().find(anchor=>anchor.offsets.includes(mark.end));
      if (!start || !end) continue;
      const range=start.node.ownerDocument.createRange();
      range.setStart(start.node,start.offsets.indexOf(mark.start));
      range.setEnd(end.node,end.offsets.indexOf(mark.end));
      const cfi=reader.getCFI(index,range);
      overlays.current.set(mark.id,cfi);
      void reader.addAnnotation({id:mark.id,value:cfi}).catch(()=>{});
      break;
    }
  },[ready,marks,revision]);

  useEffect(()=>{
    if (!ready || !focused) return;
    const mark=marks.find(mark=>mark.id===focused);
    const chapters=(JSON.parse(layout) as {nodes:WebNode[]}).nodes;
    const index=chapters.findIndex(chapter=>{
      const offsets=bounds(chapter);
      return mark && offsets[0]<=mark.start && (offsets.at(-1) ?? -1)>=mark.end;
    });
    if (index>=0 && view.current) navigate(()=>view.current!.goTo(overlays.current.get(focused) ?? index));
  },[ready,focused,marks,layout]);

  function saveBookmarks(next:typeof bookmarks) {
    try {localStorage.setItem(key,JSON.stringify(next));setBookmarks(next);}
    catch {setError('无法保存 EPUB 书签');}
  }
  async function search() {
    if (!view.current || !query.trim()) return;
    const request=++searchVersion.current;
    setResults([]);setSearching(true);
    try {
      for await (const result of view.current.search({query:query.trim()})) {
        if (request!==searchVersion.current) break;
        if (result!=='done' && result.subitems) setResults(previous=>[...previous,...result.subitems!]);
      }
    } catch {if(request===searchVersion.current)setError('无法搜索 EPUB');}
    finally {if(request===searchVersion.current)setSearching(false);}
  }
  function renderToc(items:Toc[]) {
    return <ul>{items.map((item,index)=><li key={index}>
      <button onClick={()=>navigate(()=>view.current!.goTo(item.href))}>{item.label}</button>
      {item.subitems?.length ? renderToc(item.subitems) : null}
    </li>)}</ul>;
  }
  const saved=bookmarks.some(item=>item.cfi===location?.cfi);
  return <div className={navigation.open ? 'pdf-reader has-outline' : 'pdf-reader'}>
    <ReaderToolbar target={toolbar} disabled={!ready} zoom={zoom} changeZoom={changeZoom}
      bookmark={{active:saved,disabled:!location,toggle:()=>{
        if (location) saveBookmarks(saved ? bookmarks.filter(item=>item.cfi!==location.cfi) :
          [...bookmarks,{cfi:location.cfi,label:location.tocItem?.label || `书签 ${bookmarks.length+1}`}]);
      }}}
      previous={()=>navigate(()=>view.current!.prev())} next={()=>navigate(()=>view.current!.next())}>
      <button type="button" disabled={!ready} aria-label="书内搜索" onClick={()=>setSearchOpen(!searchOpen)}>⌕</button>
    </ReaderToolbar>
    {navigation.open && <ReaderNavigation navigation={navigation} bookmarks={<>
      {bookmarks.length ? <ul>{bookmarks.map(item=><li key={item.cfi} className="bookmark-row">
        <button onClick={()=>navigate(()=>view.current!.goTo(item.cfi))}>{item.label}</button>
        <button aria-label={`删除书签：${item.label}`} onClick={()=>saveBookmarks(bookmarks.filter(other=>other.cfi!==item.cfi))}>×</button>
      </li>)}</ul> : <p>点击标题行的 ☆ 添加书签。</p>}
    </>} contents={toc.length ? renderToc(toc) : <p>这本书没有内置目录。</p>}/>}
    <div className="epub-native">
      {error && <p role="alert">{error}</p>}
      {!ready && !error && <p role="status">正在打开 EPUB…</p>}
      {searchOpen && <div className="epub-search">
        <form className="row" onSubmit={event=>{event.preventDefault();void search();}}>
          <input autoFocus type="search" aria-label="搜索当前书籍" value={query} onChange={event=>setQuery(event.target.value)}/>
          <button disabled={!ready || searching}>搜索</button>
          <button type="button" onClick={()=>{searchVersion.current++;setSearching(false);setSearchOpen(false);view.current?.clearSearch();}}>关闭</button>
        </form>
        {results.map((result,index)=><button key={index} onClick={()=>navigate(()=>view.current!.goTo(result.cfi))}>{result.excerpt.match}</button>)}
      </div>}
      <div className="epub-native-host" ref={host}/>
    </div>
  </div>;
}

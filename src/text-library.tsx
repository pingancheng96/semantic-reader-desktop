import {BookCover} from './book-cover.js';
import type {ReactNode} from 'react';
import {React} from './runtime.js';
import {readReaderPreference, saveReaderPreference} from './reader-navigation.js';

type DocumentItem = {id:string; title:string; format:string; source_url?:string|null};
type Collection = {id:string; name:string; documents:string[]; parent?:string};
const collectionKey = 'textLibraryCollections';

function displayTitle(title:string) {
  return title.replace(/\.(md|markdown|txt|pdf|epub)$/i,'') || title;
}

function bookProgress(doc:DocumentItem): number|null {
  const saved = readReaderPreference(`readingProgress:${doc.id}`) ?? '';
  const pages = doc.format==='pdf' ? /^第 (\d+) \/ (\d+) 页$/.exec(saved) : null;
  const percent = (doc.format==='epub' || doc.source_url) ? /(?:^| · )(\d+)%$/.exec(saved) : null;
  const value = pages ? 100*Number(pages[1])/Number(pages[2]) : percent ? Number(percent[1]) : NaN;
  return Number.isFinite(value) ? Math.round(Math.max(0,Math.min(100,value))) : null;
}

function showLibraryDialog(dialog:HTMLDialogElement|null) {
  if (dialog && !dialog.open) dialog.showModal();
}

export function TextLibrary({documents,recent,busy,visible,imports,open,rename,remove}:{
  documents:DocumentItem[]; recent:Record<string,number>; busy:boolean; visible:boolean; imports:ReactNode;
  remove:(id:string,done:()=>void)=>void; open:(id:string)=>void; rename:(id:string,title:string,done:()=>void)=>void;
}) {
  const [collections,setCollections] = React.useState<Collection[]>(()=>{
    try { return JSON.parse(readReaderPreference(collectionKey)) || []; }
    catch { return []; }
  });
  const [current,setCurrent] = React.useState('all');
  const [view,setView] = React.useState(readReaderPreference('textLibraryView')==='list' ? 'list' : 'shelf');
  const [query,setQuery] = React.useState('');
  const [sort,setSort] = React.useState('imported');
  const [selected,setSelected] = React.useState<string[]>([]);
  const [selecting,setSelecting] = React.useState(false);
  const [folderEdit,setFolderEdit] = React.useState<{id:string;name:string;parent?:string}|null>(null);
  const [titleEdit,setTitleEdit] = React.useState<{id:string;title:string}|null>(null);
  const [deleteTarget,setDeleteTarget] = React.useState<DocumentItem|null>(null);
  const [assign,setAssign] = React.useState<string[]>([]);
  const [destination,setDestination] = React.useState('');
  const [newCategory,setNewCategory] = React.useState('');
  const [menu,setMenu] = React.useState<{doc:DocumentItem;x:number;y:number;trigger:HTMLButtonElement}|null>(null);
  const menuRef = React.useRef<HTMLDivElement>(null);
  React.useLayoutEffect(()=>{
    const element = menuRef.current;
    if (!menu || !element) return;
    const bounds = element.getBoundingClientRect();
    element.style.left = `${Math.max(4,Math.min(menu.x,window.innerWidth-bounds.width-4))}px`;
    element.style.top = `${Math.max(4,Math.min(menu.y,window.innerHeight-bounds.height-4))}px`;
    element.querySelector('button')?.focus({preventScroll:true});
    function dismiss(event:PointerEvent) {
      if (!element?.contains(event.target as Node)) setMenu(null);
    }
    function close() { setMenu(null); }
    window.addEventListener('pointerdown',dismiss);
    window.addEventListener('scroll',close,true);
    window.addEventListener('resize',close);
    return ()=>{
      window.removeEventListener('pointerdown',dismiss);
      window.removeEventListener('scroll',close,true);
      window.removeEventListener('resize',close);
    };
  },[menu]);
  const folder = collections.find(item=>item.id===current);
  const parent = collections.find(item=>item.id===folder?.parent);
  const children = collections.filter(item=>item.parent===current);
  const roots = collections.filter(item=>!item.parent);
  const scoped = folder ? [folder,...children] : [];
  const [searchAll,setSearchAll] = React.useState(false);
  const normalizedQuery = query.trim().toLowerCase();
  const shown = documents.filter(doc=>{
    const member = normalizedQuery && searchAll ? true : folder ? (normalizedQuery ? scoped : [folder]).some(item=>item.documents.includes(doc.id))
      : current==='recent' ? Boolean(recent[doc.id])
      : current==='uncategorized' ? !collections.some(item=>item.documents.includes(doc.id)) : true;
    return member && [doc.title,doc.source_url ?? ''].some(text=>text.toLowerCase().includes(normalizedQuery));
  });
  if (sort==='title') shown.sort((a,b)=>a.title.localeCompare(b.title,undefined,{numeric:true}));
  if (sort==='recent' || current==='recent' && sort==='imported') shown.sort((a,b)=>(recent[b.id] ?? 0)-(recent[a.id] ?? 0));
  if (sort==='manual' && folder && !normalizedQuery) {
    const positions = new Map(folder.documents.map((id,index)=>[id,index]));
    shown.sort((a,b)=>positions.get(a.id)!-positions.get(b.id)!);
  }
  function moveDocument(id:string,step:number) {
    if (!folder) return;
    const index = shown.findIndex(doc=>doc.id===id);
    const neighbor = shown[index+step];
    if (index<0 || !neighbor) return;
    const order = [...folder.documents];
    const from = order.indexOf(id);
    const to = order.indexOf(neighbor.id);
    if (from<0 || to<0) return;
    [order[from],order[to]] = [order[to],order[from]];
    update(collections.map(item=>item.id===folder.id ? {...item,documents:order} : item));
    setMenu(null);
  }
  function update(next:Collection[]) {
    setCollections(next);
    saveReaderPreference(collectionKey,JSON.stringify(next));
  }
  function switchCollection(id:string) {
    setCurrent(id);
    setSort(collections.some(item=>item.id===id) ? 'manual' : 'imported');
    setSearchAll(false);
    setFolderEdit(null);
    setSelected([]);
    setAssign([]);
  }
  function beginAssign(ids:string[]) {
    setDestination(collections[0]?.id ?? '');
    setNewCategory('');
    setAssign(ids);
  }
  function format(doc:DocumentItem) {
    return doc.source_url ? '网页' : doc.format==='md' ? 'Markdown' : doc.format.toUpperCase();
  }
  function progress(doc:DocumentItem) {
    const saved = readReaderPreference(`readingProgress:${doc.id}`);
    if (saved) return saved;
    if (doc.format==='pdf') {
      const page = readReaderPreference(`pdfReadingPosition:${doc.id}`);
      if (page) return `第 ${page} 页`;
    }
    return '继续阅读';
  }
  return <main className="text-library">
    <aside className="text-library-sidebar" aria-label="文本库分类">
      <h1>文本库</h1>
      <nav aria-label="收藏夹">
        {[{id:'all',name:'全部文本'},{id:'recent',name:'最近阅读'},{id:'uncategorized',name:'未归类'}]
          .map(item=><button type="button" key={item.id} aria-current={current===item.id ? 'page' : undefined}
            onClick={()=>switchCollection(item.id)}>{item.name}</button>)}
        {roots.map(root=><React.Fragment key={root.id}>
          <button type="button" aria-current={current===root.id ? 'page' : undefined}
            onClick={()=>switchCollection(root.id)}>{root.name}</button>
          {(current===root.id || parent?.id===root.id) && collections.filter(item=>item.parent===root.id).map(child=>
            <button className="library-child" type="button" key={child.id} aria-current={current===child.id ? 'page' : undefined}
              onClick={()=>switchCollection(child.id)}>{child.name}</button>)}
        </React.Fragment>)}
      </nav>
      <button type="button" onClick={()=>setFolderEdit({id:'',name:''})}>＋ 新建分类</button>
      {folder && <div className="row">
        {!folder.parent && <button type="button" onClick={()=>setFolderEdit({id:'',name:'',parent:folder.id})}>＋ 子分类</button>}
        <button type="button" onClick={()=>setFolderEdit(folder)}>改名</button>
        <button type="button" onClick={()=>{
          if (!window.confirm(`删除分类「${folder.name}」及其子分类？其中的文档会保留在文本库。`)) return;
          update(collections.filter(item=>item.id!==folder.id && item.parent!==folder.id));
          switchCollection('all');
        }}>删除分类</button>
      </div>}
      {folderEdit && <form onSubmit={event=>{
        event.preventDefault();
        const name = folderEdit.name.trim();
        if (!name || collections.some(item=>item.id!==folderEdit.id && item.parent===folderEdit.parent && item.name===name)) return;
        const id = folderEdit.id || crypto.randomUUID();
        update(folderEdit.id ? collections.map(item=>item.id===id ? {...item,name} : item)
          : [...collections,{id,name,parent:folderEdit.parent,documents:[]}]);
        setFolderEdit(null);
        switchCollection(id);
      }}>
        <input autoFocus required maxLength={60} aria-label="分类名称" placeholder={folderEdit.parent ? '子分类名称' : '分类名称'}
          value={folderEdit.name} onChange={event=>setFolderEdit({...folderEdit,name:event.target.value})}/>
        <div className="row">
          <button type="submit" disabled={!folderEdit.name.trim() || collections.some(item=>item.id!==folderEdit.id && item.parent===folderEdit.parent && item.name===folderEdit.name.trim())}>保存</button>
          <button type="button" onClick={()=>setFolderEdit(null)}>取消</button>
        </div>
      </form>}
    </aside>
    <section className="text-library-content">
      {folder && <nav className="library-path" aria-label="当前位置">
        <button type="button" onClick={()=>switchCollection('all')}>文本库</button>
        {parent && <> › <button type="button" onClick={()=>switchCollection(parent.id)}>{parent.name}</button></>}
        <span> › {folder.name}</span>
      </nav>}
      <header className="text-library-header">
        <h2>{folder?.name ?? (current==='recent' ? '最近阅读' : current==='uncategorized' ? '未归类' : '全部文本')}</h2>
        {imports}
      </header>
      <div className="text-library-controls">
        <input type="search" aria-label="搜索文本" placeholder="搜索标题或网址" value={query}
          onChange={event=>setQuery(event.target.value)}/>
        {normalizedQuery && current!=='all' && <button type="button" aria-pressed={searchAll}
          onClick={()=>{setSearchAll(!searchAll);setSelected([]);setAssign([]);}}>搜索全库</button>}
        <select aria-label="排序" value={sort} onChange={event=>setSort(event.target.value)}>
          {folder && <option value="manual">手动排序</option>}
          <option value="imported">{current==='recent' ? '默认顺序' : '最近导入'}</option>
          <option value="title">标题</option>
          <option value="recent">最近阅读</option>
        </select>
        <div role="group" aria-label="显示方式">
          {(['shelf','list'] as const).map(mode=><button type="button" key={mode} aria-pressed={mode===view}
            onClick={()=>{setView(mode);saveReaderPreference('textLibraryView',mode);}}>{mode==='shelf' ? '书架' : '列表'}</button>)}
        </div>
        <button type="button" aria-pressed={selecting} onClick={()=>{setSelecting(!selecting);setSelected([]);setAssign([]);}}>
          {selecting ? '退出多选' : '多选'}
        </button>
        {selecting && <button type="button" disabled={!selected.length}
          onClick={()=>beginAssign(selected)}>归类所选（{selected.length}）</button>}
      </div>
      {assign.length>0 && <dialog className="library-dialog" ref={showLibraryDialog}
        aria-label="加入分类" onCancel={()=>setAssign([])}>
      <form className="text-library-editor" onSubmit={event=>{
        event.preventDefault();
        if (destination) {
          update(collections.map(item=>item.id===destination
            ? {...item,documents:[...new Set([...item.documents,...assign])]} : item));
        } else {
          const name = newCategory.trim();
          if (!name || collections.some(item=>!item.parent && item.name===name)) return;
          update([...collections,{id:crypto.randomUUID(),name,documents:assign}]);
        }
        setAssign([]);setSelected([]);
      }}>
        <label>加入分类<select value={destination} onChange={event=>setDestination(event.target.value)}>
          <option value="">新建一级分类</option>
          {roots.map(root=><optgroup key={root.id} label={root.name}>
            <option value={root.id}>{root.name}</option>
            {collections.filter(item=>item.parent===root.id).map(item=>
              <option key={item.id} value={item.id}>{root.name} › {item.name}</option>)}
          </optgroup>)}
        </select></label>
        {!destination && <label>分类名称<input autoFocus required maxLength={60} value={newCategory}
          onChange={event=>setNewCategory(event.target.value)}/></label>}
        <button type="submit" className="primary" disabled={!destination && (!newCategory.trim()
          || collections.some(item=>!item.parent && item.name===newCategory.trim()))}>加入</button>
        <button type="button" onClick={()=>setAssign([])}>取消</button>
      </form></dialog>}
      {deleteTarget && <dialog className="library-dialog" ref={showLibraryDialog}
        aria-label="确认删除文本" onCancel={()=>setDeleteTarget(null)}>
        <div className="text-library-editor">
          <p>删除《{displayTitle(deleteTarget.title)}》及其原文标注和随标注保存的学习提示？词库义项会保留。删除后无法在应用内撤销。</p>
          <button type="button" autoFocus disabled={busy} onClick={()=>setDeleteTarget(null)}>取消</button>
          <button type="button" className="delete-document" disabled={busy} onClick={()=>remove(deleteTarget.id,()=>{
            update(collections.map(item=>({...item,documents:item.documents.filter(id=>id!==deleteTarget.id)})));
            setSelected(current=>current.filter(id=>id!==deleteTarget.id));
            setDeleteTarget(null);
          })}>确认删除</button>
        </div>
      </dialog>}
      {titleEdit && <dialog className="library-dialog" ref={showLibraryDialog}
        aria-label="重命名" onCancel={()=>setTitleEdit(null)}>
      <form className="text-library-editor" onSubmit={event=>{
        event.preventDefault();
        if (!busy && titleEdit.title.trim()) rename(titleEdit.id,titleEdit.title.trim(),()=>setTitleEdit(null));
      }}>
        <label>标题<input autoFocus required maxLength={255} value={titleEdit.title}
          onChange={event=>setTitleEdit({...titleEdit,title:event.target.value})}/></label>
        <button type="submit" className="primary" disabled={busy || !titleEdit.title.trim()}>保存</button>
        <button type="button" onClick={()=>setTitleEdit(null)}>关闭</button>
      </form></dialog>}
      {!normalizedQuery && children.length>0 && <div className="library-sections">
        {children.map(child=><button type="button" key={child.id} onClick={()=>switchCollection(child.id)}>
          <strong>{child.name}</strong>
          <small>{documents.filter(doc=>child.documents.includes(doc.id)).length} 项</small>
        </button>)}
      </div>}
      <ul className={`text-library-items library-${view}`}>
        {shown.map(doc=>{
          const percent = bookProgress(doc);
          return <li key={doc.id} className={selecting ? 'library-selecting' : undefined}>
          <button className="library-document" type="button" disabled={busy} onClick={()=>open(doc.id)}
            onContextMenu={event=>{
              event.preventDefault();
              setMenu({doc,x:event.clientX,y:event.clientY,trigger:event.currentTarget});
            }}
            onKeyDown={event=>{
              if (event.key!=='ContextMenu' && !(event.shiftKey && event.key==='F10')) return;
              event.preventDefault();
              const bounds = event.currentTarget.getBoundingClientRect();
              setMenu({doc,x:bounds.left,y:bounds.bottom,trigger:event.currentTarget});
            }} aria-label={current==='recent' ? `继续阅读 ${displayTitle(doc.title)}` : undefined}>
            {view==='shelf' && <BookCover id={doc.id} format={doc.source_url ? 'web' : doc.format} visible={visible} label={format(doc)}>
              <strong>{displayTitle(doc.title)}</strong>
            </BookCover>}
            <span className="library-document-title">{displayTitle(doc.title)}</span>
            {percent!==null ? <span className="library-reading-progress" title={progress(doc)}>
              <progress value={percent} max={100} aria-label="上次阅读位置"/><small>{percent}%</small>
            </span> : current==='recent' && <small className="library-document-meta">{progress(doc)}</small>}
          </button>
          {view==='list' && <small className="library-document-meta">{format(doc)}{recent[doc.id] ? ` · ${new Date(recent[doc.id]).toLocaleDateString()}` : ''}</small>}
          {selecting ? <input type="checkbox" aria-label={`选择 ${displayTitle(doc.title)}`} checked={selected.includes(doc.id)}
            onChange={event=>setSelected(event.target.checked ? [...selected,doc.id] : selected.filter(id=>id!==doc.id))}/>
            : null}
        </li>;})}
      </ul>
      {menu && <div ref={menuRef} className="reader-document-menu-panel library-context-menu"
        role="group" aria-label={`${displayTitle(menu.doc.title)} 的管理操作`}
        onKeyDown={event=>{
          if (event.key!=='Escape') return;
          event.preventDefault();
          menu.trigger.focus();
          setMenu(null);
        }}>
        <button type="button" onClick={()=>{
          setTitleEdit({id:menu.doc.id,title:displayTitle(menu.doc.title)});
          setMenu(null);
        }}>重命名</button>
        <button type="button" onClick={()=>{beginAssign([menu.doc.id]);setMenu(null);}}>
          加入分类
        </button>
        {folder && sort==='manual' && !normalizedQuery && <>
          <button type="button" disabled={shown[0]?.id===menu.doc.id}
            onClick={()=>moveDocument(menu.doc.id,-1)}>前移一位</button>
          <button type="button" disabled={shown.at(-1)?.id===menu.doc.id}
            onClick={()=>moveDocument(menu.doc.id,1)}>后移一位</button>
        </>}
        {folder && <button type="button" onClick={()=>{
          update(collections.map(item=>item.id===folder.id
            ? {...item,documents:item.documents.filter(id=>id!==menu.doc.id)} : item));
          setMenu(null);
        }}>从当前分类移除</button>}
        <button type="button" className="delete-document" disabled={busy}
          onClick={()=>{setDeleteTarget(menu.doc);setMenu(null);}}>删除文本</button>
      </div>}
      {shown.length>0 && <p className="muted library-management-hint">右键管理文档</p>}
      {!shown.length && <p className="muted">{normalizedQuery ? '没有匹配结果' : '这里还没有文本'}</p>}
    </section>
  </main>;
}

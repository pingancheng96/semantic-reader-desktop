import {React,ReactDOM,call} from './runtime.js';

type Row = {classification_trace?:{index:number;returned:unknown[];mapped:{id:number;code:string;path:string}[]};codes?:string[];id:string;lemma:string;meaning:string;context:string|null;current_code?:string|null;code?:string|null;applied?:boolean;invalid_code?:boolean;returned_code?:string;outside?:boolean;manual?:boolean;alternative?:string;reason?:string;needs_review?:boolean;review_status?:string;ranked_move?:boolean;ranking?:string[];ranked_paths?:{code:string;path:string;score:number}[]};
type Batch = {rows:Row[];next:string|null;usage?:{total_tokens?:number};usage_incomplete?:boolean};
type Domain = {code:string;name:string};

const progressPrefix = 'silDirectReviewV1:';
export function clearSilProgress() {
  for (let i=localStorage.length-1; i>=0; i--) {
    const key = localStorage.key(i);
    if (key?.startsWith(progressPrefix) || key==='silAiReviewV1') localStorage.removeItem(key);
  }
}

type Progress = {rows:Row[];cursor:string|null;tokens:number;unknown:boolean};
const emptyProgress = ():Progress=>({rows:[],cursor:'',tokens:0,unknown:false});
function loadProgress(progressKey:string):Progress {
  try { return JSON.parse(localStorage.getItem(progressKey) ?? 'null') ?? emptyProgress(); }
  catch { return emptyProgress(); }
}

function reviewDestination(row:Row,parent:string,labels:Map<string,string>):Row {
    if (row.codes || row.manual || row.applied) return row;
    if (row.ranked_paths?.length) {
        const first = row.ranked_paths[0];
        return {...row,codes:row.ranked_paths.map(path=>path.code).filter(code=>code!==parent),code:first.code===parent ? null : first.code,
            ranked_move:true,needs_review:false,review_status:undefined};
    }
    return row.code==='unclassified' ? {...row,code:null,codes:[]} : row;
}

function pendingChange(row:Row) {
  if (row.codes) return !row.applied && row.codes.length>0;
  return Boolean(row.code && row.code!==row.current_code && !row.applied);
}

function completedProgress(progress:Progress,parent:string,labels:Map<string,string>):Progress {
  const remaining = progress.rows.filter(row=>!row.applied);
  if (progress.cursor===null && remaining.length!==progress.rows.length &&
      !remaining.some(row=>row.invalid_code && !labels.size || pendingChange(reviewDestination(row,parent,labels)))) {
    return emptyProgress();
  }
  return progress;
}

export function DestinationSelect({domains,row,parent,disabled,change}:{
  domains:Domain[];row:Pick<Row,'code'|'needs_review'|'manual'>;parent:string;disabled:boolean;change:(code:string|null)=>void;
}) {
  const dialog = React.useRef<HTMLDialogElement>(null);
  const [query,setQuery] = React.useState('');
  const [branch,setBranch] = React.useState('');
  const [choice,setChoice] = React.useState<string|null>(null);
  const term = query.trim().toLowerCase();
  const labels = new Map(domains.map(domain=>[domain.code,domain.name]));
  function path(code:string) {
    return code.split('.').map((_,index,parts)=>labels.get(parts.slice(0,index+1).join('.'))).join(' › ');
  }
  const options = term ? domains.filter(domain=>
    domain.code.startsWith(term) || path(domain.code).toLowerCase().includes(term)).slice(0,40)
    : domains.filter(domain=>domain.code.split('.').slice(0,-1).join('.')===branch);
  const destination = row.code==='unclassified' ? '退回未归类' : row.code ? path(row.code) || row.code : row.needs_review && !row.manual ? '未选择分类' : '保留在当前类';
  return <div className="sil-destination-field">
    <button type="button" className="quiet sil-change" aria-haspopup="dialog" disabled={disabled}
      onClick={()=>{
        setQuery('');
        setBranch(row.code && labels.has(row.code) ? row.code : parent);
        setChoice(row.code ?? null);
        dialog.current?.showModal();
      }}>更改去向</button>
    {ReactDOM.createPortal(<dialog ref={dialog} className="library-dialog sil-choice-dialog" aria-label="选择 SIL 分类">
      <form method="dialog" onSubmit={event=>{
        event.stopPropagation();
        if ((event.nativeEvent as SubmitEvent).submitter?.getAttribute('value')==='choose') change(choice);
      }}>
        <h2>选择 SIL 分类</h2>
        <input type="search" autoFocus aria-label="搜索全部分类" placeholder="搜索英文类名或编号" value={query}
          onChange={event=>setQuery(event.target.value)}/>
        {!term && <nav className="sil-choice-path" aria-label="分类路径">
          <button type="button" onClick={()=>setBranch('')}>分类目录</button>
          {branch && branch.split('.').map((_,index,parts)=>{
            const code=parts.slice(0,index+1).join('.');
            return <button type="button" key={code} onClick={()=>{setBranch(code);setChoice(code);}}>{labels.get(code)}</button>;
          })}
        </nav>}
        <ul className="sil-choice-options">
          {options.map(domain=><li key={domain.code}><button type="button" aria-pressed={choice===domain.code}
            onClick={()=>{setChoice(domain.code);setBranch(domain.code);setQuery('');}}>
            <span>{term ? path(domain.code) : domain.name}</span><small>{domain.code}</small>
          </button></li>)}
          {!options.length && term && <li className="muted">没有匹配分类</li>}
          {!options.length && !term && <li className="muted">没有下级分类</li>}
        </ul>
        <div className="row sil-choice-actions">
          <button type="button" onClick={()=>setChoice(null)}>保留在当前类</button>
          {parent && <button type="button" onClick={()=>setChoice('unclassified')}>退回未归类</button>}
        </div>
        <p className="sil-choice-current">去向 · {choice==='unclassified' ? '未归类' : choice ? path(choice) : '保留在当前类'}</p>
        <div className="row sil-choice-actions">
          <button type="submit" value="cancel">取消</button>
          <button type="submit" className="primary" value="choose">确认去向</button>
        </div>
      </form>
    </dialog>,document.body)}
    <span className="sil-current-destination">{destination}</span>
  </div>;
}

export function SilOrganizer({busy,configured,config,parent,apply}:{
  busy:boolean;configured:boolean;config:object;parent:string;apply:(rows:Row[],backup:boolean,keepOpen:boolean,done:()=>void)=>void;
}) {
  const progressKey = progressPrefix + parent;
  const [progress,setProgress] = React.useState(()=>loadProgress(progressKey));
  const [total,setTotal] = React.useState<number>();
  function refreshCount() {
    void call<number>('sil_count',{parent}).then(setTotal).catch(error=>setMessage(String(error)));
  }

  const [domains,setDomains] = React.useState<Domain[]>([]);
  const [backup,setBackup] = React.useState(false);
  const [stage,setStage] = React.useState(false);
  function save(next:Progress) {
    setProgress(next);
    try { localStorage.setItem(progressKey,JSON.stringify(next)); }
    catch { setMessage('无法保存扫描进度。请在离开当前分类前应用本次结果。'); }
  }
  React.useEffect(()=>{
    refreshCount();
    let active = true;
    void call<Domain[]>('sil_catalog').then(items=>{if (active) setDomains(items);}).catch(()=>{});
    return ()=>{active=false;};
  },[]);
  const [message,setMessage] = React.useState('');
  const [query,setQuery] = React.useState('');
  const [limit,setLimit] = React.useState(40);
  const stop = React.useRef(false);
  React.useEffect(()=>()=>{stop.current=true;},[]);
  const labels = new Map(domains.map(domain=>[domain.code,domain.name]));
  const [keepAppliedPreview,setKeepAppliedPreview] = React.useState(false);
  const activeProgress = keepAppliedPreview ? progress : completedProgress(progress,parent,labels);
  const rows = activeProgress.rows.map(row=>reviewDestination(row,parent,labels));
  const applicable = rows.filter(pendingChange);
  const {cursor,tokens,unknown} = activeProgress;
  async function scan() {
    if (stage || cursor===null) return;
    stop.current=false;
    setStage(true);
    setMessage('');
    let next = activeProgress;
    try {
      do {
        const batch:Batch = await call('sil_review',{...config,cursor:next.cursor,parent});
        if (batch.next && batch.next<=(next.cursor ?? '')) throw Error('批次进度无效');
        next = {rows:[...next.rows,...batch.rows],cursor:batch.next,
          tokens:next.tokens+(batch.usage?.total_tokens ?? 0),
          unknown:next.unknown || !!batch.usage_incomplete || (!!batch.rows.length && !batch.usage)};
        save(next);
      } while (next.cursor!==null && !stop.current);
    } catch (error) { setMessage(String(error)); }
    finally { setStage(false); }
  }
  function chooseDestination(id:string,code:string|null) {
    save({...activeProgress,rows:rows.map(row=>row.id===id
      ? {...row,manual:true,needs_review:false,outside:false,invalid_code:false,code,codes:code && code!=='unclassified' ? [code] : []} : row)});
  }
  function applyRows(changes:Row[],keepOpen=false) {
    const ids = new Set(changes.map(row=>row.id));
    apply(changes,backup,keepOpen,()=>{
      const next = {...activeProgress,rows:rows.map(row=>ids.has(row.id) ? {...row,applied:true} : row)};
      if (keepOpen) {setKeepAppliedPreview(true);save(next);}
      else save(completedProgress(next,parent,labels));
      refreshCount();
    });
  }
  const returns = applicable.filter(row=>row.code==='unclassified');
  const reviewed = rows.filter(row=>row.code!==undefined).length;
  const applied = rows.filter(row=>row.applied).length;
  const term = query.trim().toLowerCase();
  const labelsForSearch = (row:Row)=>domains.find(item=>item.code===row.code)?.name ?? '';
  const visible = rows.filter(row=> [row.lemma,row.meaning,row.code ?? '',labelsForSearch(row)].some(text=>text.toLowerCase().includes(term)));
  return <section aria-label="整理当前 SIL 分类">

    <p>{total ?? '…'} 个直属义项。AI 推荐的分类默认选中，可逐项取消。应用后会加入所选分类。</p>
    <div className="row">
      <button type="button" disabled={busy || stage || !configured || cursor===null || !total}
        onClick={()=>void scan()}>{rows.length ? '继续整理当前类' : 'AI 整理当前类'}</button>
      {stage && <button type="button" onClick={()=>{stop.current=true;}}>本批结束后暂停</button>}
      {!stage && rows.length>0 && <button type="button" disabled={busy} onClick={()=>{
        if (window.confirm('清除这次分类预览并从头开始？')) save(emptyProgress());
      }}>重新开始</button>}
    </div>
    {!configured && <p className="muted">请先配置 AI。</p>}
    <p role="status">本轮已复核 {reviewed} 条{applied>0 && ` · 已应用 ${applied} 条`}{cursor===null ? ' · 扫描完成' : ''}
      {reviewed>0 && ` · 已报告 ${tokens.toLocaleString()} token${unknown ? '（部分请求未返回用量）' : ''}`}</p>
    {applicable.length>0 && <label><input type="checkbox" checked={backup} disabled={busy || stage}
      onChange={event=>setBackup(event.target.checked)}/> 应用前创建备份</label>}
    {applicable.length>0 && <button type="button" className="primary" disabled={busy || !!stage}
      onClick={()=>applyRows(applicable)}>
      应用 {applicable.length} 条变更
    </button>}
    {message && <p role="alert">{message}</p>}
    {!!rows.length && <>
      <ul>
        {domains.filter(domain=>applicable.some(row=>row.code===domain.code)).map(child=>({child,count:applicable.filter(row=>row.code===child.code).length}))
          .filter(item=>item.count>0).map(({child,count})=><li key={child.code}>{child.code} {child.name} · {count} 个</li>)}
        {rows.some(row=>row.invalid_code) && <li>无效分类编号 · {rows.filter(row=>row.invalid_code).length} 个</li>}
        {rows.some(row=>row.outside && !row.code) && <li>疑似错分 · {rows.filter(row=>row.outside && !row.code).length} 个</li>}
        {returns.length>0 && <li>待退回未归类 · {returns.length} 个</li>}
        {rows.some(row=>row.needs_review && !row.code) && <li>待核对 · {rows.filter(row=>row.needs_review && !row.code).length} 个</li>}
        <li>保留在当前类 · {rows.filter(row=>!row.code && !row.invalid_code && !row.outside && !row.needs_review).length} 个</li>
      </ul>
      <input type="search" aria-label="搜索分类预览" placeholder="搜索词、释义或分类" value={query}
        onChange={event=>{setQuery(event.target.value);setLimit(40);}}/>
      <ul>{visible.slice(0,limit).map(row=><li key={row.id}>
        <b>{row.lemma}</b> · {row.meaning}
        <p>当前：{row.current_code ? `${row.current_code} ${labels.get(row.current_code) ?? ''}` : '未归类'}
          {' → '}{row.applied ? '已应用：' : row.manual ? '手动选择：' : 'AI 建议：'}{row.code==='unclassified' ? '退回未归类' : row.code ? `${row.code} ${labels.get(row.code) ?? ''}` : row.invalid_code ? (row.returned_code ? `返回了无效分类编号：${row.returned_code}` : '返回了无效分类编号') : row.outside ? '疑似错分' : row.needs_review ? '待核对' : '保留在当前类'}</p>
        {row.needs_review && !row.manual && <p className="muted">{({
          no_candidate:'模型未返回有效候选',
          disagreement:'两轮首选不一致',
          low_score:'匹配分或领先幅度不足',
          low_minimum:'首选低于最低分',
          low_margin:'首选领先幅度不足',
          low_both:'首选分数和领先幅度均不足',
          uncertain:'复审未能确认首选',
          review_failed:'复审结果不可用'
        } as Record<string,string>)[row.review_status ?? ''] ?? '请手动选择去向'}</p>}
        {!!row.ranked_paths?.length && <ul aria-label="AI 推荐分类">
          {row.ranked_paths.map(path=><li key={path.code}><label>
            <input type="checkbox" disabled={busy || stage || row.applied || path.code===parent}
              checked={path.code===parent || !!row.codes?.includes(path.code)}
              onChange={event=>{
                const codes = event.target.checked ? [...(row.codes ?? []),path.code] : (row.codes ?? []).filter(code=>code!==path.code);
                save({...activeProgress,rows:rows.map(item=>item.id===row.id ? {...item,codes,code:codes[0] ?? null} : item)});
              }}/>{path.code} {path.path}
          </label></li>)}
        </ul>}
        {!!row.ranking?.length && <ol aria-label="SIL 候选排序">{row.ranking.map(path=><li key={path}>{path}</li>)}</ol>}
        {row.alternative ? <p>跨分支建议：{row.alternative}{row.reason && ` · ${row.reason}`}</p> : row.reason && !row.manual && <p>归类依据：{row.reason}</p>}
        {!row.applied && <div className="sil-destination"><DestinationSelect domains={domains} row={row} parent={parent} disabled={busy || stage}
          change={code=>chooseDestination(row.id,code)}/>
        {applicable.includes(row) &&
          <button type="button" className="sil-apply-one" disabled={busy || stage}
            onClick={()=>applyRows([row],true)}>应用此条</button>}
        </div>}
        {row.classification_trace && <details>
          <summary>查看本次分类依据</summary>
          <p>批次内义项序号 {row.classification_trace.index} · {row.lemma} · {row.meaning}</p>
          <p>模型返回 {JSON.stringify(row.classification_trace.returned)}</p>
          <ul>{row.classification_trace.mapped.map((item,index)=><li key={index}>
            {item.id} → {item.code} {item.path}
          </li>)}</ul>
        </details>}
        {row.context && <details><summary>查看原文</summary><p>{row.context}</p></details>}
      </li>)}</ul>
      {visible.length>limit && <button type="button" onClick={()=>setLimit(limit+40)}>显示更多</button>}
    </>}
  </section>;
}

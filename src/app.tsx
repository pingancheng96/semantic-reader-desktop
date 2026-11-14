import {DestinationSelect,SilOrganizer,clearSilProgress} from './sil-organizer.js';
import {UpdateSettings} from './update-settings.js';
import {TextLibrary} from './text-library.js';
import {ReaderToolbar} from './reader-toolbar.js';
import {ReaderNavigation, readReaderPreference, saveReaderPreference, useReaderNavigation, type ReaderNavigationState} from './reader-navigation.js';
import {EpubReader} from './epub-reader.js';
import {PronunciationPanel, PronunciationSettings} from './pronunciation.js';
import {useArticleSearchShortcut} from './article-search.js';
import {PdfReader} from './pdf-reader.js';
import {waitForVoice} from './speech-voices.js';
import type {ClipboardEvent, FormEvent, KeyboardEvent as ReactKeyboardEvent, ReactNode} from 'react';
import {React, ReactDOM, call, exportFile, importLocalFile, restoreBackupFile} from './runtime.js';
import {articleMatches, changedRanges, keepUnchanged, readingLines, selectedOffsets, type Piece} from './reading.js';
import {captionNodes, type WebNode} from './web-captions.js';
import {dictionaryLookupWord} from './dictionary-word.js';
const {useState, useEffect, useLayoutEffect, useMemo, useRef} = React;

type Item = {id: string; title: string; format: 'md' | 'txt'; source_url?: string | null};
type Group = {sil_code?:string|null; id: string; name: string; description: string};
type Vocabulary = {expression: string; nuance: string};
type Sense = {id: string; lemma: string; meaning: string; group_id: string};
type Mark = {id: string; document_id: string; title: string; sense_id: string; start: number; end: number;
  surface: string; context: string; context_meaning?:string|null; lemma: string; meaning: string; group_id: string; group_name: string; learning?: string | null};
type Snapshot = {id:string; timestamp:number; safety:boolean};
const snapshotLabel = (snapshot:Snapshot) =>
  new Date(snapshot.timestamp).toLocaleString() + (snapshot.safety ? ' · 恢复前自动备份' : '');
type Library = {sil_counts?:{code:string;count:number;ids?:string}[]; documents: Item[]; groups: Group[]; undo: number};
type WebLayout = {nodes:WebNode[]; images:(string|null)[]; image_errors?:(string|null)[]};
type Read = {document: Item & {content: string; web_layout?: string | null; pdf?:number; epub?:number}; marks: Mark[]};
type Detail = {group: Group; senses: Sense[]; marks: Mark[]; vocabulary: Vocabulary[]};
type Selection = {document_id: string; start: number; end: number; surface: string};
type SavedNote = Sense & {group_name:string; learning?:string|null};
type Candidates = {context: string; exact: Sense[]; reused: Sense[]; groups: Group[]; notes:SavedNote[]};
type GroupFields = {name: string; description: string; vocabulary: string; sil_code?:string|null};
type Learning = {usage_note: string; example: string; etymology?:string|null; mnemonic?:string|null;
  ipa_uk?:string|null; ipa_us?:string|null; part_of_speech?:string|null;
  related: {expression: string; kind: 'synonym' | 'related' | 'collocation'; note: string}[]};
type Suggestion = {suggestion: {lemma: string; meaning: string; context_meaning:string; group_id: string | null; sense_id?: string | null; group_reason?: string | null;
  sil_code?:string|null; group_name?: string; group_description?: string; group_vocabulary?: {expression:string; note:string}[]; confidence: number; learning: Learning}; usage?: {total_tokens?: number|null}; group_check_failed?:boolean};
const blank = (): GroupFields => ({name: '', description: '', vocabulary: ''});
const empty: Library = {documents: [], groups: [], undo: 0};

let pendingPronunciation: AbortController | undefined;

function Pronunciation({ipa, word, locale}: {ipa?:string|null; word:string; locale?:'en-GB'|'en-US'}) {
  const [error,setError] = useState('');
  const [loading,setLoading] = useState(false);
  const request = useRef<AbortController | undefined>(undefined);
  useEffect(() => () => request.current?.abort(), [word,locale,ipa]);
  if (!ipa) return <>—</>;
  if (!locale || !word.trim()) return <>{ipa}</>;
  const label = locale === 'en-GB' ? '播放英式读音' : '播放美式读音';
  async function play() {
    const speech = window.speechSynthesis;
    if (!speech || typeof SpeechSynthesisUtterance === 'undefined' || !locale) {
      setError('当前系统无法播放本地语音。');
      return;
    }
    pendingPronunciation?.abort();
    const controller = new AbortController();
    request.current = pendingPronunciation = controller;
    setError('');
    setLoading(true);
    const voice = await waitForVoice(speech, locale, controller.signal);
    if (request.current !== controller) return;
    setLoading(false);
    if (controller.signal.aborted) return;
    if (!voice) {
      setError('未找到对应的本地语音。请检查系统语音设置后重试。');
      return;
    }
    speech.cancel();
    const utterance = new SpeechSynthesisUtterance(word.trim());
    utterance.lang = voice.lang;
    utterance.voice = voice;
    utterance.onerror = event => {
      if (!controller.signal.aborted && event.error !== 'canceled' && event.error !== 'interrupted') setError('读音播放失败，请重试。');
    };
    speech.speak(utterance);
  }
  return <>
    <span>{ipa}</span>{' '}
    <button className="pronunciation-play" type="button" aria-label={label + ' · ' + word} title={label} disabled={loading} onClick={()=>void play()}>
      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
        <path d="M11 5 6 9H3v6h3l5 4V5Zm4 4a4 4 0 0 1 0 6m3-9a8 8 0 0 1 0 12"/>
      </svg>
    </button>
    {loading && <small role="status">正在载入语音…</small>}
    {error && <small className="error" role="status">{error}</small>}
  </>;
}

function WordHeading({surface,pos}: {surface:string;pos?:string|null}) {
  return <header className="word-heading">
    <p className="muted">原文词形{pos?.trim() && <> · {pos.trim()}</>}</p>
    <h2 lang="en">{surface}</h2>
  </header>;
}

function LearningPanel({learning, word, hideIpa, hidePos}: {learning: Learning; word:string; hideIpa?:boolean; hidePos?:boolean}) {
  const kinds = {synonym:'近义表达', related:'相关概念', collocation:'常见搭配'};
  return <section className="learning">
    {!hidePos && learning.part_of_speech && <p className="muted">语境词性（AI） · {learning.part_of_speech}</p>}
    {!hideIpa && (learning.ipa_uk || learning.ipa_us) && <details className="legacy-ipa">
      <summary>笔记保存的 AI 音标</summary>
      <dl className="word-details"><dt>英式</dt><dd><Pronunciation ipa={learning.ipa_uk} word={word} locale="en-GB"/></dd>
        <dt>美式</dt><dd><Pronunciation ipa={learning.ipa_us} word={word} locale="en-US"/></dd></dl>
    </details>}
    {(learning.etymology || learning.mnemonic) && <section className="word-origins">
      <h3>词源与速记 <small className="muted">AI</small></h3>
      {learning.etymology && <p><b>词源</b> · {learning.etymology}</p>}
      {learning.mnemonic && <p><b>速记</b> · {learning.mnemonic}</p>}
    </section>}
    {(learning.usage_note || learning.example || learning.related.length>0) && <>
    <h3>用法与相关表达</h3>
    <p>{learning.usage_note}</p>
    <blockquote lang="en">{learning.example}</blockquote>
    <ul>{learning.related.map((item,i)=><li key={i}>
      <b lang="en">{item.expression}</b> <small>{kinds[item.kind]}</small><br/>{item.note}
    </li>)}</ul>
    </>}
  </section>;
}

function GroupSenses({groupId, exclude, lemma, meaning, ai}: {
  groupId:string; exclude?:string; lemma:string; meaning:string; ai?:{endpoint:string;model:string}
}) {
  const comparisonKey = JSON.stringify([groupId,exclude,lemma,meaning,ai]);
  const [comparison,setComparison] = useState<{key:string; rows?:{id:string;note:string}[]; error?:string}>();
  const currentComparison = comparison?.key === comparisonKey ? comparison : undefined;
  const request = useRef(0);
  async function compare() {
    const ticket = ++request.current;
    setComparison({key:comparisonKey});
    try {
      const rows = await call<{id:string;note:string}[]>('compare_group_senses',{...ai,id:groupId,exclude,lemma,meaning});
      if (request.current===ticket) setComparison({key:comparisonKey,rows});
    } catch (error) {
      if (request.current===ticket) setComparison({key:comparisonKey,error:String(error)});
    }
  }
  const [result,setResult] = useState<Sense[] | null>();
  useEffect(()=>{
    let current = true;
    void call<Sense[]>('group_senses',{id:groupId}).then(
      senses=>{if(current)setResult(senses);},
      ()=>{if(current)setResult(null);}
    );
    return ()=>{current=false;};
  },[groupId]);
  const senses = result?.filter(sense=>sense.id!==exclude);
  return <section className="related-senses">
    <h3>同组已收录词项</h3>
    {ai && lemma.trim() && meaning.trim() && Boolean(senses?.length) && <>
      <button type="button" disabled={Boolean(currentComparison && !currentComparison.rows && !currentComparison.error)}
        onClick={()=>void compare()}>比较用法</button>
      <p className="muted">以「{lemma}」的当前释义为参照，AI 比较{(senses?.length ?? 0)>12 ? '前 12 个' : '这些'}同组词项。</p>
    </>}
    {currentComparison && <p className="muted" role="status">{currentComparison.error ||
      (currentComparison.rows ? (currentComparison.rows.length ? '以下为 AI 用法比较。' : 'AI 未返回用法比较。') : '正在比较用法…')}</p>}
    {senses?.length ? <ul>{senses.map(sense=>{
      const difference = currentComparison?.rows?.find(row=>row.id===sense.id);
      return <li key={sense.id}>
        <b lang="en">{sense.lemma}</b> · {sense.meaning}
        {difference && <div className="muted">{difference.note}</div>}
      </li>;
    })}</ul> : <p className="muted" role="status">{senses ? '该组暂无其他已收录词项。' :
      result === null ? '暂时无法读取同组词项。' : '正在读取同组词项…'}</p>}
  </section>;
}



function readLearning(raw?: string | null): Learning | null {
  return raw ? JSON.parse(raw) as Learning : null;
}

type SilDomain = {code:string;name:string};
let silCatalogRequest:Promise<SilDomain[]>|undefined;
function useSilCatalog() {
  const [domains,setDomains] = useState<SilDomain[]>([]);
  useEffect(()=>{
    let active = true;
    silCatalogRequest ??= call<SilDomain[]>('sil_catalog').catch(error=>{
      silCatalogRequest = undefined;
      throw error;
    });
    void silCatalogRequest.then(data=>{if (active) setDomains(data);}).catch(()=>{});
    return ()=>{active=false;};
  },[]);
  return domains;
}
function silPath(domains:SilDomain[],code:string) {
  const parts = code.split('.');
  return parts.map((_,index)=>{
    const prefix = parts.slice(0,index+1).join('.');
    return domains.find(item=>item.code===prefix)?.name ?? prefix;
  }).join(' › ');
}
function SilNavigation({domains,value,change,disabled,counts,directOnly,setDirectOnly}:{
  domains:SilDomain[];value:string;change:(code:string)=>void;disabled:boolean;counts:NonNullable<Library['sil_counts']>;directOnly:boolean;setDirectOnly:(value:boolean)=>void;
}) {
  const parent = value.slice(0,Math.max(0,value.lastIndexOf('.')));
  const children = domains.filter(domain=>
    domain.code.slice(0,Math.max(0,domain.code.lastIndexOf('.')))===value);
  const title = value==='unclassified' ? '未归类' : domains.find(domain=>domain.code===value)?.name ?? 'SIL 分类';
  const [showEmpty,setShowEmpty] = useState(false);
  const branches = children.map(domain=>({...domain,count:new Set(counts.filter(item=>
    item.code===domain.code || item.code.startsWith(domain.code+'.')).flatMap(item=>item.ids?.split(',') ?? [])).size}));
  const shown = showEmpty ? branches : branches.filter(domain=>domain.count>0);
  return <nav className="sil-navigation" aria-label="SIL 分类目录">
    {value && <button className="sil-back" type="button" disabled={disabled} onClick={()=>change(parent)}>
      <span aria-hidden="true">‹</span> {parent ? '返回上一级' : '返回分类目录'}
    </button>}
    <h2 aria-current={value ? 'page' : undefined}>{title}</h2>
    {children.length>0 && value && <div className="sil-branches">
      <button type="button" disabled={disabled} aria-pressed={!directOnly} onClick={()=>setDirectOnly(false)}>整个分支</button>
      <button type="button" disabled={disabled} aria-pressed={directOnly} onClick={()=>setDirectOnly(true)}>
        <span>尚未细分</span><small>{counts.find(item=>item.code===value)?.count ?? 0}</small>
      </button>
    </div>}
    {children.length>0 || !value ? <ul className="sil-branches">
      {!value && <li><button type="button" disabled={disabled} onClick={()=>change('unclassified')}>
        <span>未归类</span><small>{counts.find(item=>item.code==='unclassified')?.count ?? 0} ›</small>
      </button></li>}
      {shown.map(domain=><li key={domain.code}><button type="button" disabled={disabled}
        onClick={()=>change(domain.code)}><span>{domain.name}</span><small>{domain.count} ›</small></button></li>)}
    </ul> : null}
    {branches.some(domain=>domain.count===0) && <button type="button" className="sil-back"
      aria-pressed={showEmpty} onClick={()=>setShowEmpty(!showEmpty)}>{showEmpty ? '隐藏空分类' : '显示空分类'}</button>}
  </nav>;
}

function SilPicker({code,change}:{code?:string|null; change:(code:string|null)=>void}) {
  const domains = useSilCatalog();
  return <fieldset>
    <legend>SIL 语义分类</legend>
    <DestinationSelect domains={domains} row={{code:code ?? 'unclassified'}} parent="" disabled={!domains.length}
      change={value=>change(value==='unclassified' ? null : value)}/>
    <small>SIL Global · Semantic Domains v4 · CC BY-SA 4.0</small>
  </fieldset>;
}

function SenseSilEditor({current,busy,save}:{
  current:string|null;busy:boolean;save:(code:string|null)=>void;
}) {
  const [open,setOpen] = useState(false);
  const [code,setCode] = useState<string|null>(current);
  return open ? <div>
    <SilPicker code={code} change={setCode}/>
    <button type="button" disabled={busy || code===current} onClick={()=>save(code)}>移动此义项</button>
    <button type="button" disabled={busy} onClick={()=>setOpen(false)}>取消</button>
  </div> : <button type="button" disabled={busy} onClick={()=>{setCode(current);setOpen(true);}}>更改 SIL 分类</button>;
}

function SilSenseList({category,query,library,busy,onCount,openCategory,openDocument,move,remove,directOnly}:{
  category:string;query:string;library:Library;busy:boolean;onCount:(count:number)=>void;openCategory:(code:string)=>void;openDocument:(id:string,mark:string)=>void;move:(sense:Sense,code:string)=>void;remove:(sense:Sense,code:string)=>void;directOnly:boolean;
}) {
  const domains = useSilCatalog();
  const [data,setData] = useState<{rows:(Sense & {context:string|null;categories:string[];links?:Mark[];sil_diagnostic?:unknown})[]; error:string}|null>(null);
  useEffect(()=>{
    let active = true;
    void call<(Sense & {context:string|null;categories:string[];links?:Mark[];sil_diagnostic?:unknown})[]>('sil_senses',{code:category,descendants:!directOnly,q:query}).then(
      rows=>{if (active) {setData({rows,error:''});onCount(rows.length);}},
      error=>{if (active) setData({rows:[],error:String(error)});}
    );
    return ()=>{active=false;};
  },[category,library,directOnly,query]);
  if (!data) return <p role="status">正在读取义项…</p>;
  if (data.error) return <p role="alert">{data.error}</p>;
  const sections = new Map<string,typeof data.rows>();
  for (const row of data.rows) {
    const code = row.categories.find(code=>code===category) ??
      row.categories.find(code=>code.startsWith(category+'.')) ?? category;
    const section = sections.get(code) ?? [];
    section.push(row);
    sections.set(code,section);
  }
  return <section aria-label="分类义项">
    <p className="muted">{data.rows.length} 个{directOnly ? '直属' : ''}义项</p>
    {[...sections].map(([code,rows])=><section key={code} className="sil-senses">
      {code!==category && <h2><button type="button" className="quiet sil-section-link"
        title={silPath(domains,code)} onClick={()=>openCategory(code)}>
        {silPath(domains,code).split(' › ').at(-1)} <span aria-hidden="true">↗</span>
      </button></h2>}
      <ul>{rows.map(row=><li key={row.id}>
        <details><summary><b lang="en">{row.lemma}</b> <span>{row.meaning}</span></summary>
          {row.sil_diagnostic != null && <details>
            <summary>查看上次 AI 分类排错记录</summary>
            <pre style={{whiteSpace:'pre-wrap',overflowWrap:'anywhere'}}>{JSON.stringify(row.sil_diagnostic,null,2)}</pre>
          </details>}
          {row.context && <blockquote>{row.context}</blockquote>}
          <div className="row">
            <div className="row" aria-label="所属分类">
              {row.categories.length ? row.categories.map(code=>{
                const name = domains.find(domain=>domain.code===code)?.name ?? code;
                return <span className="row" key={code}>
                  <button type="button" title={silPath(domains,code)} onClick={()=>openCategory(code)}>{name} ↗</button>
                  <button type="button" className="quiet" disabled={busy} aria-label={'移除 '+name}
                    title={'移除 '+name} onClick={()=>remove(row,code)}>×</button>
                </span>;
              }) : <button type="button" onClick={()=>openCategory('unclassified')}>未归类 ↗</button>}
            </div>
            <SilPicker code={null} change={code=>{if(code)move(row,code);}}/>
          </div>
          {row.links?.map(link=><div className="backlink" key={link.id}>
            <button className="quiet" disabled={busy} onClick={()=>openDocument(link.document_id,link.id)}>{link.title} ↗</button>
            <p>{link.context}</p>
            {link.context_meaning && <p>本句释义 · {link.context_meaning}</p>}
          </div>)}
        </details>
      </li>)}</ul>
    </section>)}
  </section>;
}

type SilAuditChange = Pick<Sense,'id'|'lemma'|'meaning'> & {
  categories:string[]; add:string[]; remove:string[]; reason:string;
};

function SilAudit({code,descendants,count,domains,config,busy,configured,task,refresh}:{
  code:string;descendants:boolean;count:number;domains:SilDomain[];config:object;busy:boolean;configured:boolean;
  task:(action:()=>Promise<void>)=>Promise<void>;refresh:()=>Promise<void>;
}) {
  const [result,setResult] = useState<{count:number;changes:SilAuditChange[];tokens:number;usage_incomplete:boolean;invalid_codes:number}|null>(null);
  const [selected,setSelected] = useState<string[]>([]);
  const selectionKey = (id:string,action:string,category:string)=>[id,action,category].join('/');
  const picked = (row:SilAuditChange,action:'add'|'remove') =>
    row[action].filter(category=>selected.includes(selectionKey(row.id,action,category)));
  const changes = result?.changes.map(row=>({...row,add:picked(row,'add'),remove:picked(row,'remove')}))
    .filter(row=>row.add.length || row.remove.length) ?? [];
  return <section aria-label="AI 审查分类">
    <p>将审查{descendants ? '整个分支的' : '直属的'} {count} 个义项。</p>
    <button type="button" disabled={busy || !configured || !count} onClick={()=>void task(async()=>{
      const preview = await call<{count:number;changes:SilAuditChange[];tokens:number;usage_incomplete:boolean;invalid_codes:number}>('sil_audit',{...config,code,descendants});
      setResult(preview);
      setSelected(preview.changes.flatMap(row=>row.add.map(category=>selectionKey(row.id,'add',category))));
    })}>{result ? '重新审查' : '开始审查'}</button>
    {result && <>
      <p role="status">已审查 {result.count} 个义项，{result.changes.length ? result.changes.length+' 个义项有调整建议' : result.invalid_codes ? '没有有效的调整建议' : '无需调整'}。{result.usage_incomplete && !result.tokens ? '服务未报告 token 用量。' :
        `已报告 ${result.tokens.toLocaleString()} token${result.usage_incomplete ? '（部分批次未报告）' : ''}。`}{result.invalid_codes>0 && `已跳过 ${result.invalid_codes} 个无效分类编号。`}</p>
      {result.changes.map(row=>{
        const pending = changes.find(change=>change.id===row.id);
        const becomesUnclassified = pending && !pending.add.length &&
          row.categories.every(category=>pending.remove.includes(category));
        return <fieldset key={row.id} disabled={busy}>
          <legend><b>{row.lemma}</b> · {row.meaning}</legend>
          <p className="muted">当前分类 · {row.categories.map(category=>silPath(domains,category)).join('；')}</p>
          {row.reason && <p>{row.reason}</p>}
          {(['add','remove'] as const).flatMap(action=>row[action].map(category=>{
            const key = selectionKey(row.id,action,category);
            return <label className="check" key={key}>
              <input type="checkbox" checked={selected.includes(key)} onChange={()=>setSelected(current=>toggle(current,key))}/>
              {action==='add' ? '新增' : '移除'} {silPath(domains,category)}
            </label>;
          }))}
          {becomesUnclassified && <p>应用后将回到未归类。</p>}
        </fieldset>;
      })}
      {result.changes.length>0 && <button type="button" className="primary" disabled={busy || !changes.length}
        onClick={()=>void task(async()=>{
          await call('sil_audit_apply',{code,descendants,rows:changes});
          setResult(null);setSelected([]);await refresh();
        })}>应用 {changes.length} 条变更</button>}
    </>}
  </section>;
}

function GroupEditor({value, change}: {value: GroupFields; change: (fields: GroupFields) => void}) {
  return <>
    <label>组名<input value={value.name} maxLength={120} required onChange={e => change({...value,name:e.target.value})}/></label>
    <label>组说明<textarea value={value.description} maxLength={2000} onChange={e => change({...value,description:e.target.value})}/></label>
    <label>核心词与搭配<textarea placeholder={'每行一个，可写成 plummet | 强调急剧下降'} value={value.vocabulary}
      maxLength={6000} onChange={e => change({...value,vocabulary:e.target.value})}/></label>
    <SilPicker code={value.sil_code} change={sil_code=>change({...value,sil_code})}/>
  </>;
}

function GroupPicker({groups, value, change, placeholder = '选择目标组'}:
  {groups: Group[]; value: string; change: (v: string) => void; placeholder?: string}) {
  return <select aria-label={placeholder} value={value} onChange={e => change(e.target.value)}>
    <option value="">{placeholder}</option>
    {groups.map(g => <option key={g.id} value={g.id}>{g.name}</option>)}
  </select>;
}

const webTags = new Set(['div','section','article','header','p','h1','h2','h3','h4','h5','h6',
  'ul','ol','li','blockquote','figure','figcaption','table','thead','tbody','tfoot','tr','th','td',
  'pre','dl','dt','dd','strong','b','em','i','code','s','sub','sup','mark','br','hr']);

type InlineEdit = {start:number; end:number; text:string; unchanged:ReturnType<typeof keepUnchanged>};
type EditedDocument = {content:string; positions:{id:string; start:number|null}[]; webNodes?:WebNode[]};

function DeleteMark({busy, remove}: {busy:boolean;remove:()=>void}) {
  const [confirm,setConfirm] = useState(false);
  return confirm ? <div className="delete-confirm">
    <p>删除这处原文标注？如果这是该义项的最后一处标注，也会删除词库中的义项，并清除语义组关系调整的撤销记录。</p>
    <div className="row">
      <button type="button" disabled={busy} onClick={()=>setConfirm(false)}>取消</button>
      <button type="button" className="delete-document" disabled={busy} onClick={remove}>确认删除此处</button>
    </div>
  </div> : <button type="button" className="quiet delete-document" disabled={busy}
    onClick={()=>setConfirm(true)}>删除此处标注</button>;
}

function DeleteSense({sense, busy, remove}: {
  sense: Pick<Sense,'lemma'|'meaning'>; busy: boolean; remove: () => void;
}) {
  const [confirm,setConfirm] = useState(false);
  return confirm ? <div className="delete-confirm">
    <p>删除「{sense.lemma} · {sense.meaning}」及它在所有文章中的标注和原文链接？此操作无法撤销，并会清除语义组关系调整的撤销记录。</p>
    <div className="row">
      <button type="button" disabled={busy} onClick={()=>setConfirm(false)}>取消</button>
      <button type="button" className="delete-document" disabled={busy} onClick={remove}>确认删除义项</button>
    </div>
  </div> : <button type="button" className="quiet delete-document" disabled={busy}
    onClick={()=>setConfirm(true)}>删除义项</button>;
}

type MergeTarget = {id:string;meaning:string;group_name:string;links:number};
function MergeSense({sense,links,busy,load,merge}: {
  sense:Sense;links:number;busy:boolean;
  load:(receive:(options:MergeTarget[])=>void)=>void;
  merge:(target:string,meaning:string)=>void;
}) {
  const [options,setOptions] = useState<MergeTarget[]|null>(null);
  const [target,setTarget] = useState('');
  const [meaning,setMeaning] = useState('');
  const chosen = options?.find(item=>item.id===target);
  if (!options) return <button type="button" className="quiet" disabled={busy}
    onClick={()=>load(setOptions)}>合并义项…</button>;
  return <form className="action" onSubmit={event=>{
    event.preventDefault();
    if (chosen && meaning.trim()) merge(chosen.id,meaning);
  }}>
    {options.length ? <>
      <label>保留哪个义项<select value={target} disabled={busy} onChange={event=>{
        const id=event.target.value;
        setTarget(id);setMeaning(options.find(item=>item.id===id)?.meaning ?? '');
      }}>
        <option value="">选择同词义项</option>
        {options.map(item=><option key={item.id} value={item.id}>{item.group_name} · {item.meaning}</option>)}
      </select></label>
      {chosen && <>
        <label>合并后的通用义项<textarea required maxLength={1200} disabled={busy}
          value={meaning} onChange={event=>setMeaning(event.target.value)}/></label>
        <p>将「{sense.meaning}」并入「{chosen.group_name}」中的所选义项。合并后保留两条义项的 {links+chosen.links} 处原文链接和学习笔记。已有本句释义会保留，缺少本句释义的链接会沿用原义项释义。</p>
        <p className="muted">确认后会清除语义组关系调整的撤销记录。</p>
      </>}
    </> : <p>没有找到同一词形的其他义项。</p>}
    <div className="row">
      <button type="button" disabled={busy} onClick={()=>{setOptions(null);setTarget('');}}>取消</button>
      {chosen && <button type="submit" disabled={busy || !meaning.trim()}>确认合并义项</button>}
    </div>
  </form>;
}

type GlossChange = {
  key:string;id:string;kind:'sense'|'mark';lemma:string;previous:string|null;
  general:string;sense_id:string;context:string|null;meaning:string;
};

function toggle(list: string[], id: string) {
  return list.includes(id) ? list.filter(x => x !== id) : [...list,id];
}

function LibraryGlossOrganizer({busy,configured,scan,apply}: {
  busy:boolean;configured:boolean;
  scan:(cursor:string|null,receive:(changes:GlossChange[],next:string|null)=>void,stopped:()=>boolean)=>void;
  apply:(changes:GlossChange[],done:()=>void)=>void;
}) {
  const [changes,setChanges] = useState<GlossChange[]>([]);
  const [excluded,setExcluded] = useState<string[]>([]);
  const [cursor,setCursor] = useState<string|null>(null);
  const [batches,setBatches] = useState(0);
  const [complete,setComplete] = useState(false);
  const [query,setQuery] = useState('');
  const [page,setPage] = useState(0);
  const running = useRef(false);
  const stop = useRef(false);
  useEffect(()=>()=>{stop.current=true;},[]);
  const selected = changes.filter(item=>!excluded.includes(item.key));
  const matches = changes.filter(item=>item.lemma.toLowerCase().includes(query.trim().toLowerCase()));
  const pageCount = Math.ceil(matches.length/30);
  return <section aria-label="一次性整理全库释义">
    <h2>一次性整理全库释义</h2>
    <p>扫描全库义项和未单独填写本句释义的原文链接。早期版本从义项复制到标注的释义也会纳入。</p>
    <p>开始后会先自动备份，再分批把词形、已有释义和相关原文片段发送到你配置的 AI 服务。词库只在确认后更新。</p>
    <div className="row">
      <button type="button" disabled={busy || !configured} onClick={()=>{
        if (!cursor) {
          setChanges([]);setExcluded([]);setBatches(0);setComplete(false);setPage(0);
        }
        stop.current=false;running.current=true;
        scan(cursor,(items,next)=>{
          setChanges(previous=>[...previous,...items]);setCursor(next);
          setBatches(count=>count+1);setComplete(next===null);
          if (next===null) running.current=false;
        },()=>stop.current);
      }}>{cursor ? '继续生成预览' : '开始备份并生成预览'}</button>
      {busy && running.current && <button type="button" onClick={()=>{stop.current=true;}}>本批完成后停止</button>}
    </div>
    {!configured && <p className="muted">请先在设置中配置 AI。</p>}
    {batches>0 && <p role="status">已检查 {batches} 批 · {changes.length} 条建议
      {complete ? ' · 扫描完成' : ' · 可继续扫描'}</p>}
    {changes.length>0 && <>
      <label>按词形筛选预览<input value={query} onChange={event=>{setQuery(event.target.value);setPage(0);}}/></label>
      {matches.slice(page*30,page*30+30).map(item=><section className="action" key={item.key}>
        <label className="check"><input type="checkbox" checked={!excluded.includes(item.key)}
          disabled={busy} onChange={()=>setExcluded(items=>toggle(items,item.key))}/>
          <b>{item.lemma}</b> · {item.kind==='sense' ? '通用义项' : '本句释义'}</label>
        {item.kind==='mark' && item.context && <blockquote>{item.context}</blockquote>}
        <p>原释义 · {item.previous ?? item.general}</p>
        <p>建议 · {item.meaning}</p>
      </section>)}
      {pageCount>1 && <div className="row">
        <button type="button" disabled={page===0} onClick={()=>setPage(page-1)}>上一页</button>
        <span>{page+1} / {pageCount}</span>
        <button type="button" disabled={page+1>=pageCount} onClick={()=>setPage(page+1)}>下一页</button>
      </div>}
      <p className="muted">写入会保留原文链接和学习笔记，并清除语义组关系调整的撤销记录。与其他义项释义冲突的项目会跳过。</p>
      <button className="primary" type="button" disabled={busy || !complete || !selected.length}
        onClick={()=>apply(selected,()=>{
          setChanges([]);setExcluded([]);setBatches(0);setComplete(false);setPage(0);
        })}>确认写入所选（{selected.length}）</button>
    </>}
  </section>;
}

function captureReadingAnchor(area:HTMLElement) {
  const top = area.getBoundingClientRect().top;
  const anchors = area.querySelectorAll<HTMLElement>('[data-offset]');
  // Find the first text fragment that crosses the top of the reading area.
  for (const anchor of Array.from(anchors)) {
    if (anchor.getBoundingClientRect().bottom<=top) continue;
    const walker = document.createTreeWalker(anchor,NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      const text = node as Text;
      if (!text.length) continue;
      const range = document.createRange();
      range.selectNodeContents(text);
      if (range.getBoundingClientRect().bottom<=top) continue;
      let first = 0;
      let last = text.length-1;
      while (first<last) {
        const middle = Math.floor((first+last)/2);
        range.setStart(text,0);
        range.setEnd(text,middle+1);
        if (range.getBoundingClientRect().bottom>top) last = middle;
        else first = middle+1;
      }
      range.setStart(text,first);
      range.setEnd(text,first+1);
      return {range,offset:range.getBoundingClientRect().top-top};
    }
  }
  return null;
}

function Reader({data, visible, focused, onMark, onSelect, editing, busy, beginEdit, cancel, save, toolbar, scrollPosition, navigation, readingAnchor, notesCollapsed}: {
  toolbar:HTMLElement|null; scrollPosition:{current:number}; navigation:ReaderNavigationState;
  readingAnchor:{current:ReturnType<typeof captureReadingAnchor>}; notesCollapsed:boolean;
  data: Read; visible: boolean; focused: string; onMark: (m: Mark) => void;
  onSelect: (s: Selection, doubleClick: boolean) => void;
  editing:boolean; busy:boolean; beginEdit:()=>void; cancel:()=>void; save:(edit:EditedDocument)=>void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const scrollArea = useRef<HTMLDivElement>(null);
  const [zoom,setZoom] = useState(1);
  function turn(step:number) {
    const area = scrollArea.current;
    if (area) area.scrollTop += step * area.clientHeight;
  }
  function changeZoom(value:number) {
    const nextZoom = Math.max(.5,Math.min(3,value));
    if (nextZoom===zoom) return;
    if (scrollArea.current) readingAnchor.current = captureReadingAnchor(scrollArea.current);
    setZoom(nextZoom);
  }
  useLayoutEffect(()=>{
    if (visible && scrollArea.current) scrollArea.current.scrollTop = scrollPosition.current;
  },[visible,scrollPosition]);
  const {content, format, id, web_layout} = data.document;
  useLayoutEffect(()=>{
    const area = scrollArea.current;
    const anchor = readingAnchor.current;
    readingAnchor.current = null;
    if (!visible || !area || !anchor || !area.contains(anchor.range.startContainer)) return;
    area.scrollTop += anchor.range.getBoundingClientRect().top - area.getBoundingClientRect().top - anchor.offset;
    scrollPosition.current = area.scrollTop;
    saveReaderPreference(`textReadingPosition:${id}`,String(area.scrollTop));
  },[navigation.open,notesCollapsed,zoom,visible,id,readingAnchor,scrollPosition]);
  const web = useMemo(() => web_layout ? JSON.parse(web_layout) as WebLayout : null,[web_layout]);
  const captions = useMemo(() => captionNodes(web?.nodes ?? [],content), [web,content]);
  const lines = useMemo(() => web ? [] : readingLines(content, format === 'md'), [content,format,web]);
  const bookmarkKey = `textBookmarks:${id}`;
  const [bookmarkError,setBookmarkError] = useState('');
  const [bookmarks,setBookmarks] = useState<{offset:number;label:string}[]>(()=>{
    try {
      const saved = JSON.parse(localStorage.getItem(bookmarkKey) ?? '[]');
      return Array.isArray(saved) ? saved.filter(item=>item && Number.isSafeInteger(item.offset) &&
        item.offset>=0 && typeof item.label==='string') : [];
    } catch { return []; }
  });
  function updateBookmarks(next:typeof bookmarks) {
    try {
      localStorage.setItem(bookmarkKey,JSON.stringify(next));
      setBookmarks(next);setBookmarkError('');
    } catch {setBookmarkError('无法保存书签');}
  }
  function addBookmark() {
    const anchors = ref.current?.querySelectorAll<HTMLElement>('[data-offset]');
    if (!anchors?.length) return;
    const top = scrollArea.current?.getBoundingClientRect().top ?? 0;
    const anchor = Array.from(anchors).find(node=>node.getBoundingClientRect().bottom>top) ?? anchors[anchors.length-1];
    const offset = Number(anchor.dataset.offset);
    if (bookmarks.some(item=>item.offset===offset)) return;
    const label = content.slice(offset).trim().split('\n')[0]?.slice(0,65) || '正文';
    updateBookmarks([...bookmarks,{offset,label}].sort((a,b)=>a.offset-b.offset));
  }
  function jump(offset:number) {
    const anchors = ref.current?.querySelectorAll<HTMLElement>('[data-offset]');
    if (!anchors?.length) return;
    let target = anchors[0];
    for (const anchor of Array.from(anchors)) {
      if (Number(anchor.dataset.offset)>offset) break;
      target = anchor;
    }
    target.scrollIntoView({block:'start'});
  }
  const toc = useMemo(()=>{
    if (!web) return lines.filter(line=>/^h[1-6]$/.test(line.kind) && line.pieces.length)
      .map(line=>({offset:line.pieces[0].start,label:line.pieces.map(piece=>piece.text).join('')}));
    const headings:{offset:number;label:string}[] = [];
    function textNodes(node:WebNode):Extract<WebNode,{type:'text'}>[] {
      if (node.type==='text') return [node];
      return node.type==='element' ? node.children.flatMap(textNodes) : [];
    }
    function visit(node:WebNode) {
      if (node.type!=='element') return;
      if (/^h[1-6]$/.test(node.tag)) {
        const parts = textNodes(node);
        if (parts.length) headings.push({offset:parts[0].start,label:parts.map(part=>content.slice(part.start,part.end)).join('').trim()});
      } else node.children.forEach(visit);
    }
    web.nodes.forEach(visit);
    return headings;
  },[web,lines,content]);
  const edits = useRef(new Map<number,InlineEdit>());
  const [pendingEdit,setPendingEdit] = useState<{edit:EditedDocument;removed:number}|null>(null);
  function finishEdit() {
    const changes = [...edits.current.values()].sort((a,b)=>a.start-b.start);
    const parts: string[] = [];
    let end = 0;
    const ranges = new Map<number,{start:number;end:number}>();
    let delta = 0;
    for (const edit of changes) {
      parts.push(content.slice(end,edit.start),edit.text);
      end = edit.end;
      const start = edit.start + delta;
      ranges.set(edit.start,{start,end:start + edit.text.length});
      delta += edit.text.length - (edit.end - edit.start);
    }
    const changed = changes.flatMap(edit=>
      changedRanges(edit.unchanged,edit.end-edit.start,edit.text.length).map(range=>
        ({...range,start:range.start+edit.start,end:range.end+edit.start})));
    let changeIndex = 0;
    let shift = 0;
    const positions = data.marks.map(mark=>{
      while (changed[changeIndex]?.end <= mark.start) shift += changed[changeIndex++].delta;
      return {id:mark.id,start:changed[changeIndex]?.start < mark.end ? null : mark.start + shift};
    });
    let nodeShift = 0;
    function rebase(nodes:WebNode[]):WebNode[] {
      return nodes.map(node=>{
        if (node.type==='element') return {...node,children:rebase(node.children)};
        if (node.type==='image') return node;
        const moved = ranges.get(node.start);
        if (!moved) return {...node,start:node.start+nodeShift,end:node.end+nodeShift};
        nodeShift = moved.end - node.end;
        return {...node,...moved};
      }).filter(node=>node.type!=='text' || node.start!==node.end);
    }
    const removed = positions.filter(position=>position.start===null).length;
    parts.push(content.slice(end));
    const edit = {content:parts.join(''),positions,webNodes:web && changes.length ? rebase(web.nodes) : undefined};
    if (removed) setPendingEdit({edit,removed});
    else save(edit);
  }
  function editable(start:number, text:string) {
    if (!editing) return {};
    return {
      contentEditable:'plaintext-only' as const, suppressContentEditableWarning:true,
      onKeyDown:(event:ReactKeyboardEvent<HTMLElement>)=>{
        if (event.key==='Enter' && !event.nativeEvent.isComposing) event.preventDefault();
      },
      onPaste:(event:ClipboardEvent<HTMLElement>)=>{
        const pasted = event.clipboardData.getData('text/plain');
        if (/[\r\n]/.test(pasted)) {
          event.preventDefault();
          document.execCommand('insertText',false,pasted.replace(/\r\n?|\n/g,' '));
        }
      },
      onInput:(event:FormEvent<HTMLElement>)=>{
        if (pendingEdit) setPendingEdit(null);
        const element = event.currentTarget;
        const value = element.textContent ?? '';
        if (value===text) {edits.current.delete(start);return;}
        const selection = window.getSelection();
        const cursor = selection?.anchorNode && element.contains(selection.anchorNode)
          ? (()=>{const range=document.createRange();range.selectNodeContents(element);
              range.setEnd(selection.anchorNode,selection.anchorOffset);return range.toString().length;})()
          : value.length;
        const previous = edits.current.get(start);
        const unchanged = keepUnchanged(previous?.unchanged ?? [{start:0,end:text.length,offset:0}],
          previous?.text ?? text,value,cursor);
        edits.current.set(start,{start,end:start+text.length,text:value,unchanged});
      }
    };
  }
  const input = useRef<HTMLInputElement>(null);
  const [searchOpen, setSearchOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [current, setCurrent] = useState(0);
  const blocks = useMemo(() => {
    if (!web) return lines.map(line => line.pieces);
    const result: Piece[][] = [];
    let block: Piece[] = [];
    function visit(nodes: WebNode[]) {
      for (const node of nodes) {
        if (node.type === 'text') {
          const previous = block.at(-1);
          if (!previous || previous.start + previous.text.length !== node.start) {
            block = [];
            result.push(block);
          }
          block.push({start:node.start,text:content.slice(node.start,node.end)});
        } else if (node.type === 'element' && webTags.has(node.tag)) {
          visit(node.children);
          if (['p','div','li','br','hr','td','th','figcaption'].includes(node.tag)) block = [];
        } else {
          block = [];
        }
      }
    }
    visit(web.nodes);
    return result;
  }, [web,lines,content]);
  const matches = useMemo(() => articleMatches(blocks,query), [blocks,query]);
  const active = matches.length ? current % matches.length : 0;
  useEffect(() => {
    if (matches.length) ref.current?.querySelector('.article-hit.active')?.scrollIntoView({block:'center'});
  }, [matches,active]);
  function focusSearch() {
    input.current?.focus({preventScroll:true});
    input.current?.select();
  }
  useLayoutEffect(() => {
    if (searchOpen) focusSearch();
  }, [searchOpen]);
  function closeSearch() {
    setSearchOpen(false);
    setQuery('');
    setCurrent(0);
    ref.current?.focus({preventScroll:true});
  }
  function openSearch() {
    setSearchOpen(true);
    focusSearch();
  }
  useArticleSearchShortcut(visible && !editing,openSearch);
  function move(step: number) {
    if (matches.length) setCurrent((active + step + matches.length) % matches.length);
  }
  useEffect(() => {
    if (focused) document.getElementById('mark-' + focused)?.scrollIntoView({block:'center'});
  }, [focused,data]);
  function capture(doubleClick = false) {
    if (editing || !ref.current) return;
    const s = selectedOffsets(ref.current,content,window.getSelection());
    if (!s) return;
    const existing = doubleClick && data.marks.find(mark => mark.start <= s.start && mark.end >= s.end);
    if (existing) { onMark(existing); return; }
    onSelect({...s,document_id:id}, doubleClick);
  }
  let matchIndex = 0;
  function highlight(start: number, end: number) {
    while (matches[matchIndex]?.end <= start) matchIndex++;
    const nodes: ReactNode[] = [];
    let cursor = start;
    for (let i = matchIndex; matches[i]?.start < end; i++) {
      const a = Math.max(start,matches[i].start);
      const b = Math.min(end,matches[i].end);
      if (a > cursor) nodes.push(content.slice(cursor,a));
      nodes.push(<mark key={a} className={i === active ? 'article-hit active' : 'article-hit'}>{content.slice(a,b)}</mark>);
      cursor = b;
    }
    if (cursor < end) nodes.push(content.slice(cursor,end));
    return nodes;
  }
  let markIndex = 0;
  function decorate(start: number, text: string): ReactNode[] {
    const end = start + text.length;
    while (data.marks[markIndex]?.end <= start) markIndex++;
    const nodes: ReactNode[] = [];
    let cursor = start;
    for (let i = markIndex; data.marks[i]?.start < end; i++) {
      const m = data.marks[i];
      const a = Math.max(start,m.start);
      const b = Math.min(end,m.end);
      if (a > cursor) nodes.push(...highlight(cursor,a));
      nodes.push(<a key={m.id} id={a === m.start ? 'mark-' + m.id : undefined} href={'#mark-' + m.id}
        title={m.meaning} className={focused === m.id ? 'focused' : ''}
        onClick={e => {
          e.preventDefault();
          window.getSelection()?.removeAllRanges();
          onMark(m);
        }}>
        {highlight(a,b)}</a>);
      cursor = b;
    }
    if (cursor < end) nodes.push(...highlight(cursor,end));
    return nodes;
  }
  function renderWeb(node: WebNode, index: number): ReactNode {
    const className = captions.has(node) ? 'web-caption' : undefined;
    if (node.type === 'text') {
      const text = content.slice(node.start,node.end);
      return <span key={index} className={className} data-offset={node.start} {...editable(node.start,text)}>
        {editing ? text : decorate(node.start,text)}
      </span>;
    }
    if (node.type === 'image') {
      const src = web?.images[node.index];
      return src && /^data:image\/(png|jpeg|gif|webp|avif);base64,/.test(src)
        ? <img key={index} src={src} alt={node.alt} loading="lazy"/>
        : <small className="image-fallback" key={index}>图片未能导入{web?.image_errors?.[node.index] ? ' · ' + web.image_errors[node.index] : ''}{node.alt ? ' · ' + node.alt : ''}</small>;
    }
    if (!webTags.has(node.tag)) return null;
    if (node.tag === 'br' || node.tag === 'hr') return React.createElement(node.tag,{key:index});
    return React.createElement(node.tag,{key:index,className},node.children.map(renderWeb));
  }
  return <>
    <div className="reader-tools">
      {editing ? <>
        {pendingEdit && <p role="status">保存后会移除被改动的 {pendingEdit.removed} 条原文标注，对应的词库义项会保留。</p>}
        <div className="row">
          <button className="primary" disabled={busy} onClick={()=>pendingEdit ? save(pendingEdit.edit) : finishEdit()}>
            {pendingEdit ? '确认保存' : '保存'}
          </button>
          <button disabled={busy} onClick={cancel}>取消</button>
        </div>
      </> : null}
    </div>
    {!editing && <ReaderToolbar target={toolbar} zoom={zoom} changeZoom={changeZoom}
      previous={()=>turn(-1)} next={()=>turn(1)} bookmark={{toggle:()=>{
        addBookmark();
        if (!navigation.open && scrollArea.current) readingAnchor.current = captureReadingAnchor(scrollArea.current);
        navigation.showBookmarks();
      }}}>
        <button className="reader-icon" type="button" onClick={openSearch} aria-label="文章内搜索" title="文章内搜索 · ⌘F / Ctrl+F">
          <svg aria-hidden="true" viewBox="0 0 24 24"><circle cx="10.5" cy="10.5" r="5.5"/><path d="m15 15 5 5"/></svg>
        </button>
        <button className="primary reader-icon" type="button" disabled={busy} onClick={beginEdit} aria-label="编辑正文" title="编辑正文">
          <svg aria-hidden="true" viewBox="0 0 24 24"><path d="m4 20 1-5L16 4a2.1 2.1 0 0 1 3 3L8 19l-4 1ZM14 6l4 4"/></svg>
        </button>
    </ReaderToolbar>}
    {!editing && searchOpen && <div className="article-search" role="search" aria-label="文章内搜索"
      onKeyDown={e=>{
        if (e.key === 'Escape' && !e.nativeEvent.isComposing) {e.preventDefault();closeSearch();}
      }}>
      <input ref={input} type="search" aria-label="搜索当前文章" placeholder="搜索当前文章"
        value={query} onChange={e=>{setQuery(e.target.value);setCurrent(0);}}
        onKeyDown={e=>{
          if (e.nativeEvent.isComposing) return;
          if (e.key === 'Enter') {e.preventDefault();move(e.shiftKey ? -1 : 1);}
        }}/>
      <span role="status">{query.trim() ? matches.length ? `${active + 1} / ${matches.length}` : '无匹配' : ''}</span>
      <button type="button" disabled={!matches.length} onClick={()=>move(-1)}>上一处</button>
      <button type="button" disabled={!matches.length} onClick={()=>move(1)}>下一处</button>
      <button type="button" onClick={closeSearch}>关闭</button>
    </div>}
    <div className={navigation.open && !editing ? 'pdf-reader has-outline' : 'pdf-reader'}>
    {!editing && navigation.open && <ReaderNavigation navigation={navigation} bookmarks={<>
      {bookmarkError && <p role="alert">{bookmarkError}</p>}
      {bookmarks.length ? <ul>{bookmarks.map(item=><li className="bookmark-row" key={item.offset}>
        <button type="button" onClick={()=>jump(item.offset)}>{item.label}</button>
        <button type="button" aria-label={`删除书签：${item.label}`}
          onClick={()=>updateBookmarks(bookmarks.filter(bookmark=>bookmark.offset!==item.offset))}>×</button>
      </li>)}</ul> : <p className="muted">点击标题行的 ☆ 添加当前位置书签。</p>}
    </>} contents={<>
      {toc.length ? <ul>{toc.map(item=><li key={item.offset}>
        <button type="button" onClick={()=>jump(item.offset)}>{item.label}</button>
      </li>)}</ul> : <ul><li><button type="button" onClick={()=>jump(0)}>正文开头</button></li></ul>}
    </>}/>}
    <div className="pdf-page-area text-reading-area" ref={scrollArea} onScroll={event=>{
      if (visible) {
        scrollPosition.current = event.currentTarget.scrollTop;
        if (!editing) {
          const area = event.currentTarget;
          const distance = area.scrollHeight - area.clientHeight;
          const percent = distance>0 ? Math.round(100*area.scrollTop/distance) : 100;
          saveReaderPreference(`textReadingPosition:${id}`,String(scrollPosition.current));
          saveReaderPreference(`readingProgress:${id}`,`${Math.max(0,Math.min(100,percent))}%`);
        }
      }
    }}>
    <div className={web ? 'reading web-reading' + (data.document.source_url ? '' : ' epub-reading') : 'reading'} ref={ref} tabIndex={0} style={{zoom}}
    onMouseUp={e => {if(e.detail < 2) capture();}} onDoubleClick={() => capture(true)}
    onKeyUp={e => {if (e.shiftKey) capture();}}>
    {web ? web.nodes.map(renderWeb) : lines.map(line => <div key={line.start} className={'line ' + line.kind}>
      {(editing && !line.pieces.length && line.kind==='text' ? [{start:line.start,text:''} as Piece] : line.pieces).map(piece => {
        const Tag = piece.kind ?? 'span';
        return <Tag key={piece.start} data-offset={piece.start} {...editable(piece.start,piece.text)}>
          {editing ? piece.text : decorate(piece.start,piece.text)}
        </Tag>;
      })}
    </div>)}
  </div></div></div></>;
}

function App() {
  const [mode,setMode] = useState<'texts'|'reader'|'library'|'settings'|'help'>('texts');
  const [lib,setLib] = useState<Library>(empty);
  const [webUrl,setWebUrl] = useState('');
  const [query,setQuery] = useState('');
  const [filter,setFilter] = useState('');
  const [read,setRead] = useState<Read | null>(null);
  const [editing,setEditing] = useState(false);
  const navigation = useReaderNavigation();
  const [recentDocuments,setRecentDocuments] = useState<Record<string,number>>(()=>{
    try { return JSON.parse(readReaderPreference('textLibraryRecent')) || {}; }
    catch { return {}; }
  });
  const [notesCollapsed,setNotesCollapsed] = useState(false);
  const [readerToolbar,setReaderToolbar] = useState<HTMLDivElement|null>(null);
  const readerScroll = useRef(0);
  const readerFrame = useRef<HTMLDivElement>(null);
  const readingAnchor = useRef<ReturnType<typeof captureReadingAnchor>>(null);
  function rememberReadingAnchor() {
    const area = readerFrame.current?.querySelector<HTMLElement>('.text-reading-area');
    if (area && !editing) readingAnchor.current = captureReadingAnchor(area);
  }
  const [detail,setDetail] = useState<Detail | null>(null);
  const [bulkMode,setBulkMode] = useState(false);
  const [bulkGroups,setBulkGroups] = useState<string[]>([]);
  const [bulkConfirm,setBulkConfirm] = useState(false);
  const [deleteGroupId,setDeleteGroupId] = useState('');
  const [renaming,setRenaming] = useState<{id:string;title:string}|null>(null);
  const [deleteDocId,setDeleteDocId] = useState('');
  const [focus,setFocus] = useState('');
  const [selection,setSelection] = useState<Selection | null>(null);
  const [activeMark,setActiveMark] = useState<Mark | null>(null);
  const [candidate,setCandidate] = useState<Candidates | null>(null);
  const [autoSuggest,setAutoSuggest] = useState(()=>localStorage.getItem('autoSuggestOnDoubleClick')!=='off');
  const [suggesting,setSuggesting] = useState(false);
  const selectionRequest = useRef(0);
  const autoAnalysis = useRef(false);
  const [autoLemma,setAutoLemma] = useState(()=>localStorage.getItem('autoDictionaryLemma')!=='off');
  const [lemma,setLemma] = useState('');
  const [meaning,setMeaning] = useState('');
  const [contextMeaning,setContextMeaning] = useState('');
  const [attachGroup,setAttachGroup] = useState('');
  const [attachSense,setAttachSense] = useState('');
  const [newFields,setNewFields] = useState(blank);
  const [aiNote,setAiNote] = useState('');
  const [learning,setLearning] = useState<Learning | null>(null);
  const [markSuggestion,setMarkSuggestion] = useState<{id: string; learning: Learning} | null>(null);
  const [groupFields,setGroupFields] = useState(blank);
  const [selectedSenses,setSelectedSenses] = useState<string[]>([]);
  const [selectedMarks,setSelectedMarks] = useState<string[]>([]);
  const [markSense,setMarkSense] = useState('');
  const [markFilter,setMarkFilter] = useState('');
  const [reclassMeaning,setReclassMeaning] = useState('');
  const [target,setTarget] = useState('');
  const [organizing,setOrganizing] = useState(false);
  const [operation,setOperation] = useState('move');
  const [splitFields,setSplitFields] = useState(blank);
  const [savedAi,setSavedAi] = useState(() => ({
    endpoint: localStorage.getItem('endpoint') ?? '', model: localStorage.getItem('model') ?? ''
  }));
  const [endpoint,setEndpoint] = useState(savedAi.endpoint);
  const [model,setModel] = useState(savedAi.model);
  const [keyState,setKeyState] = useState('尚未配置');
  const [pronunciationLibrary,setPronunciationLibrary] = useState(()=>localStorage.getItem('pronunciationLibrary') ?? '');
  const [keyRevision,setKeyRevision] = useState(0);
  const aiConfigured = Boolean(savedAi.endpoint && savedAi.model);
  const [key,setKey] = useState('');
  const [notice,setNotice] = useState('');
  const [error,setError] = useState('');
  const [busy,setBusy] = useState(false);
  const lock = useRef(false);
  const [snapshots,setSnapshots] = useState<Snapshot[]>([]);
  const [snapshotId,setSnapshotId] = useState('');
  const [confirmSnapshot,setConfirmSnapshot] = useState<'restore'|'delete'|null>(null);
  useEffect(() => {
    if (mode!=='settings') return;
    let active = true;
    void call<Snapshot[]>('snapshot_list').then(
      items=>{if (active) setSnapshots(items);},
      error=>{if (active) setError(String(error));}
    );
    return ()=>{active=false;};
  },[mode]);
  const selectedSnapshot = snapshots.find(snapshot=>snapshot.id===snapshotId);
  const previewFile = useRef<HTMLInputElement>(null);
  const [backupPreview,setBackupPreview] = useState<{name:string;data:BackupPreviewData}|null>(null);

  async function task(action: () => Promise<void>) {
    if (lock.current) return;
    lock.current = true;
    setBusy(true);
    setError('');
    setNotice('');
    try { await action(); }
    catch (e) { setError(typeof e === 'string' ? e : e instanceof Error ? e.message : '操作失败'); }
    finally { lock.current = false; setBusy(false); }
  }
  async function refresh() {
    setLib(await call<Library>('list'));
  }
  useEffect(() => {void task(refresh);}, []);

  useEffect(() => {
    if (mode !== 'settings') return;
    let current = true;
    if (!aiConfigured) { setKeyState('尚未配置'); return; }
    setKeyState('正在检查…');
    void call<'local'|'environment'|'missing'>('key_status',savedAi).then(
      saved => { if (current) setKeyState(saved === 'local' ? '已配置（本机）' : saved === 'environment' ? '已配置（环境变量）' : '未配置'); },
      () => { if (current) setKeyState('无法读取密钥状态'); }
    );
    return () => { current = false; };
  }, [mode,savedAi,aiConfigured,keyRevision]);

  function saveAi() {
    const config = {endpoint:endpoint.trim(),model:model.trim()};
    localStorage.setItem('endpoint',config.endpoint);
    localStorage.setItem('model',config.model);
    setSavedAi(config);
  }

  function invalidateSuggestion() {
    selectionRequest.current++;
    setSuggesting(false);
  }
  useEffect(() => {
    if (mode !== 'reader') invalidateSuggestion();
  }, [mode]);

  function resetSelection() {
    invalidateSuggestion();
    autoAnalysis.current = false;
    setSelection(null); setCandidate(null); setActiveMark(null); setAiNote(''); setLearning(null); setMarkSuggestion(null);
  }
  async function openDoc(id: string, markId = '') {
    setRenaming(null);
    const data = await call<Read>('document',{id});
    const recent = {...recentDocuments,[id]:Date.now()};
    setRecentDocuments(recent);
    saveReaderPreference('textLibraryRecent',JSON.stringify(recent));
    if (read?.document.id !== id) {
      const saved = Number(readReaderPreference(`textReadingPosition:${id}`));
      readerScroll.current = Number.isFinite(saved) && saved>=0 ? saved : 0;
    }
    setEditing(false); setDeleteDocId(''); setRead(data); setFocus(markId); setMode('reader'); resetSelection();
  }
  function clearGroup() {
    setDeleteGroupId('');
    setDetail(null); setGroupFields(blank()); setSelectedSenses([]); setSelectedMarks([]);
    setOrganizing(false); setTarget(''); setOperation('move'); setSplitFields(blank()); setMarkSense(''); setMarkFilter(''); setReclassMeaning('');
  }
  function openLibrarySense(lemma: string) {
    setSilFilter(''); setQuery(lemma); setFilter(lemma); setMode('library');
  }
  async function importFile() {
    const imported = await importLocalFile();
    if (!imported) return;
    await refresh(); await openDoc(imported.id); setNotice('已导入 ' + imported.title);
  }
  async function deleteSense(id: string) {
    await call('sense_delete',{id});
    setRead(previous=>previous && {...previous,marks:previous.marks.filter(mark=>mark.sense_id!==id)});
    resetSelection();
    await refresh();
    setNotice('已删除义项及其原文链接');
  }
  async function deleteDocument(id: string) {
    await call('document_delete',{id});
    setRead(null); setMode('texts'); setDeleteDocId(''); setFocus(''); resetSelection();
    await refresh();
    setNotice('文章已删除');
  }
  async function suggestSelection(chosen: Selection, request: number) {
    setSuggesting(true);
    setAiNote('');
    try {
      const response = await call<Suggestion>('suggest',{...chosen,...savedAi,ungrouped_capture:true});
      if (selectionRequest.current !== request) return;
      const s = response.suggestion;
      setLearning(s.learning);
      setLemma(s.lemma);setMeaning(s.meaning);setContextMeaning(s.context_meaning);setAttachSense(s.sense_id ?? '');setAttachGroup(s.group_id ?? '');
      if (!s.group_id) {
        const terms = s.group_vocabulary?.length ? s.group_vocabulary : [
          {expression:s.lemma,note:s.meaning}, ...s.learning.related
        ];
        const lines = terms.map(term =>
          [term.expression,term.note].map(value=>value.replace(/[|\r\n]+/g,' ').trim()).join(' | ')
        );
        setNewFields({name:s.group_name ?? '',description:s.group_description || s.meaning,
          vocabulary:[...new Set(lines)].join('\n'),sil_code:s.sil_code});
      }
      setAiNote((s.sense_id ? 'AI 建议复用已有义项 · 置信度 ' : 'AI 建议 · 置信度 ') + Math.round(s.confidence*100) + '%' +
        (typeof response.usage?.total_tokens === 'number' ? ' · ' + response.usage.total_tokens + ' tokens' : '') +
        (s.group_reason ? ' · ' + s.group_reason : '') +
        (response.group_check_failed ? ' · 语义组补查未完成，请核对归组。' : ''));
    } catch (error) {
      if (selectionRequest.current === request) {
        setAiNote(error instanceof Error ? error.message : String(error));
      }
    } finally {
      if (selectionRequest.current === request) setSuggesting(false);
    }
  }

  function openMarkNote(mark: Mark) {
    resetSelection();
    setActiveMark(mark);
    setNotesCollapsed(false);
  }

  function capture(s: Selection, doubleClick: boolean) {
    if (lock.current) return;
    setNotesCollapsed(false);
    const request = ++selectionRequest.current;
    const automatic = doubleClick && autoSuggest && aiConfigured;
    autoAnalysis.current = automatic;
    setSuggesting(automatic);
    setSelection(s); setActiveMark(null); setCandidate(null); setAiNote(''); setLearning(null);
    setLemma(s.surface); setMeaning(''); setContextMeaning(''); setAttachGroup(''); setAttachSense(''); setNewFields(blank());
    void call<Candidates>('candidates',s).then(options=>{
      if (selectionRequest.current !== request) return;
      setCandidate(options);
      if (options.reused.length === 1) {
        setAttachSense(options.reused[0].id);
        setAiNote('找到相同语境的既有义项，请核对后保存。');
        setSuggesting(false);
      } else if (automatic) {
        void suggestSelection(s, request);
      }
    },()=>{
      if (selectionRequest.current !== request) return;
      setAiNote('无法读取词库候选，可以手动填写或重试 AI 建议。');
      setSuggesting(false);
    });
  }
  function submit(e: FormEvent, action: () => Promise<void>) {
    e.preventDefault();
    void task(action);
  }
  const candidateSenses = [...new Map([...(candidate?.reused ?? []),...(candidate?.exact ?? [])].map(s => [s.id,s])).values()];
  const selectedSense = candidateSenses.find(sense=>sense.id===attachSense);
  const lookupWord = selection ? dictionaryLookupWord(selection.surface,lemma,
    candidateSenses.find(sense=>sense.id===attachSense)?.lemma,undefined) : '';
  const silDomains = useSilCatalog();
  const [silFilter,setSilFilter] = useState('');
  const [directOnly,setDirectOnly] = useState(false);
  useEffect(()=>{setDirectOnly(false);},[silFilter]);
  const [newSemanticGroup,setNewSemanticGroup] = useState(false);
  const silTitle = silFilter==='unclassified' ? '未归类' : silDomains.find(domain=>domain.code===silFilter)?.name ?? silFilter;

  const [silOrganizerCategory,setSilOrganizerCategory] = useState<string|null>(null);
  const [silPageRevision,setSilPageRevision] = useState(0);
  const [auditCount,setAuditCount] = useState(0);
  const showSilOrganizer = silOrganizerCategory===silFilter;
  const silOrganizer = silFilter ? <SilOrganizer key={silFilter} parent={silFilter==='unclassified' ? '' : silFilter} busy={busy} configured={aiConfigured} config={savedAi} apply={(rows,backup,keepOpen,done)=>void task(async()=>{
          if (backup) setSnapshots(await call<Snapshot[]>('snapshot_create'));
          const count = await call<number>('sil_memberships',{rows:rows.map(row=>({...row,codes:row.codes ?? (row.code && row.code!=='unclassified' ? [row.code] : [])})).filter(row=>row.codes.length)});
          done();
          resetSelection();
          if (!keepOpen) {
            setSilOrganizerCategory(null);
            clearGroup();
            setSilPageRevision(revision=>revision+1);
          }
          await refresh();
          setNotice(`已应用 ${count} 条 SIL 分类`);
          if (read) setRead(await call<Read>('document',{id:read.document.id}));
        })}/> : null;
  const activeLearning = readLearning(activeMark?.learning);
  return <div className={mode==='reader' && read ? 'app pdf-app' : 'app'}>
    <header className="top"><div className="brand"><img src="./icon.png" alt=""/> Semantic Reader</div>
      <nav aria-label="工作区">
        <button aria-current={mode==='texts' ? 'page' : undefined} className={mode==='texts'?'current':''}
          onClick={()=>setMode('texts')}>文本库</button>
        <button aria-current={mode==='reader' ? 'page' : undefined} className={mode==='reader'?'current':''}
          disabled={!read || busy} onClick={()=>{
            if (mode==='reader') return;
            if (read && !editing) void task(()=>openDoc(read.document.id));
            else setMode('reader');
          }}>阅读器</button>
        <button aria-current={mode==='library' ? 'page' : undefined} className={mode==='library'?'current':''} onClick={() => setMode('library')}>语义库</button>
        <button aria-current={mode==='settings' ? 'page' : undefined} className={mode==='settings'?'current':''} onClick={() => setMode('settings')}>设置</button>
        <button aria-current={mode==='help' ? 'page' : undefined} className={mode==='help'?'current':''}
          onClick={() => setMode('help')}>使用说明</button>
      </nav>
      <span className="local">● 本地词汇库</span>
    </header>
    <div className="status" aria-live="polite">{error ? <span className="error" role="alert">{error}</span> : busy ? '处理中…' : notice}</div>
    <div className={mode==='reader'
      ? 'workspace reading-workspace' + (notesCollapsed ? ' notes-collapsed' : '')
      : mode==='library' ? 'workspace two' : 'workspace'}>
      <div className="text-library-view" hidden={mode!=='texts'}>
        <TextLibrary documents={lib.documents} recent={recentDocuments} busy={busy} visible={mode==='texts'}
          open={id=>void task(()=>openDoc(id))}
          rename={(id,title,done)=>void task(async()=>{
            const savedTitle = await call<string>('document_rename',{id,title});
            setRead(previous=>previous?.document.id===id
              ? {...previous,document:{...previous.document,title:savedTitle}} : previous);
            await refresh();
            done();
            setNotice('标题已更新');
          })}
          imports={<>
            <button className="primary" disabled={busy} onClick={()=>void task(importFile)}>＋ 导入文件</button>
            <details className="library-web-import"><summary>从网址导入</summary>
              <form className="web-import" onSubmit={e=>submit(e,async()=>{
                const id = await call<string>('import_web',{url:webUrl.trim()});
                setWebUrl(''); await refresh(); await openDoc(id); setNotice('网页已导入');
              })}>
                <label>网页地址<input type="url" placeholder="https://example.com/article" value={webUrl}
                  maxLength={4096} required disabled={busy} onChange={e=>setWebUrl(e.target.value)}/></label>
                <button disabled={busy || !webUrl.trim()}>导入网页</button>
              </form>
            </details>
          </>}/>
      </div>
      {mode==='library' && <aside className="shelf">
        <div className="eyebrow">SEMANTIC LIBRARY</div>
        <form className="search" onSubmit={e => submit(e,async () => {
          const q = query.trim(); setFilter(q);
        })}>
          <input aria-label="搜索" placeholder="搜索词或释义" value={query} maxLength={200} onChange={e=>setQuery(e.target.value)}/>
          <button disabled={busy} aria-label="执行搜索">⌕</button>
        </form>
        {filter && <button className="quiet" onClick={()=>{setQuery('');setFilter('');}}>清除搜索 ×</button>}
        <SilNavigation directOnly={directOnly} setDirectOnly={setDirectOnly} key={silFilter} counts={lib.sil_counts ?? []} domains={silDomains} value={silFilter} disabled={busy} change={code=>{
          setSilOrganizerCategory(null);setSilFilter(code);
          setQuery('');setFilter('');
        }}/>
      </aside>}

        <div ref={readerFrame} className={read ? 'reader-frame pdf-frame' : 'reader-frame'} hidden={mode!=='reader'}>
        <main className="page">
          {read ? <>
            <div className="reader-heading">
            <div className="reader-title-bar">
              <button type="button" onClick={()=>setMode('texts')}>‹ 文本库</button>
              {!editing && <button type="button" aria-expanded={navigation.open}
                aria-controls={navigation.open ? 'reader-navigation' : undefined}
                onClick={()=>{rememberReadingAnchor();navigation.toggle();}}>目录与书签</button>}
              <h1 title={read.document.title}>{read.document.title.replace(/\.(md|markdown|txt|pdf|epub)$/i,'')}</h1>
              {!editing && <div className="reader-title-tools" ref={setReaderToolbar}/>}
              <details className="reader-document-menu" key={read.document.id} onBlur={event=>{
                if (event.relatedTarget && !event.currentTarget.contains(event.relatedTarget)) event.currentTarget.open=false;
              }} onKeyDown={event=>{
                if (event.key==='Escape' && event.currentTarget.open) {
                  event.stopPropagation();event.currentTarget.open=false;
                  event.currentTarget.querySelector('summary')?.focus();
                }
              }}>
                <summary aria-label="文档选项" title="文档选项">⋯</summary>
                <div className="reader-document-menu-panel">
                  <p className="muted">文档信息 · {read.marks.length} 条标注</p>
                  {read.document.source_url && <p className="muted">来源 · {read.document.source_url}</p>}
                  {read.document.web_layout && read.document.source_url && <button type="button" disabled={busy || editing} onClick={event=>{
                    event.currentTarget.closest('details')?.removeAttribute('open');
                    void task(async()=>{
                      const id = read.document.id;
                      const result = await call<{saved:number; missing:number}>('repair_images',{id});
                      await openDoc(id);
                      setNotice(`已保存 ${result.saved} 张图片${result.missing ? `，仍有 ${result.missing} 张未能导入` : ''}`);
                    });
                  }}>重新下载图片</button>}
                  <button type="button" disabled={busy || editing} onClick={event=>{
                    event.currentTarget.closest('details')?.removeAttribute('open');
                    setDeleteDocId('');
                    setRenaming({id:read.document.id,title:read.document.title.replace(/\.(md|markdown|txt|pdf|epub)$/i,'')});
                  }}>修改标题</button>
                  <button type="button" className="quiet delete-document" disabled={busy || editing} onClick={event=>{
                    event.currentTarget.closest('details')?.removeAttribute('open');
                    setRenaming(null);setDeleteDocId(read.document.id);
                  }}>删除文章</button>
                </div>
              </details>
            </div>
            {renaming?.id===read.document.id && <form className="rename-document" onSubmit={event=>submit(event,async()=>{
              const title = await call<string>('document_rename',renaming);
              setRead(previous=>previous?.document.id===renaming.id
                ? {...previous,document:{...previous.document,title}} : previous);
              setRenaming(null);
              await refresh();
              setNotice('标题已更新');
            })}>
              <label>文章标题<input autoFocus required maxLength={255} value={renaming.title}
                onChange={event=>setRenaming({...renaming,title:event.target.value})}/></label>
              <div className="row">
                <button type="submit" className="primary" disabled={busy || !renaming.title.trim()}>保存标题</button>
                <button type="button" disabled={busy} onClick={()=>setRenaming(null)}>取消</button>
              </div>
            </form>}
            {deleteDocId === read.document.id && <section className="delete-confirm" aria-label="确认删除文章">
              <p>删除《{read.document.title}》及其 {read.marks.length} 条原文标注和随标注保存的学习提示？词库义项、语义组和其他文章会保留。删除后无法在应用内撤销。</p>
              <div className="row">
                <button disabled={busy} onClick={()=>setDeleteDocId('')}>取消</button>
                <button className="delete-document" disabled={busy} onClick={()=>void task(()=>deleteDocument(read.document.id))}>确认删除文章</button>
              </div>
            </section>}
            </div>
            {read.document.pdf ? <PdfReader key={read.document.id} id={read.document.id} marks={read.marks}
              focused={focus} selection={selection} visible={mode==='reader'} toolbar={mode==='reader' ? readerToolbar : null} navigation={navigation} onSelect={capture} onClearSelection={resetSelection}
              onMark={openMarkNote}/> :
            read.document.epub && read.document.web_layout ? <EpubReader key={read.document.id} id={read.document.id}
              content={read.document.content} layout={read.document.web_layout} marks={read.marks} focused={focus}
              visible={mode==='reader'} toolbar={mode==='reader' ? readerToolbar : null} navigation={navigation} onSelect={capture} onMark={openMarkNote}/> :
            <Reader key={read.document.id + (editing ? ':edit' : '')} data={read} visible={mode==='reader'}
              focused={focus} editing={editing} busy={busy} toolbar={mode==='reader' ? readerToolbar : null} navigation={navigation} readingAnchor={readingAnchor} notesCollapsed={notesCollapsed} scrollPosition={readerScroll} cancel={()=>setEditing(false)}
              beginEdit={()=>{resetSelection();setFocus('');setDeleteDocId('');setEditing(true);}}
              save={({content,positions,webNodes})=>void task(async()=>{
                const {id,content:original} = read.document;
                const removed = await call<number>('document_edit',{id,original,content,positions,web_nodes:webNodes});
                await openDoc(id);
                if (detail) clearGroup();
                setNotice(removed ? `正文已保存，移除了 ${removed} 条原文标注` : '正文已保存');
              })}
              onSelect={capture} onMark={openMarkNote}/>}
          </> : <div className="welcome"><p className="eyebrow">YOUR READING ROOM</p><h1>从一篇文章开始</h1>
            <p>在原文中选中词或短语，保存它在这个语境中的含义。以后点击标注即可回到语义组。</p>
            <button className="primary" onClick={()=>void task(importFile)}>导入第一篇文章</button></div>}
        </main>
        </div>
        {mode==='reader' && <aside className="inspector" aria-label="笔记">
          <button type="button" className="notes-toggle" aria-expanded={!notesCollapsed} aria-controls="reader-notes"
            aria-label={notesCollapsed ? '展开笔记栏' : '收起笔记栏'} title={notesCollapsed ? '展开笔记栏' : '收起笔记栏'}
            onClick={()=>{rememberReadingAnchor();setNotesCollapsed(!notesCollapsed);}}>{notesCollapsed ? '‹' : '›'}</button>
          <div id="reader-notes" hidden={notesCollapsed}>
          <p className="eyebrow">CONTEXT PANEL</p>
          {activeMark ? <>

            <WordHeading surface={activeMark.surface} pos={(markSuggestion?.id === activeMark.id ? markSuggestion.learning : activeLearning)?.part_of_speech}/>
            <PronunciationPanel id={pronunciationLibrary} word={activeMark.surface}
              base={activeMark.lemma} pos={activeLearning?.part_of_speech} context={activeMark.context}
              ai={aiConfigured && autoLemma ? savedAi : undefined}/>
            <section className="word-meaning">
              <h3>本句释义</h3>
              <p>{activeMark.context_meaning?.trim() || <span className="muted">沿用通用释义</span>}</p>
            </section>
            <section className="word-meaning">
              <h3>通用释义{activeMark.lemma && activeMark.lemma.trim().toLowerCase()!==activeMark.surface.trim().toLowerCase() &&
                <> <small>原形 · <b lang="en">{activeMark.lemma}</b></small></>}</h3>
              <p>{activeMark.meaning}</p>
            </section>
            <button className="primary" disabled={busy} onClick={()=>openLibrarySense(activeMark.lemma)}>在语义库查看 →</button>
            <DeleteMark key={activeMark.id} busy={busy} remove={()=>void task(async()=>{
              const deletedSense = await call<boolean>('mark_delete',{id:activeMark.id,document_id:activeMark.document_id});
              setRead(previous=>previous && {...previous,marks:previous.marks.filter(mark=>mark.id!==activeMark.id)});
              resetSelection();
              await refresh();
              setNotice(deletedSense ? '已删除最后一处标注及其义项' : '已删除此处标注');
            })}/>

            {activeLearning && markSuggestion?.id !== activeMark.id && <LearningPanel hidePos hideIpa={Boolean(pronunciationLibrary)} learning={activeLearning} word={activeMark.lemma || activeMark.surface}/>}
            {(!activeLearning?.part_of_speech || !activeLearning.usage_note || (activeLearning.etymology === undefined && activeLearning.mnemonic === undefined)) && markSuggestion?.id !== activeMark.id && <p>
              <button disabled={busy || !aiConfigured} onClick={()=>void task(async()=>{
                const result = await call<Suggestion>('suggest',{
                  document_id:activeMark.document_id,start:activeMark.start,end:activeMark.end,...savedAi
                });
                const next = result.suggestion.learning;
                setMarkSuggestion({id:activeMark.id,learning:activeLearning ? {
                  ...next,...activeLearning,
                  usage_note:activeLearning.usage_note || next.usage_note,
                  example:activeLearning.example || next.example,
                  etymology:activeLearning.etymology ?? next.etymology ?? null,
                  mnemonic:activeLearning.mnemonic ?? next.mnemonic ?? null,
                  related:activeLearning.related.length ? activeLearning.related : next.related,
                  part_of_speech:activeLearning.part_of_speech || next.part_of_speech
                } : next});
              })}>补充学习笔记</button>
            </p>}
            {markSuggestion?.id === activeMark.id && <>
              <LearningPanel hidePos hideIpa={Boolean(pronunciationLibrary)} learning={markSuggestion.learning} word={activeMark.lemma || activeMark.surface}/>
              <button disabled={busy} onClick={()=>void task(async()=>{
                const updated = {...activeMark,learning:JSON.stringify(markSuggestion.learning)};
                await call('learning_save',{id:activeMark.id,learning:markSuggestion.learning});
                setRead(data=>data && ({...data,marks:data.marks.map(mark=>mark.id===updated.id?updated:mark)}));
                setActiveMark(updated);setMarkSuggestion(null);setNotice('学习提示已保存');
              })}>保存学习提示</button>
            </>}
          </> : selection ? <>
            <WordHeading surface={selection.surface} pos={learning?.part_of_speech}/>
            <PronunciationPanel id={pronunciationLibrary} word={selection.surface}
              base={lookupWord} pos={learning?.part_of_speech} context={candidate?.context}
              ai={aiConfigured && autoLemma ? savedAi : undefined}/>
            {candidate?.notes.length ? <section className="saved-notes">
              <h3>词库笔记</h3>
              {candidate.notes.length > 1 && <p className="muted">找到多个义项，请按当前语境查看。</p>}
              {candidate.notes.map(note=><div className="saved-note" key={note.id}>
                <b>{note.lemma}</b><p>{note.meaning}</p>
                {note.learning && <LearningPanel hideIpa={Boolean(pronunciationLibrary)} learning={readLearning(note.learning)!} word={note.lemma}/>}
                <div className="row">
                  <button className="quiet" disabled={busy} onClick={()=>openLibrarySense(note.lemma)}>在语义库查看 →</button>
                  <button disabled={busy || suggesting} onClick={()=>{setAttachSense(note.id);setLearning(null);}}>复用此义项</button>
                </div>
              </div>)}
            </section> : null}
            {candidate && <blockquote>{candidate.context}</blockquote>}
            <button disabled={busy || suggesting || !aiConfigured} onClick={()=>void suggestSelection(selection, selectionRequest.current)}>
              {suggesting ? '正在请求 AI 建议…' : '请求 AI 建议'}
            </button>
            {!aiConfigured ? <p className="muted">可在设置中配置 AI，或直接手动收录。</p> : null}
            {aiNote && <p className="muted" role="status">{aiNote}</p>}
            {learning && <LearningPanel hidePos hideIpa={Boolean(pronunciationLibrary)} learning={learning} word={lookupWord}/>}
            <form onSubmit={e=>submit(e,async ()=>{
              const savedLearning = learning ?? readLearning(candidate?.notes.find(note=>note.id===attachSense)?.learning);
              await call('annotate',{...selection,lemma,meaning,context_meaning:contextMeaning,learning:savedLearning,
                sense_id:attachSense || null,group_id:attachGroup || null,
                ungrouped_capture:true});
              window.getSelection()?.removeAllRanges();
              await refresh(); await openDoc(selection.document_id);setNotice('标注已保存');
            })}>
              <fieldset className="annotation-fields" disabled={suggesting}>
              {candidateSenses.length > 0 && <label>复用既有义项<select value={attachSense} onChange={e=>setAttachSense(e.target.value)}>
                <option value="">填写新义项</option>
                {candidateSenses.map(s=><option key={s.id} value={s.id}>{s.lemma} · {s.meaning}</option>)}
              </select></label>}
              <label>本句释义<textarea value={contextMeaning} maxLength={1200}
                placeholder="可留空，沿用通用释义" onChange={e=>setContextMeaning(e.target.value)}/></label>
              {selectedSense && <section className="word-meaning">
                <h3>通用释义{selectedSense.lemma.trim().toLowerCase()!==selection.surface.trim().toLowerCase() &&
                  <> <small>原形 · <b lang="en">{selectedSense.lemma}</b></small></>}</h3>
                <p>{selectedSense.meaning}</p>
              </section>}
              {!attachSense && <>
                <label>原形或表达<input value={lemma} required maxLength={240} onChange={e=>setLemma(e.target.value)}/></label>
                <label>通用释义<textarea value={meaning} required maxLength={1200} onChange={e=>setMeaning(e.target.value)}/></label>
                <p className="muted">新义项将收录到「未归类」。</p>
              </>}
              <button className="primary full" disabled={busy}>确认收录</button>
              </fieldset>
            </form>
          </> : <p className="muted">双击词语查看词库笔记，或选中短语开始收录。</p>}
          </div>
        </aside>}

      {mode==='library' && <main key={silFilter + ':' + silPageRevision} className="page manage">
        <header className="sil-page-header">
          <h1>{filter ? '搜索结果' : silTitle || '语义库'}</h1>
          {silFilter && !filter && <button type="button" aria-expanded={showSilOrganizer}
            onClick={()=>setSilOrganizerCategory(showSilOrganizer ? null : silFilter)}>{silFilter==='unclassified' ? 'AI 推荐分类' : 'AI 审查分类'}</button>}
        </header>
        {!filter && showSilOrganizer && (silFilter==='unclassified' ? silOrganizer :
          <SilAudit key={silFilter+':'+directOnly} code={silFilter} descendants={!directOnly} count={auditCount}
            domains={silDomains} config={savedAi} busy={busy} configured={aiConfigured} task={task}
            refresh={async()=>{await refresh();setNotice('分类调整已应用');}}/>)}
        <SilSenseList key={silFilter + ':' + directOnly + ':' + filter}
          directOnly={directOnly || silFilter==='unclassified'} category={silFilter} query={filter} library={lib} busy={busy}
          onCount={setAuditCount}
          openCategory={code=>{
            setQuery('');setFilter('');setDirectOnly(false);setSilOrganizerCategory(null);setSilFilter(code);
          }}
          openDocument={(id,mark)=>void task(()=>openDoc(id,mark))}
          move={(sense,code)=>void task(async()=>{
            await call('sil_memberships',{rows:[{...sense,codes:[code]}]});await refresh();
          })}
          remove={(sense,code)=>void task(async()=>{
            await call('sil_memberships',{rows:[{...sense,codes:[code]}],remove:true});await refresh();
          })}/>
      </main>}

      {mode==='help' && <HelpPage/>}

      {mode==='settings' && <main className="page manage">
        <p className="eyebrow">PREFERENCES</p><h1>设置</h1>
        <UpdateSettings/>
        <section><h2>AI 分类接口</h2><p>请求 AI 建议或补充学习提示时，会发送选中文字、附近语境和最多五个候选组。查词时也可能发送同词的已有义项，用来判断是否复用。建议新建组且本地补查找到其他候选时，会再请求一次归组判断。允许 AI 补音标时，应用也会发送选词和附近语境。补充提示也会发送已保存的义项。</p>
          <label className="check"><input type="checkbox" checked={autoSuggest} onChange={e=>{
            setAutoSuggest(e.target.checked);
            localStorage.setItem('autoSuggestOnDoubleClick', e.target.checked ? 'on' : 'off');
          }}/>双击新词时自动请求 AI 建议</label>
          <dl className="api-status">
            <dt>已保存的 API 地址</dt><dd>{savedAi.endpoint || '尚未设置'}</dd>
            <dt>已保存的模型</dt><dd>{savedAi.model || '尚未设置'}</dd>
            <dt>API Key 状态</dt><dd>{keyState}</dd>
          </dl>
          <label>完整 API 地址<input value={endpoint} onChange={e=>setEndpoint(e.target.value.trim())} placeholder="https://provider.example/v1/chat/completions"/></label>
          <label>模型<input value={model} onChange={e=>setModel(e.target.value.trim())} placeholder="服务提供方的模型名称"/></label>
          <button onClick={()=>{saveAi();setNotice('地址和模型已保存');}}>保存地址和模型</button>
          <label>API Key<input type="password" autoComplete="off" value={key} onChange={e=>setKey(e.target.value)}/></label>
          <p className="muted">密钥保存在当前用户的应用数据中，未加密，也不会包含在词库导出中。旧版钥匙串密钥需要重新保存一次。</p>
          <div className="row"><button disabled={busy || !key} onClick={()=>void task(async()=>{
            await call('key_save',{endpoint,model,key});saveAi();setKeyRevision(v=>v+1);setKey('');setNotice('密钥已保存到本机');
          })}>保存密钥</button>
          <button disabled={busy || !aiConfigured || keyState!=='已配置（本机）'} onClick={()=>void task(async()=>{
            await call('key_remove',savedAi);setKeyRevision(v=>v+1);setKey('');setNotice('已清除当前地址的密钥');
          })}>清除密钥</button></div>
        </section>
        <PronunciationSettings active={pronunciationLibrary} select={id=>{
          setPronunciationLibrary(id);localStorage.setItem('pronunciationLibrary',id);
        }} autoIpa={autoLemma} setAutoIpa={enabled=>{
          setAutoLemma(enabled);localStorage.setItem('autoDictionaryLemma',enabled?'on':'off');
        }}/>
        <LibraryGlossOrganizer busy={busy} configured={aiConfigured}
          scan={(resume,receive,stopped)=>void task(async()=>{
            if (!resume) setSnapshots(await call<Snapshot[]>('snapshot_create'));
            let cursor = resume ?? '';
            do {
              const batch = await call<{changes:GlossChange[];next:string|null}>('organize_glosses',{...savedAi,cursor});
              if (batch.next && batch.next<=cursor) throw Error('扫描进度无效，请重新开始');
              receive(batch.changes,batch.next);
              cursor = batch.next ?? '';
            } while (cursor && !stopped());
          })}
          apply={(changes,done)=>void task(async()=>{
            const result=await call<{applied:number;conflicts:number}>('gloss_apply',{changes});
            done();
            if (read) setRead(await call<Read>('document',{id:read.document.id}));
            resetSelection();clearGroup();await refresh();
            setNotice(`已写入 ${result.applied} 条释义${result.conflicts ? `，跳过 ${result.conflicts} 条重复释义` : ''}`);
          })}/>
        <section><h2>备份与恢复</h2>
          <p>快照保存在这台设备，包含文章、网页图片、语义库、标签、原文标注和学习笔记。</p>
          <button disabled={busy} onClick={()=>void task(async()=>{
            const items = await call<Snapshot[]>('snapshot_create');
            setSnapshots(items);
            setSnapshotId(items[0]?.id ?? '');
            setConfirmSnapshot(null);setBackupPreview(null);
            setNotice('备份已自动保存');
          })}>导出备份</button>
          <button type="button" disabled={busy} onClick={()=>void task(async()=>{
            if (!window.confirm('把所有义项退回「未归类」？将先创建词库备份。语义组、释义和原文标注都会保留。')) return;
            const items = await call<Snapshot[]>('snapshot_create');
            setSnapshots(items);
            setSnapshotId(items[0]?.id ?? '');
            setConfirmSnapshot(null);setBackupPreview(null);
            const count = await call<number>('sil_reset');
            clearSilProgress();
            setSilFilter('unclassified');
            clearGroup();
            await refresh();
            setNotice(`已清空 ${count} 个语义组的 SIL 分类，所有义项都在「未归类」。`);
          })}>重置 SIL 分类</button>
          {snapshots.length>0 ? <>
            <label>本地备份（{snapshots.length} 份）<select size={Math.max(2,Math.min(6,snapshots.length))}
              value={snapshotId} disabled={busy} onChange={e=>{
                setSnapshotId(e.target.value);setConfirmSnapshot(null);setBackupPreview(null);
              }}>
              {snapshots.map(snapshot=><option key={snapshot.id} value={snapshot.id}>{snapshotLabel(snapshot)}</option>)}
            </select></label>
            <div className="row">
              <button disabled={busy || !selectedSnapshot} onClick={()=>void task(async()=>{
                if (!selectedSnapshot) return;
                const data = await call<unknown>('snapshot_preview',{id:selectedSnapshot.id});
                setBackupPreview({name:snapshotLabel(selectedSnapshot),data:parseBackupPreview(data)});
              })}>查看所选备份</button>
              <button disabled={busy || !selectedSnapshot} onClick={()=>setConfirmSnapshot('restore')}>恢复所选备份</button>
              <button className="delete-document" disabled={busy || !selectedSnapshot} onClick={()=>setConfirmSnapshot('delete')}>删除所选备份</button>
            </div>
          </> : <p className="muted">暂无本地快照。</p>}
          {confirmSnapshot==='restore' && selectedSnapshot && <section className="delete-confirm" aria-label="确认恢复快照">
            <p>恢复到 {snapshotLabel(selectedSnapshot)}？当前文章和词库会被替换。应用会先自动备份当前数据。</p>
            <div className="row">
              <button disabled={busy} onClick={()=>setConfirmSnapshot(null)}>取消</button>
              <button className="delete-document" disabled={busy} onClick={()=>void task(async()=>{
                await call('snapshot_restore',{id:selectedSnapshot.id});
                setConfirmSnapshot(null);setSnapshotId('');setBackupPreview(null);
                setRead(null);setFocus('');setEditing(false);readerScroll.current=0;resetSelection();clearGroup();
                setSilFilter('');setBulkGroups([]);
                await refresh();
                setSnapshots(await call<Snapshot[]>('snapshot_list'));
                setNotice('已恢复快照，恢复前的数据已自动备份');
              })}>确认恢复</button>
            </div>
          </section>}
          {confirmSnapshot==='delete' && selectedSnapshot && <section className="delete-confirm" aria-label="确认删除备份">
            <p>永久删除 {snapshotLabel(selectedSnapshot)} 的备份？</p>
            <div className="row">
              <button disabled={busy} onClick={()=>setConfirmSnapshot(null)}>取消</button>
              <button className="delete-document" disabled={busy} onClick={()=>void task(async()=>{
                setSnapshots(await call<Snapshot[]>('snapshot_delete',{id:selectedSnapshot.id}));
                setConfirmSnapshot(null);setSnapshotId('');setBackupPreview(null);
                setNotice('已删除所选备份');
              })}>确认删除备份</button>
            </div>
          </section>}
          {backupPreview && <BackupPreview key={backupPreview.name} {...backupPreview} close={()=>setBackupPreview(null)}/>}
        </section>
        <section><h2>外部文件</h2>
          <button disabled={busy} onClick={()=>previewFile.current?.click()}>查看导出的 JSON</button>
          <input hidden ref={previewFile} type="file" accept=".json,application/json" onChange={e=>{
            const input=e.currentTarget.files?.[0];e.currentTarget.value='';
            if (input) void task(async()=>{
              const data = parseBackupPreview(JSON.parse(await input.text()));
              setBackupPreview({name:input.name,data});
            });
          }}/>
          <button disabled={busy} onClick={()=>void task(async()=>{
            if (await restoreBackupFile()) {await refresh();setNotice('词汇库已恢复');}
          })}>从 JSON 恢复到空词汇库</button>
          {read && <button disabled={busy} onClick={()=>void task(async()=>{if(await exportFile(read.document.id)) setNotice('已导出原文');})}>导出当前原文</button>}
          {backupPreview && <BackupPreview key={backupPreview.name} {...backupPreview} close={()=>setBackupPreview(null)}/>}
        </section>
      </main>}
    </div>
  </div>;
}

type BackupPreviewData = {
  documents:{id:string;title:string;content:string;source_url?:string|null}[];
  semantic_groups:Group[];
  expressions:{id:string;lemma:string}[];
  senses:(Omit<Sense,'lemma'> & {expression_id:string})[];
  occurrences:Pick<Mark,'id'|'document_id'|'sense_id'|'surface'|'context'|'context_meaning'>[];
};

function parseBackupPreview(value:unknown):BackupPreviewData {
  const data = value as Record<string,unknown> | null;
  const tables = {
    documents:['id','title','content'],semantic_groups:['id','name','description'],
    expressions:['id','lemma'],senses:['id','group_id','expression_id','meaning'],
    occurrences:['id','document_id','sense_id','surface','context']
  };
  if (!data || typeof data.version!=='number' || ![1,2,3,4,5,6,7,8,9].includes(data.version) ||
      !Object.entries(tables).every(([table,columns])=>{
        const rows = data[table];
        return Array.isArray(rows) && rows.every(row=>row && columns.every(key=>typeof row[key]==='string'));
      })) throw Error('请选择 Semantic Reader 导出的完整词汇库 JSON');
  const preview = value as BackupPreviewData;
  if (!Array.isArray(data.pdf_pages)) return preview;
  const pages = new Map<string,{page:number;text:string}[]>();
  for (const row of data.pdf_pages) {
    if (row && typeof row.document_id==='string' && typeof row.page==='number' && typeof row.text==='string') {
      const items = pages.get(row.document_id) ?? [];
      items.push({page:row.page,text:row.text});
      pages.set(row.document_id,items);
    }
  }
  return {...preview,documents:preview.documents.map(doc=>{
    const indexed = pages.get(doc.id);
    return indexed ? {...doc,content:indexed.sort((a,b)=>a.page-b.page).map(page=>page.text).join('\n\n')} : doc;
  })};
}

function BackupPreview({name,data,close}: {name:string;data:BackupPreviewData;close:()=>void}) {
  const panel = useRef<HTMLElement>(null);
  useEffect(()=>{panel.current?.scrollIntoView({block:'nearest'});},[data]);
  const [view,setView] = useState<'articles'|'groups'>('articles');
  const [query,setQuery] = useState('');
  const [selected,setSelected] = useState('');
  const documents = useMemo(()=>new Map(data.documents.map(doc=>[doc.id,doc])),[data]);
  const expressions = useMemo(()=>new Map(data.expressions.map(item=>[item.id,item.lemma])),[data]);
  const doc = view==='articles' ? documents.get(selected) : undefined;
  const group = view==='groups' ? data.semantic_groups.find(item=>item.id===selected) : undefined;
  const senses = group ? data.senses.filter(sense=>sense.group_id===group.id) : [];
  const senseIds = new Set(senses.map(sense=>sense.id));
  const marks = (doc || group) ? data.occurrences.filter(mark=>doc ? mark.document_id===doc.id : senseIds.has(mark.sense_id)) : [];
  const items = (view==='articles' ? data.documents.map(item=>({id:item.id,name:item.title}))
    : data.semantic_groups).filter(item=>item.name.toLowerCase().includes(query.trim().toLowerCase()));
  return <section ref={panel} className="action" aria-label="备份预览">
    <div className="row"><b>{name}</b><button type="button" onClick={close}>关闭预览</button></div>
    <p>{data.documents.length} 篇文章 · {data.semantic_groups.length} 个语义组 · {data.senses.length} 个义项 · {data.occurrences.length} 条原文链接</p>
    <div className="row">
      <button type="button" aria-pressed={view==='articles'} onClick={()=>{setView('articles');setSelected('');setQuery('');}}>文章</button>
      <button type="button" aria-pressed={view==='groups'} onClick={()=>{setView('groups');setSelected('');setQuery('');}}>语义组</button>
    </div>
    <label>{view==='articles' ? '搜索文章标题' : '搜索语义组名称'}<input value={query} onChange={e=>setQuery(e.target.value)}/></label>
    <label>{view==='articles' ? '选择文章' : '选择语义组'}<select value={items.some(item=>item.id===selected) ? selected : ''}
      onChange={e=>setSelected(e.target.value)}>
      <option value="">{items.length ? '请选择' : '没有匹配结果'}</option>
      {items.map(item=><option key={item.id} value={item.id}>{item.name}</option>)}
    </select></label>
    {doc && <><h3>{doc.title}</h3>{typeof doc.source_url==='string' && <p className="muted">{doc.source_url}</p>}
      <pre className="backup-text">{doc.content}</pre></>}
    {group && <><h3>{group.name}</h3><p>{group.description}</p>
      <ul>{senses.map(sense=><li key={sense.id}>{expressions.get(sense.expression_id)} · {sense.meaning}</li>)}</ul></>}
    {(doc || group) && <details><summary>原文链接（{marks.length}）</summary>
      <ul>{marks.map(mark=><li key={mark.id}><b>{mark.surface}</b> · {mark.context}
        {typeof mark.context_meaning==='string' && <p>本句释义 · {mark.context_meaning}</p>}
        {group && <> · <button className="quiet" type="button" onClick={()=>{
          setView('articles');setQuery('');setSelected(mark.document_id);
        }}>{documents.get(mark.document_id)?.title ?? '查看文章'}</button></>}
      </li>)}</ul>
    </details>}
  </section>;
}

function HelpPage() {
  return <main className="page help-page">
    <p className="eyebrow">SEMANTIC READER</p>
    <h1>使用说明</h1>
    <p>导入一篇英文文章，从原文收录词语和短语。以后点开标注，就能回看它在这段话里的意思和同组词项。</p>

    <section>
      <h2>1. 导入和阅读</h2>
      <p>在「文本库」导入 Markdown、TXT、PDF 或 EPUB 文件。EPUB 按章节顺序导入正文。PDF 最大 100 MB，保留原版式并逐页建立文字索引。没有文字层的页面会在本地做英文 OCR。也可以粘贴网页地址后导入。可以按标题或网址搜索。打开文本后，点击阅读器顶部的「目录与书签」展开左栏。</p>
      <p>点击标题行的铅笔可以修改正文。未改动的标注会随文字移动，改到标注时会在保存前提示。编辑时沿用阅读版式，网页图片和来源链接会保留。</p>
      <p>按 <kbd>⌘F</kbd>（Mac）或 <kbd>Ctrl+F</kbd>（Windows）搜索当前文章，也可以点击标题行的放大镜。<kbd>Enter</kbd> 跳到下一处，<kbd>Shift+Enter</kbd> 跳到上一处，按 <kbd>Esc</kbd> 或点「关闭」收起搜索。</p>
    </section>

    <section>
      <h2>2. 查词和收录</h2>
      <ol>
        <li>双击新词。配置 AI 后，右栏会自动请求语境释义、学习笔记和语义组建议。拖选短语后，可以点「请求 AI 建议」。</li>
        <li>核对原形、释义和归组。同一意思可复用已有义项，只新增原文标注。不同意思可选「填写新义项」。</li>
        <li>点「确认收录」。以后点击原文里的标注，就能打开已保存的笔记。</li>
      </ol>
      <p>「词源与速记」是 AI 生成的学习提示，旧笔记可点「补充学习笔记」。匹配到已有组时，右栏会列出同组已收录词项。点「比较用法」，可用 AI 比较最多 12 个词项的侧重点、语气和搭配。</p>
    </section>

    <section>
      <h2>3. 配置 AI</h2>
      <p>在「设置」填写服务商提供的完整 <code>/chat/completions</code> 地址、模型名和 API Key，再点「保存密钥」。密钥会保存在当前用户的应用数据中，未加密。</p>
      <p>AI 请求会发送所选文字、附近语境和少量候选语义组。建议新建组时，应用会先用 AI 给出的检索词补查本地词库。有新候选才追加一次归组请求。可在设置里关闭双击自动请求，也可单独关闭词典缺少音标时的 AI 补充。手动填写和收录无需配置 AI。</p>
    </section>

    <section>
      <h2>4. 安装离线词典和播放读音</h2>
      <p>在「设置 → 发音词典」导入含 MDX 和配套 MDD 的文件夹，然后选择当前发音库。阅读器右栏会直接显示英美音标。</p>
      <p>点扬声器优先播放词典录音。缺少录音时，可用 Kokoro 离线合成。</p>
    </section>

    <section>
      <h2>5. 管理语义组</h2>
      <p>在阅读器点击原文标注，可选择「删除此处标注」。在语义库的组详情中选择「删除义项」，会同时删除该义项和它在所有文章中的原文链接。</p>
      <p>在「语义库」搜索组名、词语或义项，也可以按 SIL 分类筛选。打开组后可编辑说明、核心词和 SIL 分类，也可以点「整理语义组」移动义项、拆组或合并组。在左栏逐级打开 SIL 分类，右侧可以用 AI 整理当前类的直属义项。点建议可预填表单，确认后才会修改词库。拆分时可点「填入所选核心词」再编辑，留空则按拆出的义项生成核心词。点原文出现记录可跳回文章中的标注。</p>
    </section>

    <section>
      <h2>6. 备份和恢复</h2>
      <p>在「设置 → 备份与恢复」点「导出备份」，应用会自动保存带时间戳的 JSON。可以在列表中按时间查看和恢复备份，恢复前会自动备份当前数据。「外部文件」可以查看以前导出的 JSON，或将其恢复到空词汇库。API Key 和离线词典需要在新设备上单独配置。</p>
    </section>
  </main>;
}

function Reclassify({groups,selectedMarks,reclassMeaning,setReclassMeaning,busy,apply}: {
  groups: Group[]; selectedMarks: string[]; reclassMeaning: string;
  setReclassMeaning: (s:string)=>void; busy:boolean; apply:(id:string)=>void;
}) {
  const [groupId,setGroupId] = useState('');
  return <form className="action" onSubmit={e=>{e.preventDefault();apply(groupId);}}>
    <h3>仅重新分类所选出现记录</h3>
    <GroupPicker groups={groups} value={groupId} change={setGroupId}/>
    <label>新的语境义项<input required maxLength={1200} value={reclassMeaning} onChange={e=>setReclassMeaning(e.target.value)}/></label>
    <button disabled={busy || !groupId || !selectedMarks.length}>重新分类 {selectedMarks.length} 条记录</button>
  </form>;
}

ReactDOM.createRoot(document.getElementById('root')!).render(<App/>);

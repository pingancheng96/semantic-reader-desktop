import {playKokoro, playKokoroText} from './kokoro-client.js';
import {React, call, choosePronunciationFolder, installKokoro, type KokoroVariant, type KokoroStatus} from './runtime.js';
const {useEffect,useRef,useState} = React;

type Estimate = {ipa_uk:string|null;ipa_us:string|null};
type Accent = 'UK'|'US';
type Sound = {word:string;pos:string;region:Accent;ipa:string;audio:string|null};
type Library = {id:string;name:string;files:string[];size:number};
type Preview = Omit<Library,'id'> & {folder:string};
const positions:Record<string,string> = {noun:'名词',verb:'动词',adjective:'形容词',adverb:'副词',
  pronoun:'代词',preposition:'介词',conjunction:'连词',interjection:'感叹词'};

function sizeLabel(bytes:number) {
  return new Intl.NumberFormat('zh-CN',{maximumFractionDigits:2}).format(bytes/1024**3)+' GiB';
}

const models = {fp32:'原版 · 327 MB',int8:'8 位量化版 · 93 MB'} as const;

function chosenModel():KokoroVariant {
  return localStorage.getItem('kokoroVariant')==='int8' ? 'int8' : 'fp32';
}

export function PronunciationSettings({active,select,autoIpa,setAutoIpa}: {active:string;select:(id:string)=>void;autoIpa:boolean;setAutoIpa:(enabled:boolean)=>void}) {
  const [libraries,setLibraries] = useState<Library[]>([]);
  const [preview,setPreview] = useState<Preview|null>(null);
  const [removing,setRemoving] = useState('');
  const [busy,setBusy] = useState(false);
  const [status,setStatus] = useState('');
  const [installed,setInstalled] = useState<KokoroStatus>({fp32:false,int8:false});
  const [variant,setVariant] = useState(chosenModel);
  const [download,setDownload] = useState<{bytes:number;total:number}|null>(null);
  const demo = useRef<AudioBufferSourceNode|null>(null);
  const demoRequest = useRef(0);
  useEffect(()=>()=>{demoRequest.current++;demo.current?.stop();},[]);
  async function refresh() {
    const [list,kokoro] = await Promise.all([
      call<Library[]>('pronunciation_list'),call<KokoroStatus>('kokoro_status')
    ]);
    setLibraries(list);setInstalled(kokoro);
  }
  useEffect(()=>{void refresh().catch(error=>setStatus(String(error)));},[]);
  async function task(action:()=>Promise<void>) {
    setBusy(true);setStatus('');
    try {await action();}
    catch(error) {setStatus(String(error));}
    finally {setBusy(false);}
  }
  return <section>
    <h2>发音词典</h2>
    <p className="muted">选择包含一个 MDX 和配套 MDD 分卷的文件夹。导入成功后，可以删除原文件。</p>
    <label>当前发音库<select value={active} disabled={busy} onChange={event=>select(event.target.value)}>
      <option value="">未选择发音词典</option>
      {libraries.map(item=><option value={item.id} key={item.id}>{item.name}</option>)}
    </select></label>
    <button disabled={busy} onClick={()=>void task(async()=>{
      setPreview(await choosePronunciationFolder<Preview>());
    })}>选择词典文件夹</button>
    {preview && <div>
      <p>{preview.name} · {preview.files.length} 个文件 · 约 {sizeLabel(preview.size)}</p>
      <div className="row"><button className="primary" disabled={busy} onClick={()=>void task(async()=>{
        setStatus('正在复制并检查全部词典文件…');
        const library = await call<Library>('pronunciation_install',{folder:preview.folder});
        setLibraries(previous=>[...previous,library]);select(library.id);setPreview(null);setStatus('词典已导入');
      })}>复制并导入</button><button disabled={busy} onClick={()=>setPreview(null)}>取消</button></div>
    </div>}
    {libraries.map(item=><div className="row" key={item.id}>
      <span>{item.name} · {sizeLabel(item.size)}</span>
      {removing===item.id ? <>
        <button disabled={busy} onClick={()=>void task(async()=>{
          await call('pronunciation_remove',{id:item.id});
          if (active===item.id) select('');
          setLibraries(previous=>previous.filter(library=>library.id!==item.id));
          setRemoving('');setStatus('已删除词典副本');
        })}>确认删除</button><button disabled={busy} onClick={()=>setRemoving('')}>取消</button>
      </> : <button disabled={busy} onClick={()=>setRemoving(item.id)}>删除</button>}
    </div>)}
    <label className="check"><input type="checkbox" checked={autoIpa}
      onChange={event=>setAutoIpa(event.target.checked)}/>缺少音标时允许 AI 补充（会发送高亮文字和附近语境）</label>
    <h3>Kokoro 离线发音</h3>
    <p className="muted">词典缺录音时，Kokoro 在本机朗读高亮文字。两个版本使用相同的英美声音。</p>
    <label>当前 Kokoro 模型<select disabled={busy} value={variant} onChange={event=>{
      const model = event.target.value==='int8' ? 'int8' : 'fp32';
      setVariant(model);localStorage.setItem('kokoroVariant',model);
    }}>
      <option value="fp32">{models.fp32}</option>
      <option value="int8">{models.int8}</option>
    </select></label>
    {!installed[variant] && <button disabled={busy} onClick={()=>void task(async()=>{
      setStatus('正在下载 Kokoro…');
      try {
        await installKokoro(variant,setDownload);
        setInstalled(previous=>({...previous,[variant]:true}));setStatus('Kokoro 已安装');
      } finally {setDownload(null);}
    })}>下载所选模型</button>}
    {download && <progress value={download.bytes} max={download.total} aria-label="Kokoro 下载进度"/>}
    {installed[variant] && <div className="row">
      {([
        ['英式 record（名词）','/ˈrekɔːd/','en-GB'],['英式 record（动词）','/rɪˈkɔːd/','en-GB'],
        ['美式 tomato','/təˈmeɪtoʊ/','en-US']
      ] as const).map(([label,ipa,locale])=><button type="button" disabled={busy} key={label} onClick={()=>void task(async()=>{
        setStatus('正在本地合成…');
        demo.current?.stop();
        const request = ++demoRequest.current;
        const start = performance.now();
        demo.current = await playKokoro(variant,ipa,locale,()=>request===demoRequest.current) ?? null;
        setStatus(`试听已播放 · 等待 ${((performance.now()-start)/1000).toFixed(2)} 秒`);
      })}>试听{label}</button>)}
    </div>}
    {status && <p role="status">{status}</p>}
  </section>;
}

export function PronunciationPanel({id,word,pos,context,ai}: {
  id:string;word:string;pos?:string|null;context?:string;ai?:{endpoint:string;model:string};
}) {
  const [offline,setOffline] = useState(false);
  const variant = chosenModel();
  useEffect(()=>{void call<KokoroStatus>('kokoro_status').then(status=>setOffline(status[variant]),()=>{});},[variant]);
  const [sounds,setSounds] = useState<Sound[]|null>(null);
  const [choice,setChoice] = useState('');
  const [error,setError] = useState('');
  const [loading,setLoading] = useState<Accent|null>(null);
  const audio = useRef<HTMLAudioElement|null>(null);
  const localAudio = useRef<AudioBufferSourceNode|null>(null);
  const generation = useRef(0);
  useEffect(()=>{
    let current = true;
    setSounds(null);setError('');setChoice('');setLoading(null);
    if (!id) setSounds([]);
    else void call<Sound[]>('pronunciation_lookup',{id,word}).then(
      value=>{if(current)setSounds(value.filter(sound=>sound.word===word.trim().toLowerCase()));},
      reason=>{if(current){setSounds([]);setError(String(reason));}}
    );
    return ()=>{
      current=false;generation.current++;
      audio.current?.pause();audio.current=null;localAudio.current?.stop();localAudio.current=null;
    };
  },[id,word]);
  const allPositions = [...new Set((sounds ?? []).map(sound=>sound.pos).filter(Boolean))];
  const hinted = allPositions.find(label=>label===pos || positions[label]===pos);
  const selected = allPositions.includes(choice) ? choice : hinted || allPositions[0] || '';
  const matching = sounds?.filter(sound=>sound.pos===selected || !sound.pos) ?? [];
  const uk = matching.find(sound=>sound.region==='UK');
  const us = matching.find(sound=>sound.region==='US');
  const endpoint = ai?.endpoint;
  const model = ai?.model;
  const role = selected || pos;
  const needsEstimate = Boolean(endpoint && model && (!uk?.ipa || !us?.ipa));
  const [suggestion,setSuggestion] = useState<{word:string;context?:string;role?:string|null;value:Estimate|null}|null>(null);
  const requestMatches = suggestion?.word===word && suggestion.context===context && suggestion.role===role;
  const estimate = requestMatches && needsEstimate ? suggestion.value : null;
  useEffect(()=>{
    setSuggestion(null);
    if (!sounds || !needsEstimate) return;
    let active = true;
    const timer = window.setTimeout(()=>{
      void call<Estimate>('pronunciation_estimate',{endpoint,model,word,context,pos:role}).then(
        value=>{if (active) setSuggestion({word,context,role,value});},
        error=>{if (active) {setSuggestion({word,context,role,value:null});setError(String(error));}}
      );
    },250);
    return ()=>{active=false;window.clearTimeout(timer);};
  },[sounds,needsEstimate,endpoint,model,word,context,role]);
  async function play(region:Accent,sound:Sound|undefined,ipa:string|undefined|null) {
    const request = ++generation.current;
    setLoading(region);setError('');audio.current?.pause();localAudio.current?.stop();localAudio.current=null;
    try {
      const locale = region==='UK' ? 'en-GB' : 'en-US';
      if (offline && !sound?.audio) {
        const current = ()=>request===generation.current;
        const source = ipa ? await playKokoro(variant,ipa,locale,current) : await playKokoroText(variant,word,locale,current);
        if (source && request===generation.current) localAudio.current=source;
        return;
      }
      if (!sound?.audio) return;
      const data = await call<string>('pronunciation_audio',{id,key:sound.audio});
      if (request!==generation.current) return;
      const player = new Audio('data:audio/mpeg;base64,'+data);
      audio.current=player;
      await player.play();
    } catch(reason) {
      if (request===generation.current) setError(String(reason));
    } finally {
      if (request===generation.current) setLoading(null);
    }
  }
  function row(region:Accent,sound:Sound|undefined,estimated?:string|null) {
    const label = region==='UK' ? '英式' : '美式';
    const usesEstimate = !sound?.ipa && Boolean(estimated);
    const ipa = sound?.ipa || estimated;
    const recording = usesEstimate ? undefined : sound;
    const waiting = !sound?.ipa && needsEstimate && !requestMatches;
    return <div className="pronunciation-row">
      <span>{label}</span><span lang="en">{ipa || '—'}</span>
      <button type="button" aria-label={`播放${label}发音`} disabled={!sounds || waiting || Boolean(loading) || (!recording?.audio && !offline)}
        onClick={()=>void play(region,recording,ipa)}>{loading===region ? '载入中…' : '🔊'}</button>
      {waiting ? <small>正在请求 AI 音标…</small> : !recording?.audio && <small>{usesEstimate ? 'AI 建议 · ' : ''}{offline ? 'Kokoro 合成' : '需下载 Kokoro'}</small>}
    </div>;
  }
  return <div className="dictionary-panel pronunciation-panel" aria-label="音标与发音">
    {allPositions.length>1 && <label>发音词性<select value={selected} onChange={event=>setChoice(event.target.value)}>
      {allPositions.map(label=><option key={label} value={label}>{positions[label] || label}</option>)}
    </select></label>}
    {row('UK',uk,estimate?.ipa_uk)}{row('US',us,estimate?.ipa_us)}
    {!sounds && <small>正在查询发音词典…</small>}
    {error && <p role="status">{error}</p>}
  </div>;
}

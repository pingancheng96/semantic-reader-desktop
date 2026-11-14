import {React,call} from './runtime.js';

type Info = {current:string; releases:string};
type Release = {available:boolean; version:string; notes:string; download:string|null};

export function UpdateSettings() {
  const [info,setInfo] = React.useState<Info|null>(null);
  const [latest,setLatest] = React.useState<Release|null>(null);
  const [checking,setChecking] = React.useState(false);
  const [message,setMessage] = React.useState('');
  React.useEffect(()=>{
    let active = true;
    void call<Info>('update_info').then(result=>{if (active) setInfo(result);})
      .catch(error=>{if (active) setMessage(String(error));});
    return ()=>{active=false;};
  },[]);
  async function check() {
    setMessage('');
    setLatest(null);
    setChecking(true);
    try { setLatest(await call<Release>('update_check')); }
    catch (error) { setMessage(String(error)); }
    finally { setChecking(false); }
  }
  function open(url:string) {
    void call('update_open_link',{url}).catch(error=>setMessage(String(error)));
  }
  return <section aria-label="软件更新">
    <h2>软件更新</h2>
    {info && <p>当前版本 {info.current}</p>}
    <div className="row">
      <button type="button" disabled={checking || !info} onClick={()=>void check()}>
        {checking ? '正在检查…' : '检查更新'}
      </button>
      {latest?.available && latest.download && <button className="primary" type="button"
        onClick={()=>open(latest.download!)}>下载新版</button>}
      {info && <button type="button" onClick={()=>open(info.releases)}>GitHub 发布页</button>}
    </div>
    <div aria-live="polite">
      {message && <p>{message}</p>}
      {latest && <p>{latest.available ? `发现新版 ${latest.version}` : '当前版本无需更新。'}</p>}
      {latest?.available && !latest.download && <p>发布中未找到适合此电脑的安装包，可查看发布页。</p>}
    </div>
    {latest?.available && latest.notes && <details><summary>更新说明</summary>
      <p className="update-notes">{latest.notes}</p></details>}
  </section>;
}

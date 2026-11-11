import {React} from './runtime.js';

export function useArticleSearchShortcut(enabled:boolean, open:()=>void) {
  const handler = React.useRef(open);
  handler.current = open;
  React.useEffect(()=>{
    if (!enabled) return;
    const openSearch = () => handler.current();
    const find = (event:KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && !event.altKey && !event.shiftKey &&
          (event.code === 'KeyF' || event.key.toLowerCase() === 'f')) {
        event.preventDefault();
        openSearch();
      }
    };
    window.addEventListener('keydown',find,true);
    window.addEventListener('semantic-reader-find',openSearch);
    return ()=>{
      window.removeEventListener('keydown',find,true);
      window.removeEventListener('semantic-reader-find',openSearch);
    };
  },[enabled]);
}

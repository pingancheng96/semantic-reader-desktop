import {languageKey} from './languages.js';
import type {ReactNode} from 'react';
import {React} from './runtime.js';

export function readReaderPreference(key:string) {
  try { return localStorage.getItem(languageKey(key)) ?? ''; }
  catch { return ''; }
}

export function saveReaderPreference(key:string,value:string) {
  try { localStorage.setItem(languageKey(key),value); }
  catch { /* Reading remains available if local storage is unavailable. */ }
}

export function useReaderNavigation() {
  const [open,setOpen] = React.useState(()=>readReaderPreference('readerNavigationOpen')==='yes');
  const [section,setSection] = React.useState<'contents'|'bookmarks'>('contents');
  function toggle() {
    saveReaderPreference('readerNavigationOpen',open ? 'no' : 'yes');
    setOpen(!open);
  }
  function showBookmarks() {
    setSection('bookmarks');
    saveReaderPreference('readerNavigationOpen','yes');
    setOpen(true);
  }
  return {open,section,setSection,toggle,showBookmarks};
}

export type ReaderNavigationState = ReturnType<typeof useReaderNavigation>;

export function ReaderNavigation({navigation,contents,bookmarks}:{
  navigation:ReaderNavigationState; contents:ReactNode; bookmarks:ReactNode;
}) {
  return <aside id="reader-navigation" className="pdf-outline" aria-label="目录与书签">
    <div className="reader-navigation-tabs" role="group" aria-label="阅读导航">
      <button type="button" aria-pressed={navigation.section==='contents'}
        onClick={()=>navigation.setSection('contents')}>目录</button>
      <button type="button" aria-pressed={navigation.section==='bookmarks'}
        onClick={()=>navigation.setSection('bookmarks')}>书签</button>
    </div>
    {navigation.section==='contents' ? contents : bookmarks}
  </aside>;
}

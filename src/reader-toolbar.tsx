import type {ReactNode} from 'react';
import {React, ReactDOM} from './runtime.js';

export function ReaderToolbar({target,bookmark,previous,next,zoom,changeZoom,resetLabel='重置缩放',pageIndicator,children,disabled,previousDisabled,nextDisabled}:{
  target:HTMLElement|null;
  bookmark:{active?:boolean;disabled?:boolean;toggle:()=>void};
  previous:()=>void;
  next:()=>void;
  zoom:number;
  changeZoom:(value:number)=>void;
  resetLabel?:string;
  pageIndicator?:ReactNode;
  children?:ReactNode;
  disabled?:boolean;
  previousDisabled?:boolean;
  nextDisabled?:boolean;
}) {
  return target && ReactDOM.createPortal(<nav className="reader-toolbar" aria-label="阅读工具">
    <button type="button" disabled={bookmark.disabled} aria-pressed={bookmark.active}
      aria-label={bookmark.active ? '删除当前位置书签' : '添加当前位置书签'}
      onClick={bookmark.toggle}>{bookmark.active ? '★' : '☆'}</button>
    <button type="button" disabled={previousDisabled ?? disabled} aria-label="上一页" onClick={previous}>‹</button>
    {pageIndicator}
    <button type="button" disabled={nextDisabled ?? disabled} aria-label="下一页" onClick={next}>›</button>
    <div className="reader-zoom" role="group" aria-label="缩放">
      <button type="button" disabled={disabled || zoom<=.5} aria-label="缩小" onClick={()=>changeZoom(zoom-.25)}>−</button>
      <button type="button" className="reader-zoom-reset" disabled={disabled} aria-label={resetLabel}
        title={resetLabel} onClick={()=>changeZoom(1)}>{Math.round(zoom*100)}%</button>
      <button type="button" disabled={disabled || zoom>=3} aria-label="放大" onClick={()=>changeZoom(zoom+.25)}>＋</button>
    </div>
    {children}
  </nav>,target);
}

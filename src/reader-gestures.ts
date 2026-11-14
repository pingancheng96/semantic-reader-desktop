type Point = {x:number;y:number};
export function readerGestures(options:{
  zoom:()=>number;
  update:(zoom:number,point:Point)=>void;
  turn:(direction:number)=>void;
  width:()=>number;
  canTurn?:()=>boolean;
  threshold?:number;
  shiftScroll?:boolean;
  pinch?:boolean;
  start?:()=>void;
  end?:()=>void;
}) {
  let base:number|null = null;
  let time = -Infinity, distance = 0, turned = false;
  const handled = new WeakSet<Event>();
  function bind(target:EventTarget, signal:AbortSignal, accepts:(event:Event)=>boolean) {
    const listenerOptions = {capture:true,passive:false,signal};
    function listen(name:string, action:(event:MouseEvent & {scale:number})=>void, continuing=false) {
      target.addEventListener(name,event=>{
        if (options.pinch===false && name.startsWith('gesture')) {
          if (accepts(event)) event.preventDefault();
          return;
        }
        if (handled.has(event) || (!(continuing && base!==null) && !accepts(event))) return;
        handled.add(event);
        action(event as MouseEvent & {scale:number});
      },listenerOptions);
    }
    listen('wheel',event=>{
      const wheel=event as unknown as WheelEvent;
      if (wheel.ctrlKey) {
        wheel.preventDefault();
        if (options.pinch!==false && base===null) options.update(options.zoom()*Math.exp(-wheel.deltaY*.012),{x:wheel.clientX,y:wheel.clientY});
        return;
      }
      if (options.canTurn?.()===false) return;
      const shifted=options.shiftScroll && wheel.shiftKey;
      const horizontal=shifted && !wheel.deltaX ? wheel.deltaY : wheel.deltaX;
      if (!horizontal || Math.abs(horizontal)<=Math.abs(shifted ? 0 : wheel.deltaY)*1.25) return;
      wheel.preventDefault();
      if (wheel.timeStamp-time>200) {distance=0;turned=false;}
      time=wheel.timeStamp;
      if (turned) return;
      distance+=horizontal*(wheel.deltaMode===1 ? 16 : wheel.deltaMode===2 ? options.width() : 1);
      if (Math.abs(distance)<(options.threshold ?? 80)) return;
      turned=true;
      options.turn(Math.sign(distance));
    });
    listen('gesturestart',event=>{
      event.preventDefault();
      base=options.zoom();
      options.start?.();
    });
    listen('gesturechange',event=>{
      if (!Number.isFinite(event.scale) || event.scale<=0) return;
      event.preventDefault();
      if (base===null) {base=options.zoom();options.start?.();}
      options.update(base*event.scale,{x:event.clientX,y:event.clientY});
    },true);
    listen('gestureend',event=>{
      if (base===null) return;
      event.preventDefault();base=null;
      options.end?.();
    },true);
  }
  return {bind, reset:()=>{base=null;options.end?.();}};
}

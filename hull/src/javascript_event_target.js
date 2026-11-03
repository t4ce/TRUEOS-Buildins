// Standard DOM event state, shared by Node wrappers and the native window.
// This file is inserted inside javascript_dom.js's private bootstrap closure.
const eventTargets = new WeakMap(), eventStates = new WeakMap();
const eventApply = Reflect.apply, eventNow = globalThis.performance?.now?.bind(globalThis.performance);
const eventDefine = Object.defineProperty;
let eventWindow = null, documentReadyState = 'loading';
function eventState(event) {
    const result = eventStates.get(event);
    if (!result) throw new TypeError('Expected an Event');
    return result;
}
function eventTarget(target) {
    const result = eventTargets.get(target);
    if (!result) throw new TypeError('Illegal EventTarget invocation');
    return result;
}
class Event {
    constructor(type, options = {}) {
        if (!arguments.length) throw new TypeError('Expected an event type');
        const value = {type:string(type), bubbles:!!options?.bubbles, cancelable:!!options?.cancelable,
            composed:!!options?.composed, target:null, current:null, phase:0, dispatching:false,
            stopped:false, immediate:false, canceled:false, passive:false, path:[], trusted:false,
            timestamp:eventNow ? eventNow() : 0, detail:null};
        eventStates.set(this, value);
        eventDefine(this, 'isTrusted', {get:() => value.trusted, enumerable:true});
    }
    get type() { return eventState(this).type; }
    get bubbles() { return eventState(this).bubbles; }
    get cancelable() { return eventState(this).cancelable; }
    get composed() { return eventState(this).composed; }
    get target() { return eventState(this).target; }
    get srcElement() { return this.target; }
    get currentTarget() { return eventState(this).current; }
    get eventPhase() { return eventState(this).phase; }
    get defaultPrevented() { return eventState(this).canceled; }
    get timeStamp() { return eventState(this).timestamp; }
    get cancelBubble() { return eventState(this).stopped; }
    set cancelBubble(value) { if (value) this.stopPropagation(); }
    get returnValue() { return !eventState(this).canceled; }
    set returnValue(value) { if (!value) this.preventDefault(); }
    stopPropagation() { eventState(this).stopped = true; }
    stopImmediatePropagation() { const s=eventState(this); s.stopped=true; s.immediate=true; }
    preventDefault() { const s=eventState(this); if (s.cancelable && !s.passive) s.canceled=true; }
    composedPath() { return eventState(this).path.slice(); }
    initEvent(type, bubbles=false, cancelable=false) {
        const s=eventState(this); if(s.dispatching) return;
        s.type=string(type);s.bubbles=!!bubbles;s.cancelable=!!cancelable;
        s.stopped=false;s.immediate=false;s.canceled=false;s.trusted=false;s.target=null;
    }
}
class CustomEvent extends Event {
    constructor(type, options={}) {
        if (!arguments.length) throw new TypeError('Expected an event type');
        super(type, options); eventState(this).detail=options?.detail ?? null;
    }
    get detail() { return eventState(this).detail; }
    initCustomEvent(type, bubbles=false, cancelable=false, detail=null) {
        if(eventState(this).dispatching)return;
        this.initEvent(type,bubbles,cancelable);eventState(this).detail=detail;
    }
}
function registerEventTarget(target, parent=() => null) {
    eventTargets.set(target, {listeners:new Map(), parent});
}
function listenerOptions(options) {
    if (options === null || typeof options !== 'object' && typeof options !== 'function')
        return {capture:!!options,once:false,passive:undefined};
    const capture=!!options.capture, once=!!options.once, passive=options.passive, signal=options.signal;
    if (signal != null) throw new DOMException('AbortSignal listener options are not implemented', 'NotSupportedError');
    return {capture,once,passive:passive===undefined ? undefined : !!passive};
}
function defaultPassive(type,target) {
    if(!['touchstart','touchmove','wheel','mousewheel'].includes(type))return false;
    if(target===eventWindow)return true;
    const node=states.get(target);
    if(node?.type===9)return true;
    return node?.type===1 && (node.id===get(document,'documentElement') || node.id===get(document,'body'));
}
function removeListener(target, type, callback, capture) {
    const list=eventTarget(target).listeners.get(type);
    if(!list)return;
    for(const entry of list)if(entry.callback===callback && entry.capture===capture)entry.removed=true;
    eventTarget(target).listeners.set(type,list.filter(entry=>!entry.removed));
}
class EventTarget {
    constructor() { registerEventTarget(this); }
    addEventListener(type, callback, options=false) {
        if(arguments.length<2)throw new TypeError('Expected event type and callback');
        const target=eventTarget(this), name=string(type), flags=listenerOptions(options);
        if(flags.passive===undefined)flags.passive=defaultPassive(name,this);
        if(callback==null)return;
        if(typeof callback!=='function' && typeof callback!=='object')throw new TypeError('Expected a callback or listener object');
        const list=target.listeners.get(name)||[];
        if(!list.some(entry=>entry.callback===callback && entry.capture===flags.capture)) {
            list.push({callback,...flags,removed:false});target.listeners.set(name,list);
        }
    }
    removeEventListener(type, callback, options=false) {
        if(arguments.length<2)throw new TypeError('Expected event type and callback');
        const capture=options!==null && (typeof options==='object'||typeof options==='function') ? !!options.capture : !!options;
        removeListener(this,string(type),callback,capture);
    }
    dispatchEvent(event) { return dispatchEventAt(this,event,false); }
}
function dispatchEventAt(target,event,trusted) {
    const state=eventState(event);eventTarget(target);
    if(state.dispatching)throw new DOMException('Event is already dispatching','InvalidStateError');
    state.trusted=trusted;state.target=target;state.dispatching=true;
    const path=[target];
    for(let parent=eventTarget(target).parent();parent;parent=eventTarget(parent).parent())path.push(parent);
    state.path=path;
    function invoke(current,capture,phase) {
        state.current=current;state.phase=phase;
        const snapshot=(eventTarget(current).listeners.get(state.type)||[]).slice();
        for(const entry of snapshot) {
            if(state.immediate)break;
            if(entry.removed || entry.capture!==capture)continue;
            if(entry.once)removeListener(current,state.type,entry.callback,entry.capture);
            state.passive=entry.passive;
            try {
                if(typeof entry.callback==='function')eventApply(entry.callback,current,[event]);
                else eventApply(entry.callback.handleEvent,entry.callback,[event]);
            } catch(error) {
                // Event-listener exceptions are reported, not propagated to the
                // dispatchEvent caller, and do not stop the remaining listeners.
                try { globalThis.console?.error(String(error)); } catch(_) {}
            } finally { state.passive=false; }
        }
    }
    try {
        for(let i=path.length-1;i>0 && !state.stopped;i--)invoke(path[i],true,1);
        if(!state.stopped) { invoke(target,true,2);invoke(target,false,2); }
        if(state.bubbles)for(let i=1;i<path.length && !state.stopped;i++)invoke(path[i],false,3);
        return !state.canceled;
    } finally {
        state.current=null;state.phase=0;state.dispatching=false;state.stopped=false;state.immediate=false;state.path=[];
    }
}
for(const [name,value] of Object.entries({NONE:0,CAPTURING_PHASE:1,AT_TARGET:2,BUBBLING_PHASE:3})) {
    Object.defineProperty(Event,name,{value,enumerable:true});
    Object.defineProperty(Event.prototype,name,{value,enumerable:true});
}
function dispatchDomRecord(record) {
    if(receiveMedia(record))return true;
    if(eventWindow && record.source==='winit-window-state') {
        if(record.kind==='resize')dispatchEventAt(eventWindow,new Event('resize'),true);
        else if(record.kind==='focus' && typeof record.focused==='boolean')
            dispatchEventAt(eventWindow,new Event(record.focused ? 'focus' : 'blur'),true);
    }
    if(record.kind!=='__hull_dom_ready')return false;
    if(documentReadyState!=='loading')return true;
    documentReadyState='interactive';
    dispatchEventAt(document,new Event('readystatechange'),true);
    dispatchEventAt(document,new Event('DOMContentLoaded',{bubbles:true}),true);
    return true;
}

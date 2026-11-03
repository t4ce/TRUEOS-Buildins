// Raw native input endpoint, with an optional DOM lifecycle receiver supplied
// by the enclosing DOM bootstrap. Raw HID records retain their own contract.
(() => {
    'use strict';
    const freeze = Object.freeze, isFrozen = Object.isFrozen, values = Object.values, apply = Reflect.apply;
    const listeners = new Map();
    const receiveDOM=typeof dispatchDomRecord==='function' ? dispatchDomRecord : null;
    let lastEvent = null, received = 0n, resetCount = 0n;
    function immutable(value) {
        if (value && typeof value === 'object' && !isFrozen(value)) {
            for (const child of values(value)) immutable(child);
            freeze(value);
        }
        return value;
    }
    const endpoint = freeze({
        addEventListener(kind, callback, options = {}) {
            if (typeof callback !== 'function') throw new TypeError('Expected input callback');
            kind = String(kind);
            const list = listeners.get(kind) || [];
            if (!list.some(entry => entry.callback === callback)) list.push({callback, once:!!options?.once});
            listeners.set(kind, list);
        },
        removeEventListener(kind, callback) {
            kind = String(kind);
            listeners.set(kind, (listeners.get(kind) || []).filter(entry => entry.callback !== callback));
        },
        snapshot() { return freeze({lastEvent, received:received.toString(), resetCount:resetCount.toString()}); },
    });
    Object.defineProperty(globalThis, 'hullInput', {value:endpoint, writable:false, configurable:false});
    Object.defineProperty(globalThis, '__hullInputReceive', {
        configurable:true,
        value(record) {
            if(receiveDOM && receiveDOM(record))return;
            const event = immutable(record);
            lastEvent = event; received++;
            if (event.kind === 'input-reset') resetCount++;
            let firstError, failed = false;
            const schedule = [...(listeners.get(event.kind) || []).map(entry => [event.kind, entry]), ...(listeners.get('*') || []).map(entry => ['*', entry])];
            for (const [kind,entry] of schedule) {
                if (!(listeners.get(kind) || []).includes(entry)) continue;
                if (entry.once) endpoint.removeEventListener(kind, entry.callback);
                try { apply(entry.callback, endpoint, [event]); }
                catch (error) { if (!failed) { failed=true; firstError=error; } }
            }
            if (failed) throw firstError;
        },
    });
})();

// The actor owns deadlines; only this realm retains callbacks and arguments.
(()=>{
    'use strict';
    const schedule=globalThis.__hullTimerSchedule;
    if(typeof schedule!=='function')return;
    delete globalThis.__hullTimerSchedule;
    const callbacks=new Map(),apply=Reflect.apply,define=Object.defineProperty;
    const receiver=globalThis.__hullInputReceive,page=globalThis;
    let nesting=0;
    const proto=Object.setPrototypeOf;
    function call(args){proto(args,null);return schedule(args);}
    function effectiveDelay(delay,level){return level>5&&delay<4?4:delay;}
    function start(callback,delay,args,repeating){
        if(typeof callback!=='function')throw new TypeError('String timer handlers are not admitted');
        delay=(+delay)>>0;if(delay<0)delay=0;
        const id=call([repeating?'interval':'set',effectiveDelay(delay,nesting)]);
        if(!id){const error=new Error('Pending timer quota exceeded');error.name='QuotaExceededError';throw error;}
        callbacks.set(id,{callback,args,repeating,delay,nesting:nesting+1});return id;
    }
    define(page,'setTimeout',{value:function(callback,delay=0,...args){
        if(!arguments.length)throw new TypeError('setTimeout requires a callback');
        return start(callback,delay,args,false);
    },writable:true,configurable:true});
    define(page,'setInterval',{value:function(callback,delay=0,...args){
        if(!arguments.length)throw new TypeError('setInterval requires a callback');
        return start(callback,delay,args,true);
    },writable:true,configurable:true});
    function clear(id=0){
        id=(+id)>>0;callbacks.delete(id);call(['clear',id]);
    }
    define(page,'clearTimeout',{value:clear,writable:true,configurable:true});
    define(page,'clearInterval',{value:clear,writable:true,configurable:true});
    define(page,'__hullInputReceive',{configurable:true,value:function(record){
        if(record.kind!=='__hull_timer')return receiver(record);
        const timer=callbacks.get(record.id);if(!timer)return;
        if(!timer.repeating)callbacks.delete(record.id);
        const previous=nesting;nesting=timer.nesting;
        try{return apply(timer.callback,page,timer.args);}finally{
            nesting=previous;
            if(timer.repeating&&callbacks.get(record.id)===timer){
                call(['rearm',record.id,effectiveDelay(timer.delay,timer.nesting)]);timer.nesting++;
            }
        }
    }});
})();

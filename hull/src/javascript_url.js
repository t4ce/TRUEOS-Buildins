// Trusted URL binding. Rust parses/serializes URL and form data; JS retains
// object identity, Web IDL string coercion, callbacks and live list iteration.
function installURL(request) {
    'use strict';
    const urls=new WeakMap(), queries=new WeakMap(), iterators=new WeakMap();
    const apply=Reflect.apply, wellFormed=String.prototype.toWellFormed;
    const define=Object.defineProperty, proto=Object.setPrototypeOf, ownKeys=Reflect.ownKeys;
    const descriptor=Object.getOwnPropertyDescriptor, arraySort=Array.prototype.sort;
    const limit=4096;
    function required(args,n){if(args.length<n)throw new TypeError(`Expected ${n} argument(s)`);}
    function usv(value){return apply(wellFormed,`${value}`,[]);}
    function call(op,...args){return request('url',op,...args);}
    function urlState(value){const state=urls.get(value);if(!state)throw new TypeError('Illegal URL invocation');return state;}
    function queryState(value){const state=queries.get(value);if(!state)throw new TypeError('Illegal URLSearchParams invocation');return state;}
    function pairsTransport(entries){
        const result=[];for(const [k,v]of entries){const pair=[k,v];proto(pair,null);result.push(pair);}proto(result,null);return result;
    }
    function serialize(entries){return call('querySerialize',pairsTransport(entries));}
    function parsed(value,base){const input=usv(value);const relative=base===undefined?null:usv(base);return call('parse',input,relative);}
    function updateURL(state,parts){
        state.parts=parts;
        if(state.params)queryState(state.params).entries=call('queryParse',parts.search);
    }
    function mutate(state,entries){
        if(entries.length>limit)throw new RangeError('URLSearchParams pair budget exceeded');
        // Validate/serialize before publishing. Preserve the same params object
        // across href/search replacement; iterators read its current list.
        const encoded=serialize(entries);
        if(state.owner)state.owner.parts=call('set',state.owner.parts.href,'search',encoded?`?${encoded}`:'');
        state.entries=entries;
    }
    class URL {
        constructor(input,base=undefined){required(arguments,1);const parts=parsed(input,base);if(parts===null)throw new TypeError('Invalid URL');urls.set(this,{parts,params:null});}
        get origin(){return urlState(this).parts.origin;}
        get searchParams(){const s=urlState(this);if(!s.params){s.params=new URLSearchParams(s.parts.search);queryState(s.params).owner=s;}return s.params;}
        toString(){return urlState(this).parts.href;}
        toJSON(){return urlState(this).parts.href;}
        static canParse(input,base=undefined){required(arguments,1);return parsed(input,base)!==null;}
        static parse(input,base=undefined){required(arguments,1);const parts=parsed(input,base);if(parts===null)return null;const result=Object.create(URL.prototype);urls.set(result,{parts,params:null});return result;}
    }
    for(const name of ['href','protocol','username','password','host','hostname','port','pathname','search','hash'])define(URL.prototype,name,{
        get(){return urlState(this).parts[name];},
        set(value){const state=urlState(this);const text=usv(value);updateURL(state,call('set',state.parts.href,name,text));},
        enumerable:true,configurable:true,
    });
    function object(value){return value!==null&&(typeof value==='object'||typeof value==='function');}
    function iterableWith(iterator){return {[Symbol.iterator](){return iterator;}};}
    class URLSearchParams {
        constructor(init=undefined){
            const state={entries:[],owner:null};queries.set(this,state);
            if(init===undefined)return;
            if(object(init)){
                const method=init[Symbol.iterator];
                if(method!=null){
                    if(typeof method!=='function')throw new TypeError('Expected an iterator');
                    for(const entry of iterableWith(apply(method,init,[]))){
                        if(!object(entry))throw new TypeError('Expected a name/value sequence');
                        const entryMethod=entry[Symbol.iterator];if(typeof entryMethod!=='function')throw new TypeError('Expected a name/value sequence');
                        const pair=[];for(const value of iterableWith(apply(entryMethod,entry,[]))){if(pair.length===2)throw new TypeError('Expected sequence with length 2');pair.push(value);}
                        if(pair.length!==2)throw new TypeError('Expected sequence with length 2');state.entries.push([usv(pair[0]),usv(pair[1])]);
                        if(state.entries.length>limit)throw new RangeError('URLSearchParams pair budget exceeded');
                    }
                }else{
                    const record=new Map();
                    for(const key of ownKeys(init)){
                        const d=descriptor(init,key);if(d&&d.enumerable){record.set(usv(key),usv(init[key]));if(record.size>limit)throw new RangeError('URLSearchParams pair budget exceeded');}
                    }
                    state.entries=[...record];
                }
                serialize(state.entries);
            }else state.entries=call('queryParse',usv(init));
        }
        get size(){return queryState(this).entries.length;}
        append(name,value){const state=queryState(this);required(arguments,2);const key=usv(name),text=usv(value);mutate(state,[...state.entries,[key,text]]);}
        delete(name,value=undefined){const state=queryState(this);required(arguments,1);const key=usv(name),text=value===undefined?undefined:usv(value);mutate(state,state.entries.filter(([k,v])=>k!==key||(text!==undefined&&v!==text)));}
        get(name){const state=queryState(this);required(arguments,1);const key=usv(name);const pair=state.entries.find(([k])=>k===key);return pair?pair[1]:null;}
        getAll(name){const state=queryState(this);required(arguments,1);const key=usv(name);return state.entries.filter(([k])=>k===key).map(([,v])=>v);}
        has(name,value=undefined){const state=queryState(this);required(arguments,1);const key=usv(name),text=value===undefined?undefined:usv(value);return state.entries.some(([k,v])=>k===key&&(text===undefined||v===text));}
        set(name,value){
            const state=queryState(this);required(arguments,2);const key=usv(name),text=usv(value);let found=false;const result=[];
            for(const [k,v]of state.entries){if(k!==key)result.push([k,v]);else if(!found){result.push([key,text]);found=true;}}
            if(!found)result.push([key,text]);mutate(state,result);
        }
        sort(){const state=queryState(this);const sorted=state.entries.slice();apply(arraySort,sorted,[(a,b)=>a[0]<b[0]?-1:a[0]>b[0]?1:0]);mutate(state,sorted);}
        toString(){return serialize(queryState(this).entries);}
        forEach(callback,thisArg=undefined){const state=queryState(this);if(typeof callback!=='function')throw new TypeError('Expected callback');for(let i=0;i<state.entries.length;i++){const[k,v]=state.entries[i];apply(callback,thisArg,[v,k,this]);}}
        entries(){return iterator(queryState(this),'entries');}
        keys(){return iterator(queryState(this),'keys');}
        values(){return iterator(queryState(this),'values');}
    }
    const iteratorPrototype=Object.create(Object.getPrototypeOf(Object.getPrototypeOf([][Symbol.iterator]())));
    define(iteratorPrototype,Symbol.toStringTag,{value:'URLSearchParams Iterator',configurable:true});
    define(iteratorPrototype,'next',{value:function next(){
        const it=iterators.get(this);if(!it)throw new TypeError('Illegal URLSearchParams iterator invocation');
        if(it.done||it.index>=it.state.entries.length){it.done=true;return{value:undefined,done:true};}
        const[k,v]=it.state.entries[it.index++];return{value:it.type==='keys'?k:it.type==='values'?v:[k,v],done:false};
    },writable:true,configurable:true});
    function iterator(state,type){const result=Object.create(iteratorPrototype);iterators.set(result,{state,type,index:0,done:false});return result;}
    define(URLSearchParams.prototype,Symbol.iterator,{value:URLSearchParams.prototype.entries,writable:true,configurable:true});
    for(const name of ['canParse','parse']){const d=descriptor(URL,name);d.enumerable=true;define(URL,name,d);}
    for(const Constructor of [URL,URLSearchParams]){
        define(Constructor.prototype,Symbol.toStringTag,{value:Constructor.name,configurable:true});
        // Web IDL prototype members are enumerable, unlike class syntax defaults.
        for(const key of Object.getOwnPropertyNames(Constructor.prototype))if(key!=='constructor'){
            const d=descriptor(Constructor.prototype,key);d.enumerable=true;define(Constructor.prototype,key,d);
        }
        define(globalThis,Constructor.name,{value:Constructor,writable:true,configurable:true});
    }
}

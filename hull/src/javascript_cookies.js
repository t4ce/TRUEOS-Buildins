    function cookieInputFits(value) {
        // JSON string bytes plus the jar's 32-byte charge. Stop before sending
        // oversized strings through the existing 1 MiB native JSON bridge.
        if(!/["\\\x00-\x1f\x80-\uffff]/.test(value))return value.length+34<=1048576;
        let bytes=34;
        for(let i=0;i<value.length;i++) {
            const c=value.charCodeAt(i);
            if(c===34||c===92||c===8||c===9||c===10||c===12||c===13)bytes+=2;
            else if(c<32)bytes+=6;
            else if(c<128)bytes++;
            else if(c<2048)bytes+=2;
            else if(c>=0xd800&&c<=0xdbff){bytes+=4;i++;}
            else bytes+=3;
            if(bytes>1048576)return false;
        }
        return true;
    }
    Object.defineProperty(Document.prototype, 'cookie', {
        get() { if(state(this).type!==9)throw new TypeError('Illegal invocation'); return request('cookie'); },
        set(value) {
            if(state(this).type!==9)throw new TypeError('Illegal invocation');
            const text=string(value);
            if(!cookieInputFits(text)){request('cookieFull');return;}
            request('cookie',text);
        },
        enumerable:true, configurable:true
    });
    // Same Rust box and cumulative quota as cookies; only the public protocol
    // and internal key prefix differ. Never reload persisted diagnostic data.
    (()=>{
        const instances=new WeakMap();
        function check(value){const data=instances.get(value);if(!data)throw new TypeError('Illegal Storage invocation');return data;}
        function storageCall(value,...args){return request(check(value).operation,...args);}
        function quota(){throw new DOMException('Navigation storage budget exceeded','QuotaExceededError');}
        class Storage {
            constructor(){throw new TypeError('Illegal constructor');}
            get length(){check(this);return storageCall(this,'length');}
            key(index){check(this);required(arguments,1);return storageCall(this,'key',`${(+index)>>>0}`);}
            getItem(key){check(this);required(arguments,1);return storageCall(this,'get',string(key));}
            setItem(key,value){
                const data=check(this);required(arguments,2);key=string(key);value=string(value);
                if(data.readOnly)return;
                // Account for JSON escaping before crossing the native transport.
                if(!cookieInputFits(key)||!cookieInputFits(value)||!cookieInputFits(key+value+'localStorage:')) {
                    request('cookieFull');quota();
                }
                if(!storageCall(this,'set',key,value))quota();
            }
            removeItem(key){check(this);required(arguments,1);storageCall(this,'remove',string(key));}
            clear(){check(this);storageCall(this,'clear');}
        }
        Object.defineProperty(Storage.prototype,Symbol.toStringTag,{value:'Storage',configurable:true});
        function createStore(operation,readOnly){
        const target=Object.create(Storage.prototype);
        const store=new Proxy(target,{
            get(target,key,receiver){return typeof key==='string'&&!Reflect.has(target,key)?storageCall(store,'get',key)??undefined:Reflect.get(target,key,receiver);},
            set(target,key,value){if(typeof key!=='string')return Reflect.set(target,key,value);Storage.prototype.setItem.call(store,key,value);return true;},
            deleteProperty(target,key){if(typeof key!=='string')return Reflect.deleteProperty(target,key);storageCall(store,'remove',key);return true;},
            has(target,key){return Reflect.has(target,key)||(typeof key==='string'&&storageCall(store,'get',key)!==null);},
            ownKeys(target){return [...storageCall(store,'keys').filter(key=>!Reflect.has(target,key)),...Reflect.ownKeys(target)];},
            getOwnPropertyDescriptor(target,key){
                const own=Reflect.getOwnPropertyDescriptor(target,key);if(own)return own;
                if(typeof key==='string'&&!Reflect.has(target,key)){
                    const value=storageCall(store,'get',key);if(value!==null)return {value,writable:true,enumerable:true,configurable:true};
                }
            },
            defineProperty(target,key,descriptor){
                if(typeof key!=='string')return Reflect.defineProperty(target,key,descriptor);
                if(!Object.hasOwn(descriptor,'value'))return false;
                Storage.prototype.setItem.call(store,key,descriptor.value);return true;
            },
            preventExtensions(){return false;},
        });
        instances.set(store,{operation,readOnly});return store;
        }
        const store=createStore('storage',false),session=createStore('sessionStorage',true);
        Object.defineProperty(globalThis,'Storage',{value:Storage,writable:true,configurable:true});
        Object.defineProperty(globalThis,'localStorage',{get:()=>store,enumerable:true,configurable:true});
        Object.defineProperty(globalThis,'sessionStorage',{get:()=>session,enumerable:true,configurable:true});
    })();

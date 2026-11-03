    const styleObjects=new WeakMap(),styleNodes=new WeakMap();
    function styleNode(value){const node=styleNodes.get(value);if(!node)throw new TypeError('Illegal CSSStyleDeclaration invocation');return node;}
    function styleCall(value,action,name='',text='',priority=''){return request('style',state(styleNode(value)).id,action,name,text,priority);}
    function cssName(name){
        if(name==='cssFloat')return 'float';
        if(name.startsWith('--'))return name;
        if(name.startsWith('webkit'))name='Webkit'+name.slice(6);
        if(name.startsWith('ms')&&/[A-Z]/.test(name[2]??''))name='Ms'+name.slice(2);
        return name.replace(/[A-Z]/g,c=>'-'+c.toLowerCase());
    }
    class CSSStyleDeclaration {
        constructor(){throw new TypeError('Illegal constructor');}
        get cssText(){return styleCall(this,'text');}
        set cssText(value){styleCall(this,'cssText','',value==null?'':string(value));}
        get length(){return styleCall(this,'names').length;}
        get parentRule(){styleNode(this);return null;}
        item(index){required(arguments,1);return styleCall(this,'names')[(+index)>>>0]??'';}
        getPropertyValue(name){required(arguments,1);return styleCall(this,'get',string(name));}
        getPropertyPriority(name){required(arguments,1);return styleCall(this,'priority',string(name));}
        setProperty(name,value,priority=''){required(arguments,2);styleCall(this,'set',string(name),value==null?'':string(value),priority==null?'':string(priority));}
        removeProperty(name){required(arguments,1);return styleCall(this,'remove',string(name));}
        *[Symbol.iterator](){yield* styleCall(this,'names');}
    }
    Object.defineProperty(CSSStyleDeclaration.prototype,Symbol.toStringTag,{value:'CSSStyleDeclaration',configurable:true});
    Object.defineProperty(globalThis,'CSSStyleDeclaration',{value:CSSStyleDeclaration,writable:true,configurable:true});
    function inlineStyle(node){
        if(state(node).type!==1)throw new TypeError('Expected an Element');
        if(styleObjects.has(node))return styleObjects.get(node);
        const target=Object.create(CSSStyleDeclaration.prototype);
        const proxy=new Proxy(target,{
            get(target,key,receiver){
                if(typeof key!=='string'||Reflect.has(target,key))return Reflect.get(target,key,receiver);
                if(/^(0|[1-9][0-9]*)$/.test(key))return styleCall(proxy,'names')[Number(key)];
                const name=cssName(key);return !name.startsWith('--')&&styleCall(proxy,'supports',name)?styleCall(proxy,'get',name):undefined;
            },
            set(target,key,value,receiver){
                if(typeof key!=='string'||Reflect.has(target,key))return Reflect.set(target,key,value,receiver);
                if(/^(0|[1-9][0-9]*)$/.test(key))return false;
                const name=cssName(key);if(!name.startsWith('--'))styleCall(proxy,'set',name,value==null?'':string(value));return true;
            },
            has(target,key){return Reflect.has(target,key)||(typeof key==='string'&&!key.startsWith('--')&&styleCall(proxy,'supports',cssName(key)));},
        });
        styleNodes.set(proxy,node);styleObjects.set(node,proxy);return proxy;
    }

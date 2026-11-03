// Navigation is diagnostic only: retain the current document and never fetch,
// execute javascript: URLs, or create another browsing context.
if (eventWindow !== null) (() => {
    const page=eventWindow, define=Object.defineProperty, apply=Reflect.apply;
    const URLClass=globalThis.URL, output=globalThis.console?.info;
    const current=new URLClass(document.URL), base=document.baseURI;
    let attempts=0;
    function blocked(kind,target) {
        request("mediaNavigation",kind,target);
        if(attempts>3)return;
        if(attempts++===3) {
            if(output)apply(output,undefined,['hull: navigation log exceeded; further attempts silent']);
            return;
        }
        if(output)apply(output,undefined,[`hull: blocked navigation (${kind}) ${target.slice(0,2048)}`]);
    }
    function resolve(value) {
        const url=URLClass.parse(`${value}`,base);
        if(!url)throw new DOMException('Invalid navigation URL','SyntaxError');
        return url.href;
    }
    class Location {
        constructor(){throw new TypeError('Illegal constructor');}
    }
    const location=Object.create(Location.prototype);
    function checked(value){if(value!==location)throw new TypeError('Illegal Location invocation');}
    for(const name of ['href','origin','protocol','host','hostname','port','pathname','search','hash']) {
        const descriptor={get(){checked(this);return current[name];},enumerable:true};
        if(name!=='origin')descriptor.set=function(value){
            checked(this);
            if(name==='href')blocked('location.href',resolve(value));
            else {const target=new URLClass(current.href);target[name]=value;blocked(`location.${name}`,target.href);}
        };
        define(location,name,descriptor);
    }
    for(const name of ['assign','replace'])define(location,name,{value:function(value){
        checked(this);if(!arguments.length)throw new TypeError('Expected a URL');blocked(`location.${name}`,resolve(value));
    },enumerable:true});
    define(location,'reload',{value:function(){checked(this);blocked('location.reload',current.href);},enumerable:true});
    define(location,'toString',{value:function(){checked(this);return current.href;},enumerable:true});
    define(Location.prototype,Symbol.toStringTag,{value:'Location',configurable:true});
    define(page,'Location',{value:Location,writable:true,configurable:true});
    const descriptor={get:()=>location,set:value=>{blocked('location',resolve(value));},enumerable:true};
    define(page,'location',descriptor);
    define(document,'location',descriptor);
    define(page,'open',{value:function(url='',target='',features=''){
        // Preserve ordinary argument conversion even though no window is opened.
        const address=`${url}`;`${target}`;`${features}`;
        blocked('window.open',address===''?'about:blank':resolve(address));return null;
    },writable:true,configurable:true});
})();

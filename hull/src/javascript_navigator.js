// A per-window virtual browser identity. Unsupported capabilities stay absent.
if(eventWindow!==null)(()=>{
    const define=Object.defineProperty;
    const pluginArrays=new WeakSet(),mimeArrays=new WeakSet();
    function check(set,value){if(!set.has(value))throw new TypeError('Illegal invocation');}
    function argument(args){if(!args.length)throw new TypeError('Expected one argument');}
    class PluginArray {
        constructor(){throw new TypeError('Illegal constructor');}
        get length(){check(pluginArrays,this);return 0;}
        item(index){check(pluginArrays,this);argument(arguments);(+index)>>>0;return null;}
        namedItem(name){check(pluginArrays,this);argument(arguments);`${name}`;return null;}
        refresh(){check(pluginArrays,this);}
        *[Symbol.iterator](){check(pluginArrays,this);}
    }
    class MimeTypeArray {
        constructor(){throw new TypeError('Illegal constructor');}
        get length(){check(mimeArrays,this);return 0;}
        item(index){check(mimeArrays,this);argument(arguments);(+index)>>>0;return null;}
        namedItem(name){check(mimeArrays,this);argument(arguments);`${name}`;return null;}
        *[Symbol.iterator](){check(mimeArrays,this);}
    }
    let plugins=null,mimeTypes=null;
    function navigatorState(value){if(value!==navigator)throw new TypeError('Illegal Navigator invocation');}
    class Navigator {
        constructor(){throw new TypeError('Illegal constructor');}
        // Matches the current kernel page/media HTTP request identity.
        get userAgent(){navigatorState(this);return 'TRUEOS net-fetch';}
        get platform(){navigatorState(this);return 'TRUEOS';}
        get plugins(){
            navigatorState(this);
            if(!plugins){plugins=Object.create(PluginArray.prototype);pluginArrays.add(plugins);}
            return plugins;
        }
        get mimeTypes(){
            navigatorState(this);
            if(!mimeTypes){mimeTypes=Object.create(MimeTypeArray.prototype);mimeArrays.add(mimeTypes);}
            return mimeTypes;
        }
        get pdfViewerEnabled(){navigatorState(this);return false;}
    }
    const navigator=Object.create(Navigator.prototype);
    for(const Constructor of [Navigator,PluginArray,MimeTypeArray]){
        define(Constructor.prototype,Symbol.toStringTag,{value:Constructor.name,configurable:true});
        define(eventWindow,Constructor.name,{value:Constructor,writable:true,configurable:true});
    }
    for(const name of ['navigator','clientInformation'])define(eventWindow,name,{get:()=>navigator,enumerable:true,configurable:true});
})();

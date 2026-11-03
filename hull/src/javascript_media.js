    // Video is a URL handoff to the separate native player, never an embedded
    // texture. A play promise settles only on a host first-frame receipt.
    const mediaStates=new WeakMap();
    let mediaPending=null;
    let mediaActive=null;
    function mediaState(node){
        const s=state(node);if(s.type!==1||s.name!=='VIDEO')throw new TypeError('Expected HTMLVideoElement');
        if(!mediaStates.has(node))mediaStates.set(node,{paused:true,currentSrc:'',promise:null});
        return mediaStates.get(node);
    }
    function resolvedMedia(node,value){
        if(value===null)return '';
        const parts=request('url','parse',value,get(node,'baseURI'));return parts?parts.href:value;
    }
    class HTMLMediaElement extends HTMLElement {
        get src(){mediaState(this);return resolvedMedia(this,attribute(this,'get','src'));}
        set src(value){mediaState(this);attribute(this,'set','src',string(value));}
        get currentSrc(){return mediaState(this).currentSrc;}
        get paused(){return mediaState(this).paused;}
        play(){
            try {
                const s=mediaState(this);
                let source=attribute(this,'get','src');
                if(source===null){const child=this.querySelector(':scope > source[src]');source=child?child.getAttribute('src'):null;}
                const url=resolvedMedia(this,source);
                if(!url)return Promise.reject(new DOMException('No video source','NotSupportedError'));
                if(mediaPending?.node===this&&mediaPending.url===url)return s.promise;
                if(!mediaPending&&!s.paused&&s.currentSrc===url)return Promise.resolve();
                const id=request('mediaPlay',state(this).id,url);
                if(mediaPending)mediaPending.reject(new DOMException('Superseded by a newer play request','AbortError'));
                s.promise=new Promise((resolve,reject)=>{mediaPending={id,node:this,url,resolve,reject};});
                return s.promise;
            }catch(error){return Promise.reject(error);}
        }
    }
    class HTMLVideoElement extends HTMLMediaElement {}
    function receiveMedia(record){
        if(record.kind!=='__hull_media_receipt')return false;
        if(!mediaPending||mediaPending.id!==record.id)return true;
        const pending=mediaPending;mediaPending=null;
        if(record.status===1||record.status===2){
            if(mediaActive&&mediaActive!==pending.node)mediaState(mediaActive).paused=true;
            mediaActive=pending.node;
            mediaState(pending.node).currentSrc=pending.url;
            mediaState(pending.node).paused=false;
            dispatchEventAt(pending.node,new Event('playing'),true);pending.resolve();
        }else pending.reject(new DOMException('Native video playback failed','NotSupportedError'));
        return true;
    }

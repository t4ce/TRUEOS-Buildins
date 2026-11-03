    function cookieInputFits(value) {
        // JSON string bytes plus the jar's 32-byte charge. Stop before sending
        // oversized strings through the existing 1 MiB native JSON bridge.
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
